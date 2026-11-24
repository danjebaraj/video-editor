//! Headless Chrome renderer for HTML / SVG / Lottie → alpha frames → ProRes 4444 mezzanine.
//!
//! Deterministic time: for every frame `t` the page's Web Animations are paused and seeked
//! (`currentTime = t·1000`), SVG SMIL documents are paused and `setCurrentTime(t)`, Lottie uses
//! `goToAndStop(frame, true)`, and an optional `window.__cutline_seek(t)` hook is awaited.
//! Network is sandboxed: only `file://` URLs under the allowed directories load; everything
//! else fails with `BlockedByClient` (recorded for tests).

pub mod delegate;
pub mod demo;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);

/// Chrome executable: `CUTLINE_CHROME`, installed headless shell, system Chrome/Edge.
pub fn chrome() -> Option<PathBuf> {
    cutline_deps::paths::find_chrome()
}

/// `p` without a Windows verbatim prefix: `\\?\C:\x` → `C:\x`, `\\?\UNC\srv\share` →
/// `\\srv\share`. `canonicalize` returns verbatim paths on Windows, and neither Chrome's URLs nor
/// `Path::starts_with` treat them as the same place as the plain form. Elsewhere `p` unchanged.
pub(crate) fn plain_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

pub(crate) fn file_url(p: &Path) -> String {
    let s = plain_path(p).to_string_lossy().replace('\\', "/");
    // `//server/share/x` (UNC) becomes `file://server/share/x`; `C:/x` becomes `file:///C:/x`.
    let s = if let Some(unc) = s.strip_prefix("//") {
        unc.to_string()
    } else if s.starts_with('/') {
        s
    } else {
        format!("/{s}")
    };
    let mut out = String::from("file://");
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub(crate) fn url_to_path(u: &str) -> Option<PathBuf> {
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
    let s = match s.strip_prefix('/') {
        // `file:///C:/x`
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_string(),
        // `file:////server/share` (a UNC path written with an empty host)
        Some(rest) if rest.starts_with('/') => format!("/{rest}"),
        Some(rest) => format!("/{rest}"),
        // `file://server/share/x`
        None => format!("//{s}"),
    };
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

/// Marker for "Chrome launched but never got a page ready / produced a frame" (as opposed to a
/// broken page), which is worth retrying with the next browser.
const STALL: &str = "Chrome produced no frames";

async fn render_async(spec: &RenderSpec, work: &Path) -> Result<RenderResult> {
    render_candidates(&cutline_deps::paths::chrome_candidates(), spec, work).await
}

/// Tries each browser in turn. Every attempt starts from a work folder with no frames, and a
/// failed one leaves none behind, so frames can only come from the attempt that succeeded.
pub(crate) async fn render_candidates(cands: &[PathBuf], spec: &RenderSpec, work: &Path) -> Result<RenderResult> {
    if cands.is_empty() {
        bail!("no Chrome/Chromium/Edge found (run `cutline deps install chrome` or set CUTLINE_CHROME)");
    }
    let mut failures: Vec<String> = vec![];
    for (i, exe) in cands.iter().enumerate() {
        clear_frames(work);
        match render_with(exe, spec, work).await {
            Ok(r) => return Ok(r),
            Err(e) if format!("{e:#}").contains(STALL) => {
                clear_frames(work);
                if let Some(next) = cands.get(i + 1) {
                    tracing::warn!("{e:#}; retrying with {}", next.display());
                }
                failures.push(format!("{e:#}"));
            }
            Err(e) => {
                clear_frames(work);
                return Err(e);
            }
        }
    }
    bail!(
        "no Chrome rendered frames ({} tried):\n  - {}\n{}",
        failures.len(),
        failures.join("\n  - "),
        exhausted_hint()
    )
}

/// What to do when no browser started.
pub(crate) fn exhausted_hint() -> &'static str {
    if std::env::var_os("CODEX_SANDBOX").is_some() {
        "Chrome can't start inside Codex's sandbox (on macOS it may not register its Mach services): \
         render where Chrome can start, or set CUTLINE_BROWSER_DELEGATE to a program that does"
    } else {
        "On macOS a freshly downloaded Chrome can stall for a few minutes while the system checks it on \
         first launch: retry shortly, or set CUTLINE_CHROME to an installed Chrome"
    }
}

/// Removes the frames an attempt wrote.
fn clear_frames(work: &Path) {
    let Ok(rd) = std::fs::read_dir(work) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("frame_") && name.ends_with(".png") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Why a browser gave no DevTools address (BUG-stellar-lily-K8CN): the executable, whether it
/// exited during startup or timed out, after how long, its exit status when known, and its
/// stderr (or that it was empty).
pub(crate) fn launch_failure(exe: &Path, elapsed: Duration, e: &chromiumoxide::error::CdpError) -> String {
    use chromiumoxide::error::CdpError;
    let stderr_of = |b: &[u8]| {
        let text = String::from_utf8_lossy(b);
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if lines.is_empty() {
            "stderr empty".to_string()
        } else {
            format!("stderr: {}", lines[lines.len().saturating_sub(4)..].join(" | "))
        }
    };
    let secs = elapsed.as_secs_f64();
    let what = match e {
        CdpError::LaunchExit(status, err) => {
            format!("exited during startup after {secs:.2} s ({status}); {}", stderr_of(err.as_slice()))
        }
        CdpError::LaunchIo(io, err)
            if io.kind() == std::io::ErrorKind::UnexpectedEof || io.to_string().contains("end of stream") =>
        {
            format!("exited during startup after {secs:.2} s (exit status unknown); {}", stderr_of(err.as_slice()))
        }
        CdpError::LaunchIo(io, err) => {
            format!("failed during startup after {secs:.2} s ({io}); {}", stderr_of(err.as_slice()))
        }
        CdpError::LaunchTimeout(err) => {
            format!("gave no DevTools address within {secs:.0} s (startup timeout); {}", stderr_of(err.as_slice()))
        }
        other => format!("failed to launch after {secs:.2} s: {other}"),
    };
    format!("{STALL}: {}: {what}", exe.display())
}

async fn render_with(exe: &Path, spec: &RenderSpec, work: &Path) -> Result<RenderResult> {
    let profile = work.join("chrome-profile");
    let mut cfg = BrowserConfig::builder()
        .chrome_executable(exe)
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
    let started = std::time::Instant::now();
    let (mut browser, mut handler) = match Browser::launch(cfg).await {
        Ok(b) => b,
        Err(e) => bail!(launch_failure(exe, started.elapsed(), &e)),
    };
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
    // Health check independent of the page content: does this browser produce frames at all?
    let alive =
        tokio::time::timeout(HEALTH_TIMEOUT, page.evaluate("new Promise(r => requestAnimationFrame(() => r(1)))"));
    match alive.await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => bail!("{STALL}: {e}"),
        Err(_) => bail!("{STALL} within {}s", HEALTH_TIMEOUT.as_secs()),
    }
    page.execute(SetDeviceMetricsOverrideParams::new(spec.width as i64, spec.height as i64, spec.device_scale, false))
        .await?;
    if spec.alpha {
        page.execute(SetDefaultBackgroundColorOverrideParams { color: Some(Rgba { r: 0, g: 0, b: 0, a: Some(0.0) }) })
            .await?;
    }
    // network sandbox
    let mut allow: Vec<PathBuf> =
        spec.allow_dirs.iter().filter_map(|d| d.canonicalize().ok()).map(|d| plain_path(&d)).collect();
    if let Some(d) = spec.source.parent().and_then(|d| d.canonicalize().ok()) {
        allow.push(d);
    }
    if let Ok(d) = cutline_deps::paths::assets_dir().canonicalize() {
        allow.push(d);
    }
    if let Ok(d) = work.canonicalize() {
        allow.push(d);
    }
    let allow: Vec<PathBuf> = allow.iter().map(|d| plain_path(d)).collect();
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
                    .map(|p| plain_path(&p))
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
            // Still lossless: Chrome spends less time compressing each PNG, and the frames are
            // re-encoded into the mezzanine straight away.
            optimize_for_speed: Some(true),
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

/// Another HTML page (as opposed to a style sheet, script, image or data file).
fn is_page(p: &Path) -> bool {
    matches!(p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("html" | "htm"))
}

/// Render to a cached mezzanine. Key: source content (+ sibling files for HTML), params, size,
/// fps, duration, alpha, Chrome path.
pub fn render_cached(tc: &cutline_deps::Toolchain, spec: &RenderSpec, cache_dir: &Path) -> Result<PathBuf> {
    let mut parts: Vec<Vec<u8>> = vec![std::fs::read(&spec.source)?];
    if spec.kind == Kind::Html {
        if let Some(dir) = spec.source.parent() {
            // Files beside the page can be part of it (styles, scripts, images), so they are part
            // of the key. Another page in the same folder is only part of it when this page names
            // it: otherwise editing one slide would re-render every slide in the folder.
            let own = std::fs::read_to_string(&spec.source).unwrap_or_default();
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p != &spec.source)
                .filter(|p| !is_page(p) || p.file_name().and_then(|n| n.to_str()).is_some_and(|n| own.contains(n)))
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
    // where Chrome can't start (Codex's sandbox), another process renders into this cache
    if let Some(program) = delegate::delegate() {
        delegate::run(&program, &delegate::Job::Html { spec: spec.clone(), cache_dir: cache_dir.to_path_buf() })?;
        if out.exists() {
            return Ok(out);
        }
        bail!("the browser renderer {} finished without writing {}", program.display(), out.display());
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

    /// BUG-stellar-lily-K8CN: a browser that dies at launch, one that complains and dies, and
    /// (when one is installed) a real one to fall back to.
    #[cfg(unix)]
    mod launch {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        }

        fn card(dir: &Path) -> RenderSpec {
            let page = dir.join("card.html");
            std::fs::write(&page, "<html><body style='margin:0;background:#ffb000'></body></html>").unwrap();
            RenderSpec {
                kind: Kind::Html,
                source: page,
                width: 64,
                height: 64,
                fps: 5.0,
                duration: 0.4,
                alpha: false,
                params: serde_json::Value::Null,
                allow_dirs: vec![],
                device_scale: 1.0,
            }
        }

        fn frames_in(work: &Path) -> usize {
            std::fs::read_dir(work)
                .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("frame_")).count())
                .unwrap_or(0)
        }

        fn render(cands: &[PathBuf], spec: &RenderSpec, work: &Path) -> Result<RenderResult> {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(render_candidates(cands, spec, work))
        }

        #[test]
        fn every_browser_exiting_at_launch_is_reported_and_leaves_no_frames() {
            let d = tempfile::tempdir().unwrap();
            let silent = fake(d.path(), "silent-chrome", "exit 3");
            let loud = fake(d.path(), "loud-chrome", "echo 'FATAL: bootstrap_check_in denied' >&2; exit 5");
            let work = d.path().join("work");
            std::fs::create_dir_all(&work).unwrap();
            // frames from an earlier attempt must not survive as this render's output
            std::fs::write(work.join("frame_00000.png"), b"stale").unwrap();
            let err = format!("{:#}", render(&[silent.clone(), loud.clone()], &card(d.path()), &work).unwrap_err());
            assert!(err.contains("2 tried"), "{err}");
            for exe in [&silent, &loud] {
                assert!(err.contains(&exe.display().to_string()), "{err}");
            }
            assert!(err.contains("exited during startup after"), "{err}");
            assert!(err.contains("stderr empty"), "{err}");
            assert!(err.contains("FATAL: bootstrap_check_in denied"), "{err}");
            assert_eq!(frames_in(&work), 0, "a failed render left frames behind");
        }

        #[test]
        fn a_browser_that_exits_at_launch_falls_back_to_the_next() {
            let Some(chrome) = chrome() else {
                eprintln!("skipped: no Chrome");
                return;
            };
            let d = tempfile::tempdir().unwrap();
            let silent = fake(d.path(), "silent-chrome", "exit 3");
            let work = d.path().join("work");
            let spec = card(d.path());
            let r = render(&[silent, chrome], &spec, &work).unwrap();
            assert_eq!(r.frames.len(), 2);
            assert_eq!(frames_in(&work), 2);
        }

        #[test]
        fn a_startup_timeout_reads_differently_from_an_exit() {
            use chromiumoxide::error::{BrowserStderr, CdpError};
            let exe = Path::new("/x/chrome");
            let t = launch_failure(exe, Duration::from_secs(20), &CdpError::LaunchTimeout(BrowserStderr::new(vec![])));
            assert!(
                t.contains("no DevTools address within 20 s (startup timeout)") && t.contains("stderr empty"),
                "{t}"
            );
            let eof = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "unexpected end of stream");
            let x = launch_failure(
                exe,
                Duration::from_millis(40),
                &CdpError::LaunchIo(eof, BrowserStderr::new(b"boom\n".to_vec())),
            );
            assert!(
                x.contains("exited during startup after 0.04 s (exit status unknown)") && x.contains("stderr: boom"),
                "{x}"
            );
        }

        /// The hand-over: the job names the page and the cache, and a renderer that writes
        /// nothing is an error, not a silent success.
        #[test]
        fn a_delegated_render_hands_over_the_job() {
            let d = tempfile::tempdir().unwrap();
            let seen = d.path().join("seen.json");
            let program =
                fake(d.path(), "renderer", &format!("[ \"$1\" = browser-job ] && cp \"$2\" '{}'", seen.display()));
            let spec = card(d.path());
            let cache = d.path().join("cache");
            let job = delegate::Job::Html { spec: spec.clone(), cache_dir: cache.clone() };
            delegate::run(&program, &job).unwrap();
            let seen: delegate::Job = serde_json::from_slice(&std::fs::read(&seen).unwrap()).unwrap();
            match seen {
                delegate::Job::Html { spec: s, cache_dir } => {
                    assert_eq!((s.source, cache_dir), (spec.source, cache.clone()))
                }
                other => panic!("{other:?}"),
            }
            // the job file is gone afterwards
            assert!(std::fs::read_dir(&cache).unwrap().next().is_none());
            let failing = fake(d.path(), "broken", "echo 'no host' >&2; exit 2");
            let err = format!("{:#}", delegate::run(&failing, &job).unwrap_err());
            assert!(err.contains("no host"), "{err}");
        }
    }
    #[test]
    fn urls_roundtrip() {
        #[cfg(not(windows))]
        let (p, prefix) = (PathBuf::from("/tmp/ü dir/a b.html"), "file:///tmp/%C3%BC%20dir/");
        #[cfg(windows)]
        let (p, prefix) = (PathBuf::from(r"C:\tmp\ü dir\a b.html"), "file:///C:/tmp/%C3%BC%20dir/");
        let u = file_url(&p);
        assert!(u.starts_with(prefix), "{u}");
        assert_eq!(url_to_path(&u).unwrap(), PathBuf::from(p.to_string_lossy().replace('\\', "/")));
        assert!(url_to_path("https://x").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_verbatim_and_unc_paths() {
        assert_eq!(file_url(Path::new(r"\\?\C:\a b\x.html")), "file:///C:/a%20b/x.html");
        assert_eq!(file_url(Path::new(r"\\server\share\x.html")), "file://server/share/x.html");
        assert_eq!(file_url(Path::new(r"\\?\UNC\server\share\x.html")), "file://server/share/x.html");
        assert_eq!(url_to_path("file://server/share/x.html").unwrap(), PathBuf::from("//server/share/x.html"));
        assert!(plain_path(Path::new(r"\\?\C:\a")).starts_with(r"C:\"));
    }
}
