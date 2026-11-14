//! Browser recordings: a `.demo.json5` script drives a page in headless Chrome (go to, click,
//! type, press, scroll, wait) while the screen is captured in real time. The result is a
//! constant-frame-rate video and an events sidecar: the pointer's path, clicks, key presses,
//! and the on-screen rectangle of every element a step touched.
//!
//! Chrome draws no pointer in headless mode; the edit draws one from the sidecar, so its motion
//! is smooth whatever the capture rate was.

use anyhow::{anyhow, bail, Context, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::emulation::{
    MediaFeature, SetDeviceMetricsOverrideParams, SetEmulatedMediaParams,
};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EnableParams, EventRequestPaused, FailRequestParams, RequestPattern,
};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType, InsertTextParams,
    MouseButton,
};
use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, CaptureScreenshotParams};
use chromiumoxide::Page;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `.demo.json5` / `.demo.json` assets are browser recording scripts.
pub fn is_demo(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".demo.json5") || p.ends_with(".demo.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Viewport {
    pub w: u32,
    pub h: u32,
    /// Device pixel ratio (2 records at twice the resolution, for zooms).
    #[serde(default = "two")]
    pub scale: f64,
}
fn two() -> f64 {
    2.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct TypeStep {
    /// Field to click into first.
    #[serde(default)]
    pub into: Option<String>,
    /// Text to type. `$NAME` reads the environment variable NAME (for credentials).
    pub text: String,
    /// Milliseconds between characters (default 60).
    #[serde(default)]
    pub delay: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct ScrollStep {
    /// Scroll until this element is centred.
    #[serde(default)]
    pub to: Option<String>,
    /// Or scroll by this many CSS pixels (positive = down).
    #[serde(default)]
    pub by: Option<f64>,
    /// Seconds (default 0.8).
    #[serde(default)]
    pub duration: Option<f64>,
}

/// One step: exactly one of `goto`, `wait`, `click`, `hover`, `type`, `press`, `scroll`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goto: Option<String>,
    /// Seconds, or a selector to wait for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<TypeStep>,
    /// A key or chord: `Enter`, `Escape`, `Meta+K`, `Control+Shift+P`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll: Option<ScrollStep>,
    /// Name to refer to this step from the edit (`step: "export"`, `"step:export"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Seconds to stay after the step (default: the script's `settle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<f64>,
    /// Seconds to wait for the element (default 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
}

impl Step {
    pub fn kind(&self) -> Option<&'static str> {
        let k: Vec<&'static str> = [
            self.goto.as_ref().map(|_| "goto"),
            self.wait.as_ref().map(|_| "wait"),
            self.click.as_ref().map(|_| "click"),
            self.hover.as_ref().map(|_| "hover"),
            self.r#type.as_ref().map(|_| "type"),
            self.press.as_ref().map(|_| "press"),
            self.scroll.as_ref().map(|_| "scroll"),
        ]
        .into_iter()
        .flatten()
        .collect();
        (k.len() == 1).then(|| k[0])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DemoScript {
    pub viewport: Viewport,
    /// Base address: `http://localhost:3000`, or a path to a local page next to the script.
    #[serde(default)]
    pub url: Option<String>,
    /// `light` or `dark` (prefers-color-scheme).
    #[serde(default)]
    pub theme: Option<String>,
    /// Browser language, e.g. `de` or `fr-CA`.
    #[serde(default)]
    pub locale: Option<String>,
    /// Other origins the page may load from (`cdn.example.com`, `https://api.example.com`).
    /// The base address's origin is always allowed; everything else is blocked.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Seconds to stay after each action (default 0.6) and at the end.
    #[serde(default)]
    pub settle: Option<f64>,
    /// Frame rate of the recording (default 30).
    #[serde(default)]
    pub fps: Option<f64>,
    pub steps: Vec<Step>,
}

/// Parse and check a script.
pub fn parse(text: &str) -> Result<DemoScript> {
    let v = cutline_core::parse::parse_json5(text)
        .map_err(|d| anyhow!("{}", d.iter().map(|x| x.message.clone()).collect::<Vec<_>>().join("; ")))?;
    let s: DemoScript = serde_json::from_value(v).map_err(|e| anyhow!("demo script: {e}"))?;
    if s.viewport.w < 200 || s.viewport.h < 200 || s.viewport.w > 3840 || s.viewport.h > 3840 {
        bail!("viewport must be between 200 and 3840 px on each side");
    }
    if !(0.5..=3.0).contains(&s.viewport.scale) {
        bail!("viewport.scale must be within 0.5–3");
    }
    if s.steps.is_empty() {
        bail!("the script has no steps");
    }
    for (i, st) in s.steps.iter().enumerate() {
        if st.kind().is_none() {
            bail!("step {i}: needs exactly one of goto, wait, click, hover, type, press, scroll");
        }
    }
    let mut names = std::collections::BTreeSet::new();
    for n in s.steps.iter().filter_map(|s| s.name.as_ref()) {
        if !names.insert(n) {
            bail!("step name `{n}` is used twice");
        }
    }
    Ok(s)
}

/// A point on the pointer's path (fractions of the viewport).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CursorPoint {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub click: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepEvent {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: String,
    /// When the step began (the pointer starts moving).
    pub t0: f64,
    /// When it acted (the click, the first key, the scroll's end).
    pub t: f64,
    /// When it was done (after typing; equal to `t` for a click).
    pub t1: f64,
    /// The element's rectangle as fractions of the viewport: x, y, w, h.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<[f64; 4]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyEvent {
    pub t: f64,
    pub keys: Vec<String>,
}

/// The sidecar of a recording. Times are seconds in the video.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DemoEvents {
    /// Viewport in CSS pixels and the device pixel ratio.
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub duration: f64,
    pub cursor: Vec<CursorPoint>,
    pub steps: Vec<StepEvent>,
    pub keys: Vec<KeyEvent>,
    /// Requests refused by the origin allow-list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<String>,
}

impl DemoEvents {
    pub fn step(&self, name: &str) -> Option<&StepEvent> {
        self.steps.iter().find(|s| s.name.as_deref() == Some(name))
    }
    pub fn names(&self) -> Vec<&str> {
        self.steps.iter().filter_map(|s| s.name.as_deref()).collect()
    }
}

const FIND_JS: &str = r#"(sel) => {
  const vis = e => { const r = e.getBoundingClientRect(); const s = getComputedStyle(e);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  let el = null;
  try {
    const m = sel.match(/^(.*?):has-text\((['"])(.*)\2\)$/);
    if (sel.startsWith('text=')) {
      const want = sel.slice(5).trim();
      const all = Array.from(document.querySelectorAll('body *')).filter(e => vis(e) && e.textContent.includes(want));
      el = all.find(e => !Array.from(e.children).some(c => c.textContent.includes(want))) || null;
    } else if (m) {
      el = Array.from(document.querySelectorAll(m[1] || '*')).find(e => vis(e) && e.textContent.includes(m[3])) || null;
    } else {
      el = Array.from(document.querySelectorAll(sel)).find(vis) || null;
    }
  } catch (e) { return {error: String(e.message || e)}; }
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return {x: r.left, y: r.top, w: r.width, h: r.height,
          in_view: r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth};
}"#;

/// Smooth scroll by `dy` CSS px over `ms`, resolved when it has finished.
const SCROLL_JS: &str = r#"(dy, ms) => new Promise(done => {
  const el = document.scrollingElement || document.documentElement;
  const from = el.scrollTop, t0 = performance.now();
  const ease = u => u < 0.5 ? 4*u*u*u : 1 - Math.pow(-2*u + 2, 3) / 2;
  const tick = now => { const u = Math.min(1, (now - t0) / Math.max(1, ms));
    el.scrollTop = from + dy * ease(u); if (u < 1) requestAnimationFrame(tick); else done(el.scrollTop - from); };
  requestAnimationFrame(tick);
})"#;

const READY_JS: &str = r#"async () => {
  if (document.readyState !== 'complete') { await new Promise(r => addEventListener('load', r, {once: true})); }
  if (document.fonts && document.fonts.ready) { await document.fonts.ready; }
  await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
  return true;
}"#;

#[derive(Debug, Deserialize)]
struct Found {
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    w: f64,
    #[serde(default)]
    h: f64,
    #[serde(default)]
    in_view: bool,
    #[serde(default)]
    error: Option<String>,
}

fn ease(u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    if u < 0.5 {
        4.0 * u * u * u
    } else {
        1.0 - (-2.0 * u + 2.0).powi(3) / 2.0
    }
}

/// `scheme://host[:port]` of a URL.
pub fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| format!("{}://{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

/// Whether a request may load: same origin as the base, an allowed origin or host, a local
/// file under an allowed directory, or inline data.
pub fn request_allowed(url: &str, origins: &[String], dirs: &[PathBuf]) -> bool {
    if url.starts_with("data:") || url.starts_with("blob:") || url.starts_with("about:") {
        return true;
    }
    if url.starts_with("file://") {
        return crate::url_to_path(url)
            .and_then(|p| p.canonicalize().ok())
            .is_some_and(|p| dirs.iter().any(|d| p.starts_with(d)));
    }
    let Some(o) = origin(url) else { return false };
    let host = o.split_once("://").map(|x| x.1).unwrap_or("");
    origins.iter().any(|a| {
        let a = a.trim().trim_end_matches('/').to_ascii_lowercase();
        if a.contains("://") {
            a == o
        } else {
            a == host || a == host.split(':').next().unwrap_or("")
        }
    })
}

/// Key definition: (key, code, virtual key code, text).
fn key_def(name: &str) -> (String, String, i64, Option<String>) {
    let lower = name.to_ascii_lowercase();
    let (k, c, vk, text): (&str, &str, i64, Option<&str>) = match lower.as_str() {
        "enter" | "return" => ("Enter", "Enter", 13, Some("\r")),
        "tab" => ("Tab", "Tab", 9, None),
        "escape" | "esc" => ("Escape", "Escape", 27, None),
        "backspace" => ("Backspace", "Backspace", 8, None),
        "delete" => ("Delete", "Delete", 46, None),
        "space" => (" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => ("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => ("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => ("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => ("ArrowRight", "ArrowRight", 39, None),
        "home" => ("Home", "Home", 36, None),
        "end" => ("End", "End", 35, None),
        "pageup" => ("PageUp", "PageUp", 33, None),
        "pagedown" => ("PageDown", "PageDown", 34, None),
        _ => {
            let ch = name.chars().next().unwrap_or(' ');
            if name.chars().count() == 1 && ch.is_ascii_alphanumeric() {
                let up = ch.to_ascii_uppercase();
                let code = if ch.is_ascii_digit() { format!("Digit{up}") } else { format!("Key{up}") };
                return (ch.to_string(), code, up as i64, Some(ch.to_string()));
            }
            return (name.to_string(), name.to_string(), 0, None);
        }
    };
    (k.to_string(), c.to_string(), vk, text.map(str::to_string))
}

/// `Meta+Shift+K` → (modifier bits, modifier names, key).
pub fn chord(s: &str) -> (i64, Vec<String>, String) {
    let parts: Vec<&str> = s.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    let mut bits = 0;
    let mut mods = vec![];
    let mut key = String::new();
    for (i, p) in parts.iter().enumerate() {
        let m = match p.to_ascii_lowercase().as_str() {
            "alt" | "option" => Some((1, "alt")),
            "control" | "ctrl" => Some((2, "ctrl")),
            "meta" | "cmd" | "command" => Some((4, "cmd")),
            "shift" => Some((8, "shift")),
            _ => None,
        };
        match m {
            Some((b, n)) if i + 1 < parts.len() => {
                bits |= b;
                mods.push(n.to_string());
            }
            _ => key = p.to_string(),
        }
    }
    (bits, mods, key)
}

struct Recorder {
    page: Page,
    start: Instant,
    size: (f64, f64),
    pos: (f64, f64),
    ev: DemoEvents,
    settle: f64,
    /// Factor from CSS px to the coordinates Chrome's input takes (see `calibrate`).
    input_scale: f64,
    /// Held while a screenshot is taken. With an emulated device pixel ratio, input sent
    /// while Chrome is capturing lands at coordinates divided by the ratio; taking turns
    /// with the capture avoids that.
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl Recorder {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    async fn find(&self, sel: &str, timeout: f64, step: usize) -> Result<Found> {
        let call = format!("({FIND_JS})({})", serde_json::to_string(sel)?);
        let until = Instant::now() + Duration::from_secs_f64(timeout);
        loop {
            let v: Option<Found> = self.page.evaluate(call.as_str()).await?.into_value().unwrap_or(None);
            match v {
                Some(Found { error: Some(e), .. }) => bail!("step {step}: bad selector `{sel}`: {e}"),
                Some(f) => return Ok(f),
                None if Instant::now() >= until => {
                    bail!("step {step}: nothing matched `{sel}` within {timeout:.0} s")
                }
                None => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }

    async fn mouse(&self, kind: DispatchMouseEventType, pressed: bool) -> Result<()> {
        let mut b = DispatchMouseEventParams::builder()
            .r#type(kind.clone())
            .x(self.pos.0 * self.input_scale)
            .y(self.pos.1 * self.input_scale);
        if pressed {
            b = b.button(MouseButton::Left).buttons(1).click_count(1);
        }
        let _turn = self.gate.lock().await;
        self.page.execute(b.build().map_err(|e| anyhow!(e))?).await?;
        Ok(())
    }

    /// Find how Chrome maps dispatched mouse coordinates to the page. With an emulated device
    /// pixel ratio some builds deliver them divided by it, so a click meant for (200, 110)
    /// lands at (100, 55). One probe move tells; every later event is scaled to match.
    async fn calibrate(&mut self) -> Result<()> {
        self.page
            .evaluate(
                "window.__cutline_probe = null; addEventListener('mousemove', e => { window.__cutline_probe = e.clientX; }, {once: true, capture: true}); true",
            )
            .await?;
        let probe = 120.0_f64.min(self.size.0 / 2.0);
        let b = DispatchMouseEventParams::builder().r#type(DispatchMouseEventType::MouseMoved).x(probe).y(probe);
        {
            let _turn = self.gate.lock().await;
            self.page.execute(b.build().map_err(|e| anyhow!(e))?).await?;
        }
        for _ in 0..20 {
            let seen: Option<f64> = self.page.evaluate("window.__cutline_probe").await?.into_value().unwrap_or(None);
            if let Some(x) = seen.filter(|x| *x > 0.5) {
                let k = probe / x;
                // only the known cases: exact, or off by the device pixel ratio
                self.input_scale = if (k - 1.0).abs() < 0.05 { 1.0 } else { k.clamp(0.25, 4.0) };
                tracing::debug!(probe, seen = x, scale = self.input_scale, "mouse input calibrated");
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        tracing::debug!("mouse input calibration saw no move; assuming CSS pixels");
        Ok(())
    }

    /// Move the pointer to (x, y) in CSS px, at a human pace, dispatching moves on the way so
    /// hover states react. The path is recorded as two keys the edit eases between the same way.
    async fn move_to(&mut self, x: f64, y: f64) -> Result<()> {
        let (x0, y0) = self.pos;
        let dist = ((x - x0).powi(2) + (y - y0).powi(2)).sqrt() / self.size.0.max(self.size.1);
        if dist < 0.002 {
            return Ok(());
        }
        let secs = (0.3 + 0.75 * dist).clamp(0.3, 1.1);
        let t0 = self.now();
        self.key(t0, false);
        let begin = Instant::now();
        loop {
            let u = (begin.elapsed().as_secs_f64() / secs).min(1.0);
            let e = ease(u);
            self.pos = (x0 + (x - x0) * e, y0 + (y - y0) * e);
            self.mouse(DispatchMouseEventType::MouseMoved, false).await?;
            if u >= 1.0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(16)).await;
        }
        // the move is recorded with its planned length, so the drawn pointer arrives exactly then
        self.ev.cursor.push(CursorPoint { t: t0 + secs, x: x / self.size.0, y: y / self.size.1, click: false });
        Ok(())
    }

    fn key(&mut self, t: f64, click: bool) {
        self.ev.cursor.push(CursorPoint { t, x: self.pos.0 / self.size.0, y: self.pos.1 / self.size.1, click });
    }

    /// Bring an element into view (smooth scroll) and return its rectangle.
    async fn reveal(&self, sel: &str, timeout: f64, step: usize) -> Result<Found> {
        let f = self.find(sel, timeout, step).await?;
        if f.in_view {
            return Ok(f);
        }
        let dy = f.y + f.h / 2.0 - self.size.1 / 2.0;
        self.page.evaluate(format!("({SCROLL_JS})({dy}, 500)")).await?;
        tokio::time::sleep(Duration::from_millis(120)).await;
        self.find(sel, timeout, step).await
    }

    fn rect(&self, f: &Found) -> [f64; 4] {
        let (w, h) = self.size;
        let x0 = (f.x / w).clamp(0.0, 1.0);
        let y0 = (f.y / h).clamp(0.0, 1.0);
        [x0, y0, ((f.x + f.w) / w).clamp(0.0, 1.0) - x0, ((f.y + f.h) / h).clamp(0.0, 1.0) - y0]
    }

    async fn click_at(&mut self, f: &Found) -> Result<f64> {
        self.move_to(f.x + f.w / 2.0, f.y + f.h / 2.0).await?;
        tokio::time::sleep(Duration::from_millis(120)).await;
        let t = self.now();
        self.key(t, true);
        self.mouse(DispatchMouseEventType::MousePressed, true).await?;
        tokio::time::sleep(Duration::from_millis(60)).await;
        self.mouse(DispatchMouseEventType::MouseReleased, true).await?;
        Ok(t)
    }

    async fn press(&mut self, combo: &str) -> Result<f64> {
        let (bits, mods, key) = chord(combo);
        let (k, code, vk, text) = key_def(&key);
        let t = self.now();
        let mut names = mods;
        names.push(key.clone());
        self.ev.keys.push(KeyEvent { t, keys: names });
        // with a modifier held the key is a command, not text
        let text = text.filter(|_| bits & 0b111 == 0);
        let mut down = DispatchKeyEventParams::builder()
            .r#type(if text.is_some() { DispatchKeyEventType::KeyDown } else { DispatchKeyEventType::RawKeyDown })
            .modifiers(bits)
            .key(k.clone())
            .code(code.clone())
            .windows_virtual_key_code(vk)
            .native_virtual_key_code(vk);
        if let Some(tx) = &text {
            down = down.text(tx.clone());
        }
        self.page.execute(down.build().map_err(|e| anyhow!(e))?).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let up = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyUp)
            .modifiers(bits)
            .key(k)
            .code(code)
            .windows_virtual_key_code(vk)
            .native_virtual_key_code(vk);
        self.page.execute(up.build().map_err(|e| anyhow!(e))?).await?;
        Ok(t)
    }

    async fn run_step(&mut self, i: usize, st: &Step, base: &Base) -> Result<()> {
        let timeout = st.timeout.unwrap_or(10.0);
        let t0 = self.now();
        let mut rect = None;
        let (mut t, mut t1) = (t0, t0);
        let kind = st.kind().unwrap_or("");
        if let Some(path) = &st.goto {
            let url = base.resolve(path)?;
            tokio::time::timeout(Duration::from_secs(45), self.page.goto(url.as_str()))
                .await
                .with_context(|| format!("step {i}: loading {url} timed out"))?
                .with_context(|| format!("step {i}: loading {url}"))?;
            tokio::time::timeout(Duration::from_secs(45), self.page.evaluate_function(READY_JS))
                .await
                .with_context(|| format!("step {i}: {url} never finished loading"))??;
            t = self.now();
            t1 = t;
        } else if let Some(w) = &st.wait {
            match w {
                serde_json::Value::Number(n) => {
                    tokio::time::sleep(Duration::from_secs_f64(n.as_f64().unwrap_or(0.0).clamp(0.0, 600.0))).await
                }
                serde_json::Value::String(sel) => {
                    let f = self.find(sel, timeout, i).await?;
                    rect = Some(self.rect(&f));
                }
                _ => bail!("step {i}: wait takes seconds or a selector"),
            }
            t = self.now();
            t1 = t;
        } else if let Some(sel) = &st.click {
            let f = self.reveal(sel, timeout, i).await?;
            rect = Some(self.rect(&f));
            t = self.click_at(&f).await?;
            t1 = t;
        } else if let Some(sel) = &st.hover {
            let f = self.reveal(sel, timeout, i).await?;
            rect = Some(self.rect(&f));
            self.move_to(f.x + f.w / 2.0, f.y + f.h / 2.0).await?;
            t = self.now();
            t1 = t;
        } else if let Some(ty) = &st.r#type {
            if let Some(sel) = &ty.into {
                let f = self.reveal(sel, timeout, i).await?;
                rect = Some(self.rect(&f));
                self.click_at(&f).await?;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            let text = match ty.text.strip_prefix('$') {
                Some(var) if !var.is_empty() && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                    std::env::var(var).with_context(|| format!("step {i}: environment variable {var} is not set"))?
                }
                _ => ty.text.clone(),
            };
            t = self.now();
            let delay = Duration::from_secs_f64(ty.delay.unwrap_or(60.0).clamp(0.0, 2000.0) / 1000.0);
            for ch in text.chars() {
                self.page.execute(InsertTextParams::new(ch.to_string())).await?;
                tokio::time::sleep(delay).await;
            }
            t1 = self.now();
        } else if let Some(combo) = &st.press {
            t = self.press(combo).await?;
            t1 = t;
        } else if let Some(sc) = &st.scroll {
            let secs = sc.duration.unwrap_or(0.8).clamp(0.0, 20.0);
            let dy = match (&sc.to, sc.by) {
                (Some(sel), _) => {
                    let f = self.find(sel, timeout, i).await?;
                    f.y + f.h / 2.0 - self.size.1 / 2.0
                }
                (None, Some(by)) => by,
                (None, None) => bail!("step {i}: scroll needs `to` or `by`"),
            };
            self.page.evaluate(format!("({SCROLL_JS})({dy}, {})", secs * 1000.0)).await?;
            if let Some(sel) = &sc.to {
                let f = self.find(sel, timeout, i).await?;
                rect = Some(self.rect(&f));
            }
            t = self.now();
            t1 = t;
        }
        self.ev.steps.push(StepEvent { index: i, name: st.name.clone(), kind: kind.to_string(), t0, t, t1, rect });
        // let the result show before the next step
        let after = st.after.unwrap_or(if matches!(kind, "wait") { 0.0 } else { self.settle });
        tokio::time::sleep(Duration::from_secs_f64(after.clamp(0.0, 60.0))).await;
        Ok(())
    }
}

/// Where relative `goto` paths go.
struct Base {
    http: Option<String>,
    dir: PathBuf,
}

impl Base {
    fn resolve(&self, path: &str) -> Result<String> {
        if path.contains("://") {
            return Ok(path.to_string());
        }
        match &self.http {
            Some(b) => Ok(format!("{}/{}", b.trim_end_matches('/'), path.trim_start_matches('/'))),
            None => {
                let p = self.dir.join(path.trim_start_matches('/'));
                let p = p.canonicalize().with_context(|| format!("local page {} not found", p.display()))?;
                Ok(crate::file_url(&p))
            }
        }
    }
}

/// Result of a recording: frames on disk with their times.
struct Captured {
    frames: Vec<(PathBuf, f64)>,
    events: DemoEvents,
}

async fn record_with(exe: &Path, script: &DemoScript, script_dir: &Path, work: &Path) -> Result<Captured> {
    let profile = work.join("chrome-profile");
    let (w, h) = (script.viewport.w, script.viewport.h);
    let mut args: Vec<String> = [
        "--hide-scrollbars",
        "--mute-audio",
        "--force-color-profile=srgb",
        "--disable-background-networking",
        "--disable-extensions",
        "--disable-lcd-text",
        "--allow-file-access-from-files",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(l) = &script.locale {
        args.push(format!("--lang={l}"));
        args.push(format!("--accept-lang={l}"));
    }
    let mut cfg = BrowserConfig::builder()
        .chrome_executable(exe)
        .user_data_dir(&profile)
        .window_size(w, h)
        .viewport(None)
        .request_timeout(Duration::from_secs(60))
        .args(args);
    if cfg!(target_os = "linux") {
        cfg = cfg.no_sandbox();
    }
    let cfg = cfg.build().map_err(|e| anyhow!("chrome config: {e}"))?;
    let (mut browser, mut handler) =
        Browser::launch(cfg).await.with_context(|| format!("launching {} failed", exe.display()))?;
    let hj = tokio::spawn(async move { while let Some(_ev) = handler.next().await {} });
    let res = drive(&browser, script, script_dir, work).await;
    let _ = browser.close().await;
    let _ = browser.wait().await;
    hj.abort();
    let _ = std::fs::remove_dir_all(&profile);
    res
}

async fn drive(browser: &Browser, script: &DemoScript, script_dir: &Path, work: &Path) -> Result<Captured> {
    let page = browser.new_page("about:blank").await?;
    let (w, h) = (script.viewport.w, script.viewport.h);
    page.execute(SetDeviceMetricsOverrideParams::new(w as i64, h as i64, script.viewport.scale, false)).await?;
    // a headless page has no window focus; without this, typed text goes nowhere
    page.execute(chromiumoxide::cdp::browser_protocol::emulation::SetFocusEmulationEnabledParams::new(true)).await?;
    if let Some(theme) = &script.theme {
        let f = MediaFeature::new("prefers-color-scheme", theme.as_str());
        page.execute(SetEmulatedMediaParams { media: None, features: Some(vec![f]) }).await?;
    }
    // the base address: a web origin, or a local page next to the script
    let http = script.url.as_ref().filter(|u| u.contains("://") && !u.starts_with("file://")).cloned();
    let base = Base { http: http.clone(), dir: script_dir.to_path_buf() };
    let mut origins: Vec<String> = script.allow.clone();
    if let Some(o) = http.as_deref().and_then(origin) {
        origins.push(o);
    }
    let dirs: Vec<PathBuf> = script_dir.canonicalize().into_iter().collect();
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
    let (p2, b2) = (page.clone(), blocked.clone());
    let guard = tokio::spawn(async move {
        while let Some(ev) = paused.next().await {
            let url = ev.request.url.clone();
            if request_allowed(&url, &origins, &dirs) {
                let _ = p2.execute(ContinueRequestParams::new(ev.request_id.clone())).await;
            } else {
                b2.lock().unwrap().push(url);
                let _ = p2.execute(FailRequestParams::new(ev.request_id.clone(), ErrorReason::BlockedByClient)).await;
            }
        }
    });

    let mut steps = script.steps.iter().enumerate().peekable();
    let mut rec = Recorder {
        page: page.clone(),
        start: Instant::now(),
        size: (w as f64, h as f64),
        pos: (w as f64 / 2.0, h as f64 / 2.0),
        ev: DemoEvents { width: w, height: h, scale: script.viewport.scale, ..Default::default() },
        settle: script.settle.unwrap_or(0.6),
        input_scale: 1.0,
        gate: Arc::new(tokio::sync::Mutex::new(())),
    };
    // open the page before the recording starts: no blank frames at the head
    if let Some(u) = script.url.as_ref().filter(|_| !matches!(steps.peek(), Some((_, s)) if s.goto.is_some())) {
        let first = Step { goto: Some(if http.is_some() { "/".into() } else { u.clone() }), ..Default::default() };
        rec.run_step(usize::MAX, &first, &base).await.map_err(|e| anyhow!("opening {u}: {e:#}"))?;
        rec.ev.steps.clear();
    } else if let Some((i, s)) = steps.next_if(|(_, s)| s.goto.is_some()) {
        let mut s0 = s.clone();
        s0.after = Some(0.0);
        rec.run_step(i, &s0, &base).await?;
        rec.ev.steps.clear();
        rec.ev.steps.push(StepEvent {
            index: i,
            name: s.name.clone(),
            kind: "goto".into(),
            t0: 0.0,
            t: 0.0,
            t1: 0.0,
            rect: None,
        });
    }

    // ---- capture: screenshots as fast as Chrome gives them, each stamped with its time
    let fps_cap = script.fps.unwrap_or(30.0).clamp(1.0, 60.0);
    let stop = Arc::new(AtomicBool::new(false));
    let frames: Arc<Mutex<Vec<(PathBuf, f64)>>> = Arc::new(Mutex::new(vec![]));
    rec.start = Instant::now();
    rec.key(0.0, false);
    let cap = {
        let gate = rec.gate.clone();
        let (page, stop, frames, start, work) =
            (page.clone(), stop.clone(), frames.clone(), rec.start, work.to_path_buf());
        tokio::spawn(async move {
            let mut n = 0usize;
            let mut err = None;
            while !stop.load(Ordering::Relaxed) {
                let begun = Instant::now();
                let shot = CaptureScreenshotParams {
                    format: Some(CaptureScreenshotFormat::Jpeg),
                    quality: Some(95),
                    from_surface: Some(true),
                    capture_beyond_viewport: Some(false),
                    optimize_for_speed: Some(true),
                    ..Default::default()
                };
                let turn = gate.lock().await;
                let asked = start.elapsed().as_secs_f64();
                let got = tokio::time::timeout(Duration::from_secs(20), page.screenshot(shot)).await;
                drop(turn);
                match got {
                    Ok(Ok(bytes)) => {
                        // the picture shows the page somewhere between asking and getting it
                        let t = (asked + start.elapsed().as_secs_f64()) / 2.0;
                        let p = work.join(format!("f_{n:06}.jpg"));
                        if let Err(e) = std::fs::write(&p, bytes) {
                            err = Some(anyhow!(e));
                            break;
                        }
                        frames.lock().unwrap().push((p, t));
                        n += 1;
                    }
                    Ok(Err(e)) => {
                        err = Some(anyhow!("capture failed: {e}"));
                        break;
                    }
                    Err(_) => {
                        err = Some(anyhow!("Chrome produced no frame for 20 s"));
                        break;
                    }
                }
                let spent = begun.elapsed().as_secs_f64();
                if spent < 1.0 / fps_cap {
                    tokio::time::sleep(Duration::from_secs_f64(1.0 / fps_cap - spent)).await;
                }
            }
            err
        })
    };
    // a first frame before anything moves; then see how input maps once capture is running
    tokio::time::sleep(Duration::from_millis(250)).await;
    rec.calibrate().await?;
    let mut failure = None;
    for (i, st) in steps {
        if let Err(e) = rec.run_step(i, st, &base).await {
            failure = Some(e);
            break;
        }
    }
    if failure.is_none() {
        tokio::time::sleep(Duration::from_secs_f64(rec.settle.max(0.3))).await;
    }
    let duration = rec.now();
    stop.store(true, Ordering::Relaxed);
    let cap_err = cap.await.ok().flatten();
    guard.abort();
    if let Some(e) = failure {
        return Err(e);
    }
    if let Some(e) = cap_err {
        return Err(e);
    }
    let frames = std::mem::take(&mut *frames.lock().unwrap());
    if frames.is_empty() {
        bail!("Chrome produced no frames");
    }
    let mut events = rec.ev;
    events.duration = duration;
    // milliseconds and 1/100 px are plenty, and round-trip through JSON unchanged
    let ms = |v: &mut f64| *v = (*v * 1e4).round() / 1e4;
    ms(&mut events.duration);
    for c in &mut events.cursor {
        ms(&mut c.t);
        c.x = (c.x * 1e5).round() / 1e5;
        c.y = (c.y * 1e5).round() / 1e5;
    }
    for st in &mut events.steps {
        ms(&mut st.t0);
        ms(&mut st.t);
        ms(&mut st.t1);
        if let Some(r) = &mut st.rect {
            for v in r.iter_mut() {
                *v = (*v * 1e5).round() / 1e5;
            }
        }
    }
    for k in &mut events.keys {
        ms(&mut k.t);
    }
    events.blocked = blocked.lock().unwrap().clone();
    Ok(Captured { frames, events })
}

/// Encode captured frames (each shown from its time until the next one's) to a
/// constant-frame-rate H.264 4:4:4 video.
fn encode(tc: &cutline_deps::Toolchain, cap: &Captured, fps: f64, work: &Path, out: &Path) -> Result<()> {
    let mut list = String::from("ffconcat version 1.0\n");
    for (i, (p, t)) in cap.frames.iter().enumerate() {
        let next = cap.frames.get(i + 1).map(|f| f.1).unwrap_or(cap.events.duration);
        let start = if i == 0 { 0.0 } else { *t };
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        list.push_str(&format!("file '{name}'\nduration {:.6}\n", (next - start).max(0.001)));
    }
    // the concat demuxer needs the last file again to honour its duration
    if let Some((p, _)) = cap.frames.last() {
        list.push_str(&format!("file '{}'\n", p.file_name().and_then(|n| n.to_str()).unwrap_or_default()));
    }
    let lp = work.join("frames.ffconcat");
    std::fs::write(&lp, list)?;
    let frames = (cap.events.duration * fps).round().max(1.0) as i64;
    let o = tc
        .ffmpeg_cmd()
        .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"])
        .arg(&lp)
        .args([
            "-vf",
            &format!("fps={fps},scale=trunc(iw/2)*2:trunc(ih/2)*2:out_color_matrix=bt709:out_range=tv,format=yuv444p"),
            "-frames:v",
            &frames.to_string(),
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            "12",
            "-tune",
            "animation",
            "-pix_fmt",
            "yuv444p",
        ])
        .args(["-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-color_range", "tv"])
        .args(["-movflags", "+faststart", "-map_metadata", "-1"])
        .arg(out)
        .output()?;
    if !o.status.success() {
        bail!("encoding the recording failed: {}", String::from_utf8_lossy(&o.stderr));
    }
    Ok(())
}

/// A finished recording.
#[derive(Debug, Clone)]
pub struct Recording {
    pub video: PathBuf,
    pub events_path: PathBuf,
    pub events: DemoEvents,
}

/// Cache key: the script, the local page files beside it (for local pages) and the browser.
fn cache_key(script_path: &Path, text: &str, script: &DemoScript) -> String {
    let mut parts: Vec<Vec<u8>> = vec![text.as_bytes().to_vec()];
    let local = !script.url.as_ref().is_some_and(|u| u.contains("://") && !u.starts_with("file://"));
    if let (true, Some(dir)) = (local, script_path.parent()) {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && p != script_path).collect())
            .unwrap_or_default();
        files.sort();
        for f in files.iter().take(64) {
            if let Ok(fp) = cutline_media::fingerprint(f) {
                parts.push(fp.into_bytes());
            }
        }
    }
    parts.push(crate::chrome().map(|p| p.to_string_lossy().to_string()).unwrap_or_default().into_bytes());
    parts.push(b"demo-v5".to_vec());
    let refs: Vec<&[u8]> = parts.iter().map(|v| v.as_slice()).collect();
    cutline_media::hash_key(&refs)
}

/// Read a script; `locale` (from the project's `--locale`) replaces the script's own.
fn read_script(script_path: &Path, locale: Option<&str>) -> Result<(String, DemoScript)> {
    let text =
        std::fs::read_to_string(script_path).with_context(|| format!("cannot read {}", script_path.display()))?;
    let mut script = parse(&text)?;
    match locale {
        Some(l) => {
            script.locale = Some(l.to_string());
            // each language is its own recording
            Ok((format!("{text}\nlocale={l}"), script))
        }
        None => Ok((text, script)),
    }
}

/// The cached recording of a script, if there is one.
pub fn cached(script_path: &Path, cache_dir: &Path, locale: Option<&str>) -> Result<Option<Recording>> {
    let (text, script) = read_script(script_path, locale)?;
    let key = cache_key(script_path, &text, &script);
    let (video, events_path) = (cache_dir.join(format!("{key}.mp4")), cache_dir.join(format!("{key}.events.json")));
    if !(video.is_file() && events_path.is_file()) {
        return Ok(None);
    }
    let events = serde_json::from_slice(&std::fs::read(&events_path)?)?;
    Ok(Some(Recording { video, events_path, events }))
}

/// Record a script into the cache (or reuse the cached recording unless `force`).
pub fn record_cached(
    tc: &cutline_deps::Toolchain,
    script_path: &Path,
    cache_dir: &Path,
    force: bool,
    locale: Option<&str>,
) -> Result<Recording> {
    if !force {
        if let Some(r) = cached(script_path, cache_dir, locale)? {
            return Ok(r);
        }
    }
    let (text, script) = read_script(script_path, locale)?;
    let key = cache_key(script_path, &text, &script);
    let dir = script_path.parent().unwrap_or(Path::new("."));
    let work = cache_dir.join(format!("{key}.work"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let cands = cutline_deps::paths::chrome_candidates();
    if cands.is_empty() {
        bail!("no Chrome/Chromium/Edge found (run `cutline deps install chrome` or set CUTLINE_CHROME)");
    }
    tracing::info!("recording browser demo {}", script_path.display());
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let mut last = None;
    let mut cap = None;
    for exe in &cands {
        match rt.block_on(record_with(exe, &script, dir, &work)) {
            Ok(c) => {
                cap = Some(c);
                break;
            }
            // a step that failed is the script's problem, not the browser's
            Err(e) if format!("{e:#}").contains("step ") => {
                let _ = std::fs::remove_dir_all(&work);
                return Err(e);
            }
            Err(e) => {
                tracing::warn!("{}: {e:#}", exe.display());
                last = Some(e);
            }
        }
    }
    let Some(cap) = cap else {
        let _ = std::fs::remove_dir_all(&work);
        return Err(last.unwrap_or_else(|| anyhow!("recording failed")));
    };
    let fps = script.fps.unwrap_or(30.0).clamp(1.0, 60.0);
    let (video, events_path) = (cache_dir.join(format!("{key}.mp4")), cache_dir.join(format!("{key}.events.json")));
    let tmp = cache_dir.join(format!("{key}.tmp.mp4"));
    encode(tc, &cap, fps, &work, &tmp)?;
    std::fs::write(&events_path, serde_json::to_vec_pretty(&cap.events)?)?;
    std::fs::rename(&tmp, &video)?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(Recording { video, events_path, events: cap.events })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_checked() {
        let ok = parse("{viewport: {w: 1280, h: 800}, url: 'http://localhost:3000', steps: [{goto: '/'}, {click: '#go', name: 'go'}]}")
            .unwrap();
        assert_eq!(ok.viewport.scale, 2.0);
        assert_eq!(ok.steps[1].kind(), Some("click"));
        assert!(parse("{viewport: {w: 1280, h: 800}, steps: []}").unwrap_err().to_string().contains("no steps"));
        let e = parse("{viewport: {w: 1280, h: 800}, steps: [{click: '#a', press: 'Enter'}]}").unwrap_err();
        assert!(e.to_string().contains("exactly one"), "{e}");
        let e = parse("{viewport: {w: 1280, h: 800}, steps: [{click: '#a', name: 'x'}, {hover: '#b', name: 'x'}]}")
            .unwrap_err();
        assert!(e.to_string().contains("used twice"), "{e}");
        assert!(parse("{viewport: {w: 1280, h: 800}, steps: [{clik: '#a'}]}").is_err());
        assert!(is_demo("rec/Onboarding.demo.json5") && !is_demo("demo.json5"));
    }

    #[test]
    fn only_listed_origins_load() {
        let origins = vec!["http://localhost:3000".to_string(), "cdn.example.com".to_string()];
        assert!(request_allowed("http://localhost:3000/app.js", &origins, &[]));
        assert!(request_allowed("https://cdn.example.com/x.woff2", &origins, &[]));
        assert!(request_allowed("data:image/png;base64,AAAA", &origins, &[]));
        assert!(!request_allowed("http://localhost:4000/app.js", &origins, &[]));
        assert!(!request_allowed("https://tracker.example.net/p.gif", &origins, &[]));
        assert!(!request_allowed("file:///etc/passwd", &origins, &[]));
    }

    #[test]
    fn chords_split_into_modifiers_and_key() {
        assert_eq!(chord("Meta+Shift+K"), (12, vec!["cmd".to_string(), "shift".to_string()], "K".to_string()));
        assert_eq!(chord("Enter"), (0, vec![], "Enter".to_string()));
        assert_eq!(key_def("Enter").2, 13);
        assert_eq!(key_def("k"), ("k".to_string(), "KeyK".to_string(), 75, Some("k".to_string())));
    }
}
