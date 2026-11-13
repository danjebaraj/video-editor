//! Headless Chrome renderer for HTML / SVG / Lottie → alpha frames → ProRes 4444 mezzanine.
//!
//! Deterministic time: for every frame `t` the page's Web Animations are paused and seeked
//! (`currentTime = t·1000`), SVG SMIL documents are paused and `setCurrentTime(t)`, Lottie uses
//! `goToAndStop(frame, true)`, and an optional `window.__cutline_seek(t)` hook is awaited.
//! Network is sandboxed: only `file://` URLs under the allowed directories load; everything
//! else fails with `BlockedByClient` (recorded for tests).

use anyhow::{bail, Context, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::dom::Rgba;
use chromiumoxide::cdp::browser_protocol::emulation::{
    SetDefaultBackgroundColorOverrideParams, SetDeviceMetricsOverrideParams,
};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EnableParams, EventRequestPaused, FailRequestParams, RequestPattern,
};
use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, CaptureScreenshotParams};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Html,
    Svg,
    Lottie,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderSpec {
    pub kind: Kind,
    pub source: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
    pub alpha: bool,
    pub params: serde_json::Value,
    /// Directories the page may read (`file://`); the source's directory is always allowed.
    pub allow_dirs: Vec<PathBuf>,
    pub device_scale: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RenderResult {
    pub frames: Vec<PathBuf>,
    pub blocked_requests: Vec<String>,
}

pub const FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// Chrome executable: `CUTLINE_CHROME`, installed headless shell, system Chrome/Edge.
pub fn chrome() -> Option<PathBuf> {
    cutline_deps::paths::find_chrome()
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = if s.starts_with('/') { s } else { format!("/{s}") };
    let mut out = String::from("file://");
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn url_to_path(u: &str) -> Option<PathBuf> {
    let rest = u.strip_prefix("file://")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let mut bytes = vec![];
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).ok()?, 16) {
                bytes.push(v);
                i += 3;
                continue;
            }
        }
        bytes.push(b[i]);
        i += 1;
    }
    let s = String::from_utf8(bytes).ok()?;
    #[cfg(windows)]
    let s = s.trim_start_matches('/').to_string();
    Some(PathBuf::from(s))
}

/// Wrapper page for SVG / Lottie sources (HTML is loaded directly).
fn wrapper(spec: &RenderSpec, work: &Path) -> Result<PathBuf> {
    let (w, h) = (spec.width, spec.height);
    let body = match spec.kind {
        Kind::Html => return Ok(spec.source.clone()),
        Kind::Svg => {
            let svg = std::fs::read_to_string(&spec.source)?;
            format!("<div id=\"root\" style=\"width:{w}px;height:{h}px\">{svg}</div><style>#root svg{{width:100%;height:100%}}</style>")
        }
        Kind::Lottie => {
            let json = std::fs::read_to_string(&spec.source)?;
            let lottie = cutline_deps::paths::assets_dir().join("html").join("lottie.min.js");
            format!(
                "<div id=\"root\" style=\"width:{w}px;height:{h}px\"></div><script src=\"{}\"></script><script>
window.__cutline_lottie = lottie.loadAnimation({{container: document.getElementById('root'), renderer: 'svg', loop: false, autoplay: false, animationData: {json}}});
</script>",
                file_url(&lottie)
            )
        }
    };
    let html = format!("<!doctype html><html><head><meta charset=\"utf-8\"><style>html,body{{margin:0;padding:0;background:transparent;overflow:hidden}}</style></head><body>{body}</body></html>");
    let p = work.join("wrapper.html");
    std::fs::write(&p, html)?;
    Ok(p)
}

const SEEK_JS: &str = r#"async (t) => {
  for (const a of document.getAnimations()) { try { a.pause(); a.currentTime = t * 1000; } catch (e) {} }
  if (window.__cutline_lottie) {
    const an = window.__cutline_lottie;
    an.goToAndStop(Math.min(t * an.frameRate, an.totalFrames - 1), true);
  }
  for (const s of document.querySelectorAll('svg')) {
    if (s.pauseAnimations && s.ownerSVGElement === null) { s.pauseAnimations(); s.setCurrentTime(t); }
  }
  if (window.__cutline_seek) { await window.__cutline_seek(t); }
  await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
  return true;
}"#;

const READY_JS: &str = r#"async () => {
  if (document.fonts && document.fonts.ready) { await document.fonts.ready; }
  await Promise.all(Array.from(document.images).map(i => i.complete ? 1 : new Promise(r => { i.onload = i.onerror = r; })));
  if (window.__cutline_lottie && !window.__cutline_lottie.isLoaded) {
    await new Promise(r => window.__cutline_lottie.addEventListener('DOMLoaded', r));
  }
  return true;
}"#;

/// Render all frames to PNG files in `work`.
pub fn render_frames(spec: &RenderSpec, work: &Path) -> Result<RenderResult> {
    std::fs::create_dir_all(work)?;
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    rt.block_on(render_async(spec, work))
}

async fn render_async(spec: &RenderSpec, work: &Path) -> Result<RenderResult> {
    let exe =
        chrome().context("no Chrome/Chromium/Edge found (run `cutline deps install chrome` or set CUTLINE_CHROME)")?;
    let profile = work.join("chrome-profile");
    let mut cfg = BrowserConfig::builder()
        .chrome_executable(&exe)
        .user_data_dir(&profile)
        .window_size(spec.width, spec.height)
        .viewport(None)
        .request_timeout(FRAME_TIMEOUT)
        .args([
            "--disable-gpu",
            "--hide-scrollbars",
            "--mute-audio",
            "--force-color-profile=srgb",
            "--font-render-hinting=none",
            "--disable-font-subpixel-positioning",
            "--disable-lcd-text",
            "--run-all-compositor-stages-before-draw",
            "--disable-threaded-animation",
            "--disable-threaded-scrolling",
            "--disable-checker-imaging",
            "--disable-background-networking",
            "--disable-extensions",
            "--allow-file-access-from-files",
        ]);
    if cfg!(target_os = "linux") {
        cfg = cfg.no_sandbox();
    }
    let cfg = cfg.build().map_err(|e| anyhow::anyhow!("chrome config: {e}"))?;
    let (mut browser, mut handler) = Browser::launch(cfg).await.context("launching Chrome")?;
    let h = tokio::spawn(async move { while let Some(_ev) = handler.next().await {} });
    let res = drive(&browser, spec, work).await;
    let _ = browser.close().await;
    let _ = browser.wait().await;
    h.abort();
    let _ = std::fs::remove_dir_all(&profile);
    res
}

async fn drive(browser: &Browser, spec: &RenderSpec, work: &Path) -> Result<RenderResult> {
    let page = browser.new_page("about:blank").await?;
    page.execute(SetDeviceMetricsOverrideParams::new(spec.width as i64, spec.height as i64, spec.device_scale, false))
        .await?;
    if spec.alpha {
        page.execute(SetDefaultBackgroundColorOverrideParams { color: Some(Rgba { r: 0, g: 0, b: 0, a: Some(0.0) }) })
            .await?;
    }
    // network sandbox
    let mut allow: Vec<PathBuf> = spec.allow_dirs.iter().filter_map(|d| d.canonicalize().ok()).collect();
    if let Some(d) = spec.source.parent().and_then(|d| d.canonicalize().ok()) {
        allow.push(d);
    }
    if let Ok(d) = cutline_deps::paths::assets_dir().canonicalize() {
        allow.push(d);
    }
    if let Ok(d) = work.canonicalize() {
        allow.push(d);
    }
    page.execute(EnableParams {
        patterns: Some(vec![RequestPattern {
            url_pattern: Some("*".into()),
            resource_type: None,
            request_stage: None,
        }]),
        handle_auth_requests: None,
    })
    .await?;
    let blocked: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let mut paused = page.event_listener::<EventRequestPaused>().await?;
    let p2 = page.clone();
    let b2 = blocked.clone();
    let guard = tokio::spawn(async move {
        while let Some(ev) = paused.next().await {
            let url = ev.request.url.clone();
            let ok = url.starts_with("data:")
                || url.starts_with("blob:")
                || url_to_path(&url)
                    .and_then(|p| p.canonicalize().ok())
                    .map(|p| allow.iter().any(|a| p.starts_with(a)))
                    .unwrap_or(false);
            if ok {
                let _ = p2.execute(ContinueRequestParams::new(ev.request_id.clone())).await;
            } else {
                b2.lock().unwrap().push(url);
                let _ = p2.execute(FailRequestParams::new(ev.request_id.clone(), ErrorReason::BlockedByClient)).await;
            }
        }
    });
    let params = serde_json::to_string(&spec.params)?;
    page.evaluate_on_new_document(format!("window.__cutline_params = {params};")).await?;
    let entry = wrapper(spec, work)?;
    let entry = entry.canonicalize().unwrap_or(entry);
    tokio::time::timeout(FRAME_TIMEOUT, page.goto(file_url(&entry))).await.context("page load timeout")??;
    tokio::time::timeout(FRAME_TIMEOUT, page.evaluate_function(READY_JS)).await.context("fonts/images timeout")??;
    // warm-up seek: the first seek can race animations already running on the compositor
    tokio::time::timeout(FRAME_TIMEOUT, page.evaluate(format!("({SEEK_JS})(0.05)")))
        .await
        .context("warm-up seek timeout")??;
    let n = (spec.duration * spec.fps).round().max(1.0) as usize;
    let mut frames = vec![];
    for i in 0..n {
        let t = i as f64 / spec.fps;
        let call = format!("({SEEK_JS})({t})");
        tokio::time::timeout(FRAME_TIMEOUT, page.evaluate(call))
            .await
            .with_context(|| format!("frame {i}: seek timeout"))??;
        let shot = CaptureScreenshotParams {
            format: Some(CaptureScreenshotFormat::Png),
            from_surface: Some(true),
            capture_beyond_viewport: Some(false),
            ..Default::default()
        };
        let png = tokio::time::timeout(FRAME_TIMEOUT, page.screenshot(shot))
            .await
            .with_context(|| format!("frame {i}: capture timeout"))??;
        let p = work.join(format!("frame_{i:05}.png"));
        std::fs::write(&p, png)?;
        frames.push(p);
    }
    guard.abort();
    let blocked_requests = blocked.lock().unwrap().clone();
    Ok(RenderResult { frames, blocked_requests })
}

/// Encode rendered PNG frames to a mezzanine (ProRes 4444 with alpha, else 422 HQ).
pub fn encode(tc: &cutline_deps::Toolchain, spec: &RenderSpec, work: &Path, out: &Path) -> Result<()> {
    let mut c = tc.ffmpeg_cmd();
    c.args(["-framerate", &format!("{}", spec.fps), "-i"]).arg(work.join("frame_%05d.png"));
    c.args(["-vf", &format!("scale={}:{}:flags=lanczos", spec.width, spec.height)]);
    if spec.alpha {
        c.args(["-c:v", "prores_ks", "-profile:v", "4444", "-pix_fmt", "yuva444p10le", "-alpha_bits", "16"]);
    } else {
        c.args(["-c:v", "prores_ks", "-profile:v", "3", "-pix_fmt", "yuv422p10le"]);
    }
    c.args([
        "-vendor",
        "apl0",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
    ]);
    c.args(["-map_metadata", "-1", "-fflags", "+bitexact"]).arg(out);
    let o = c.output()?;
    if !o.status.success() {
        bail!("encoding HTML frames failed: {}", String::from_utf8_lossy(&o.stderr));
    }
    Ok(())
}

/// Render to a cached mezzanine. Key: source content (+ sibling files for HTML), params, size,
/// fps, duration, alpha, Chrome path.
pub fn render_cached(tc: &cutline_deps::Toolchain, spec: &RenderSpec, cache_dir: &Path) -> Result<PathBuf> {
    let mut parts: Vec<Vec<u8>> = vec![std::fs::read(&spec.source)?];
    if spec.kind == Kind::Html {
        if let Some(dir) = spec.source.parent() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p != &spec.source)
                .collect();
            files.sort();
            for f in files.iter().take(64) {
                if let Ok(fp) = cutline_media::fingerprint(f) {
                    parts.push(fp.into_bytes());
                }
            }
        }
    }
    parts.push(serde_json::to_vec(&(
        spec.kind,
        &spec.params,
        spec.width,
        spec.height,
        spec.fps,
        spec.duration,
        spec.alpha,
        spec.device_scale,
    ))?);
    parts.push(chrome().map(|p| p.to_string_lossy().to_string()).unwrap_or_default().into_bytes());
    parts.push(b"html-v1".to_vec());
    let refs: Vec<&[u8]> = parts.iter().map(|v| v.as_slice()).collect();
    let key = cutline_media::hash_key(&refs);
    let out = cache_dir.join(format!("{key}.mov"));
    if out.exists() {
        return Ok(out);
    }
    let work = cache_dir.join(format!("{key}.work"));
    let _ = std::fs::remove_dir_all(&work);
    render_frames(spec, &work)?;
    std::fs::create_dir_all(cache_dir)?;
    let tmp = cache_dir.join(format!("{key}.tmp.mov"));
    encode(tc, spec, &work, &tmp)?;
    std::fs::rename(&tmp, &out)?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn urls_roundtrip() {
        let p = PathBuf::from("/tmp/ü dir/a b.html");
        let u = file_url(&p);
        assert!(u.starts_with("file:///tmp/%C3%BC%20dir/"));
        assert_eq!(url_to_path(&u).unwrap(), p);
        assert!(url_to_path("https://x").is_none());
    }
}
