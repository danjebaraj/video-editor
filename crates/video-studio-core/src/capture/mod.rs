//! Driving and recording apps during a production run (`video-studio capture …`).
//!
//! When a spec asks for footage that doesn't exist yet — a walkthrough of a desktop app, a web
//! app in a browser — Claude Code makes it during the run: it opens the app, looks at
//! screenshots, clicks, types and uses menus, and records the screen. cutline plays no part in
//! this; it only edits the recordings afterwards.
//!
//! Claude never gets a general shell for this. Every action goes through the helper, which
//! - works only inside a run whose project has app recording turned on,
//! - acts only on the app it was pointed at, and refuses apps that would hand over the machine
//!   (terminals, System Settings, password managers, Finder) unless the project lists them,
//! - writes only under the run folder's `captures/`.
//!
//! The platform part (events, windows, screen recording) is behind [`Backend`]; macOS is
//! implemented, and a fake backend stands in for it in tests.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
pub mod windows;

/// The container recordings are written in: what the system recorder makes (QuickTime on macOS,
/// H.264 MP4 from Media Foundation on Windows).
pub const RECORDING_EXT: &str = if cfg!(windows) { "mp4" } else { "mov" };

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{paths, Error, Result};

/// Names a file the fake backend logs its actions to; set only by tests.
pub const FAKE_ENV: &str = "VIDEO_STUDIO_CAPTURE_FAKE";
/// Folder in the run folder that screenshots and recordings go to.
pub const CAPTURES_DIR: &str = "captures";
/// Longest single recording, in seconds.
pub const MAX_RECORDING_S: u32 = 600;
/// Longest one `record wait` call blocks, so it stays inside a tool call's time limit.
const WAIT_SLICE_S: u64 = 90;

/// Apps a run may never drive unless the project lists them by name: each would turn "click and
/// type" into running commands, reading secrets or deleting files.
const GUARDED_APPS: [&str; 22] = [
    "Terminal",
    "iTerm",
    "iTerm2",
    "Warp",
    "kitty",
    "Alacritty",
    "Ghostty",
    "WezTerm",
    "Hyper",
    "Script Editor",
    "Automator",
    "Shortcuts",
    "System Settings",
    "System Preferences",
    "Keychain Access",
    "Passwords",
    "1Password",
    "Bitwarden",
    "Activity Monitor",
    "Console",
    "Disk Utility",
    "Finder",
];

/// The Windows apps a run may never drive unless the project lists them (ui/spec.md §23.7):
/// terminals, Settings, Registry Editor, Task Manager, password managers, File Explorer and
/// the UAC prompt. Matched against window owners' executable names and display names.
#[cfg(windows)]
pub const GUARDED_APPS_WINDOWS: [&str; 30] = [
    "cmd",
    "Command Prompt",
    "powershell",
    "Windows PowerShell",
    "pwsh",
    "PowerShell",
    "WindowsTerminal",
    "Windows Terminal",
    "wt",
    "conhost",
    "OpenConsole",
    "SystemSettings",
    "Settings",
    "control",
    "Control Panel",
    "regedit",
    "Registry Editor",
    "Taskmgr",
    "Task Manager",
    "mmc",
    "explorer",
    "File Explorer",
    "consent",
    "KeePass",
    "KeePassXC",
    "1Password",
    "Bitwarden",
    "Dashlane",
    "LastPass",
    "CredentialUIBroker",
];

fn is_guarded(app: &str) -> bool {
    let name = app.trim().trim_end_matches(".app");
    #[cfg(windows)]
    {
        let stem = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name);
        if GUARDED_APPS_WINDOWS.iter().any(|g| g.eq_ignore_ascii_case(stem)) {
            return true;
        }
    }
    GUARDED_APPS.iter().any(|g| g.eq_ignore_ascii_case(name))
}

/// What the run's project allows, passed to the helper in its run context.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureSettings {
    /// When not empty, the only apps the run may drive.
    #[serde(default)]
    pub apps: Vec<String>,
}

fn same_app(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    let strip = |s: &str| s.trim_end_matches(".app").to_string();
    let (a, b) = (strip(&a), strip(&b));
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

impl CaptureSettings {
    /// Whether the run may drive `app`.
    pub fn check_app(&self, app: &str) -> Result<()> {
        let listed = self.apps.iter().any(|a| same_app(a, app));
        if listed {
            return Ok(());
        }
        if !self.apps.is_empty() {
            return Err(Error::Denied(format!(
                "\"{app}\" isn't one of the apps this project lets a run drive ({}). The user can add it under Project › App recording.",
                self.apps.join(", ")
            )));
        }
        if is_guarded(app) {
            return Err(Error::Denied(format!(
                "\"{app}\" can't be driven during a run unless the user lists it under Project › App recording."
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= 0.0 && y >= 0.0 && x <= self.w && y <= self.h
    }
}

/// What the operating system lets the app (or the terminal running the CLI) do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    /// Screenshots and screen recordings.
    pub screen_recording: bool,
    /// Clicking, typing, menus and window sizing.
    pub accessibility: bool,
    /// False where driving apps isn't implemented yet.
    pub supported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pointer {
    Move {
        x: f64,
        y: f64,
    },
    Click {
        x: f64,
        y: f64,
        right: bool,
        count: u8,
    },
    Drag {
        x: f64,
        y: f64,
        to_x: f64,
        to_y: f64,
    },
    /// `over`: spread the lines evenly over that many seconds (`--over`), else all at once.
    Scroll {
        x: f64,
        y: f64,
        lines: i32,
        over: Option<f64>,
    },
}

/// Shortest and longest `--over` for a timed scroll, in seconds.
pub const SCROLL_OVER_RANGE: std::ops::RangeInclusive<f64> = 0.1..=60.0;

/// A timed scroll as steps: the delta of each step in the platform's fine unit (`unit` per line:
/// pixels on macOS, 1/120 of a notch on Windows) and the pause between steps. At least one step
/// per line and at least 10 steps a second, evenly spaced; the deltas add up to exactly
/// `lines × unit`, so the whole movement happens.
pub fn scroll_plan(lines: i32, over: f64, unit: i32) -> (Vec<i32>, Duration) {
    let total = lines as i64 * unit as i64;
    let n = (lines.unsigned_abs() as u64).max((over * 10.0).ceil() as u64).max(1);
    let steps: Vec<i32> = (1..=n as i64).map(|k| (total * k / n as i64 - total * (k - 1) / n as i64) as i32).collect();
    (steps, Duration::from_secs_f64(over / n as f64))
}

/// A key with modifiers, such as `cmd+shift+n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    pub command: bool,
    pub shift: bool,
    pub control: bool,
    pub option: bool,
    /// Lower-case key name from [`KEY_NAMES`].
    pub key: String,
}

/// Keys `capture key` understands besides single letters, digits and punctuation.
pub const KEY_NAMES: [&str; 28] = [
    "return",
    "tab",
    "space",
    "delete",
    "forwarddelete",
    "escape",
    "left",
    "right",
    "up",
    "down",
    "home",
    "end",
    "pageup",
    "pagedown",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "enter",
    "esc",
];

impl KeyCombo {
    pub fn parse(s: &str) -> Result<KeyCombo> {
        let mut c = KeyCombo { command: false, shift: false, control: false, option: false, key: String::new() };
        // A trailing "+" is the plus key itself: "cmd++".
        let (body, plus) = match s.strip_suffix("++") {
            Some(b) => (b, true),
            None if s == "+" => ("", true),
            None => (s, false),
        };
        let mut parts: Vec<&str> = body.split('+').filter(|p| !p.is_empty()).collect();
        if plus {
            parts.push("+");
        }
        for (i, part) in parts.iter().enumerate() {
            let p = part.trim().to_lowercase();
            let last = i + 1 == parts.len();
            match p.as_str() {
                "cmd" | "command" | "meta" | "super" | "win" if !last => c.command = true,
                "shift" if !last => c.shift = true,
                "ctrl" | "control" if !last => c.control = true,
                "alt" | "option" | "opt" if !last => c.option = true,
                _ if last => {
                    let known = KEY_NAMES.contains(&p.as_str())
                        || (p.chars().count() == 1 && p.chars().all(|ch| ch.is_ascii_graphic()));
                    if !known {
                        return Err(Error::other(format!(
                            "\"{part}\" isn't a key. Use a letter, digit or punctuation mark, or one of: {}.",
                            KEY_NAMES[..26].join(", ")
                        )));
                    }
                    c.key = match p.as_str() {
                        "enter" => "return".into(),
                        "esc" => "escape".into(),
                        _ => p,
                    };
                }
                _ => return Err(Error::other(format!("\"{part}\" isn't a modifier (cmd, shift, ctrl, alt)."))),
            }
        }
        if c.key.is_empty() {
            return Err(Error::other("Name a key, for example `cmd+n`, `return` or `shift+tab`."));
        }
        Ok(c)
    }
}

/// The platform part: everything that touches other apps or the screen. Coordinates are screen
/// points.
pub trait Backend {
    fn permissions(&self) -> Permissions;
    /// Asks the operating system to show its permission prompts.
    fn request_permissions(&self) -> Permissions {
        self.permissions()
    }
    /// Name of the app in front.
    fn frontmost(&self) -> Result<String>;
    /// Launches the app if needed and brings it to the front.
    fn activate(&self, app: &str) -> Result<()>;
    fn open_url(&self, url: &str, app: Option<&str>) -> Result<()>;
    /// The front window of the app in front.
    fn window(&self) -> Result<Rect>;
    fn set_window(&self, size: Option<(f64, f64)>, at: Option<(f64, f64)>) -> Result<Rect>;
    /// The main display.
    fn screen(&self) -> Result<Rect>;
    /// Saves a PNG of `rect`, one pixel per point.
    fn screenshot(&self, rect: Rect, out: &Utf8Path) -> Result<()>;
    fn pointer(&self, action: Pointer) -> Result<()>;
    fn type_text(&self, text: &str) -> Result<()>;
    fn key(&self, combo: &KeyCombo) -> Result<()>;
    /// Chooses a menu item of the app in front: `["File", "New Project…"]`.
    fn menu(&self, path: &[String]) -> Result<()>;
    /// Starts recording `rect` to `out` for `seconds`; returns the recorder's process ID. The
    /// recorder ends by itself and only then is the file complete.
    fn record_start(&self, rect: Rect, out: &Utf8Path, seconds: u32, show_clicks: bool) -> Result<u32>;
    fn record_running(&self, pid: u32) -> bool;
    /// Ends the recorder now; the file is lost.
    fn record_cancel(&self, pid: u32);
    /// ffmpeg, for rewriting finished recordings at a constant frame rate.
    fn ffmpeg(&self) -> Option<Utf8PathBuf> {
        ffmpeg()
    }
}

/// ffmpeg as cutline installs it: `CUTLINE_FFMPEG` if set, else `cutline deps path ffmpeg` for
/// the configured cutline.
pub fn ffmpeg() -> Option<Utf8PathBuf> {
    if let Some(p) = std::env::var("CUTLINE_FFMPEG").ok().filter(|v| !v.is_empty()).map(Utf8PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let settings = crate::settings::Settings::load(&crate::paths::library_dir());
    let cutline = crate::eleven::HelperContext::from_env().ok().and_then(|c| c.cutline).or_else(|| {
        crate::claude::locate::find_cutline(&settings.cutline_path, &settings.cutline_repo).map(|c| c.path.into())
    })?;
    let (code, out, _) = crate::platform::run_capture(
        std::process::Command::new(cutline.as_str()).args(["deps", "path", "ffmpeg"]),
        Duration::from_secs(20),
    )
    .ok()?;
    let p = Utf8PathBuf::from(out.trim());
    (code == 0 && p.is_file()).then_some(p)
}

/// The frame rate recordings are rewritten at.
pub const RECORDING_FPS: u32 = 30;

/// Rewrites a finished recording at a constant 30 fps (TASK-honest-cedar-A73E). The system
/// recorders store a frame only when the picture changes, so a 7 s capture can hold 25 frames;
/// seeks, crops and frame grabs on such a file land between frames. Writes a temporary file next
/// to `path` and renames it over `path` only when ffmpeg succeeds; otherwise `path` is untouched.
pub fn rewrite_cfr(ffmpeg: &Utf8Path, path: &Utf8Path) -> Result<()> {
    let ext = path.extension().unwrap_or(RECORDING_EXT);
    let tmp = Utf8PathBuf::from(format!("{}.cfr-tmp.{ext}", path.as_str().trim_end_matches(&format!(".{ext}"))));
    let _ = std::fs::remove_file(&tmp);
    let (code, _, err) = crate::platform::run_capture(
        std::process::Command::new(ffmpeg.as_str())
            .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i", path.as_str()])
            .args(["-map", "0:v:0", "-map", "0:a?", "-vf", &format!("fps={RECORDING_FPS}")])
            .args(["-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p", "-c:a", "copy"])
            .arg(tmp.as_str()),
        Duration::from_secs(600),
    )?;
    let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if code != 0 || size == 0 {
        let _ = std::fs::remove_file(&tmp);
        let why = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("ffmpeg failed").trim().to_string();
        return Err(Error::other(why));
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::from(e)
    })
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
struct Unsupported;

impl Unsupported {
    fn err<T>() -> Result<T> {
        Err(Error::Environment("Driving and recording apps isn't available on this platform yet.".into()))
    }
}

impl Backend for Unsupported {
    fn permissions(&self) -> Permissions {
        Permissions::default()
    }
    fn frontmost(&self) -> Result<String> {
        Self::err()
    }
    fn activate(&self, _: &str) -> Result<()> {
        Self::err()
    }
    fn open_url(&self, _: &str, _: Option<&str>) -> Result<()> {
        Self::err()
    }
    fn window(&self) -> Result<Rect> {
        Self::err()
    }
    fn set_window(&self, _: Option<(f64, f64)>, _: Option<(f64, f64)>) -> Result<Rect> {
        Self::err()
    }
    fn screen(&self) -> Result<Rect> {
        Self::err()
    }
    fn screenshot(&self, _: Rect, _: &Utf8Path) -> Result<()> {
        Self::err()
    }
    fn pointer(&self, _: Pointer) -> Result<()> {
        Self::err()
    }
    fn type_text(&self, _: &str) -> Result<()> {
        Self::err()
    }
    fn key(&self, _: &KeyCombo) -> Result<()> {
        Self::err()
    }
    fn menu(&self, _: &[String]) -> Result<()> {
        Self::err()
    }
    fn record_start(&self, _: Rect, _: &Utf8Path, _: u32, _: bool) -> Result<u32> {
        Self::err()
    }
    fn record_running(&self, _: u32) -> bool {
        false
    }
    fn record_cancel(&self, _: u32) {}
}

/// A backend that touches nothing: it logs each action as a JSON line and writes placeholder
/// files. The app in front is whatever was last activated.
pub struct FakeBackend {
    log: Utf8PathBuf,
    /// A real video copied as the "recording" ([`FAKE_RECORDING_ENV`]); placeholder bytes
    /// otherwise.
    recording: Option<Utf8PathBuf>,
    /// ffmpeg for the rewrite of a real recording (`CUTLINE_FFMPEG`).
    ffmpeg: Option<Utf8PathBuf>,
    /// The permissions it reports, normally both granted.
    granted: Permissions,
}

/// Names a video the fake backend hands back as its recording; set only by tests.
pub const FAKE_RECORDING_ENV: &str = "VIDEO_STUDIO_CAPTURE_FAKE_RECORDING";

impl FakeBackend {
    pub fn new(log: impl Into<Utf8PathBuf>) -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty()).map(Utf8PathBuf::from);
        FakeBackend {
            log: log.into(),
            recording: env(FAKE_RECORDING_ENV),
            ffmpeg: env("CUTLINE_FFMPEG"),
            granted: Permissions { screen_recording: true, accessibility: true, supported: true },
        }
    }
    /// Reports these permissions instead.
    pub fn with_permissions(mut self, screen_recording: bool, accessibility: bool) -> Self {
        self.granted = Permissions { screen_recording, accessibility, supported: true };
        self
    }
    /// The fake recorder copies `video` instead of writing placeholder bytes, and `record wait`
    /// rewrites it with `ffmpeg`.
    pub fn with_recording(mut self, video: impl Into<Utf8PathBuf>, ffmpeg: Option<Utf8PathBuf>) -> Self {
        self.recording = Some(video.into());
        self.ffmpeg = ffmpeg;
        self
    }
    fn note(&self, v: serde_json::Value) {
        crate::claude::broker::append_line(&self.log, &v.to_string());
    }
    fn front_file(&self) -> Utf8PathBuf {
        Utf8PathBuf::from(format!("{}.front", self.log))
    }
}

impl Backend for FakeBackend {
    fn permissions(&self) -> Permissions {
        self.granted
    }
    fn frontmost(&self) -> Result<String> {
        Ok(std::fs::read_to_string(self.front_file()).unwrap_or_else(|_| "Finder".into()))
    }
    fn activate(&self, app: &str) -> Result<()> {
        self.note(json!({"do": "activate", "app": app}));
        Ok(std::fs::write(self.front_file(), app)?)
    }
    fn open_url(&self, url: &str, app: Option<&str>) -> Result<()> {
        self.note(json!({"do": "open_url", "url": url, "app": app}));
        Ok(std::fs::write(self.front_file(), app.unwrap_or("Safari"))?)
    }
    fn window(&self) -> Result<Rect> {
        Ok(Rect { x: 100.0, y: 80.0, w: 1280.0, h: 800.0 })
    }
    fn set_window(&self, size: Option<(f64, f64)>, at: Option<(f64, f64)>) -> Result<Rect> {
        self.note(json!({"do": "set_window", "size": size, "at": at}));
        let (w, h) = size.unwrap_or((1280.0, 800.0));
        // like Safari, the fake app's windows are at least 574 points wide
        let w = w.max(574.0);
        let (x, y) = at.unwrap_or((100.0, 80.0));
        Ok(Rect { x, y, w, h })
    }
    fn screen(&self) -> Result<Rect> {
        Ok(Rect { x: 0.0, y: 0.0, w: 1728.0, h: 1117.0 })
    }
    fn screenshot(&self, rect: Rect, out: &Utf8Path) -> Result<()> {
        self.note(json!({"do": "screenshot", "rect": rect, "out": out}));
        Ok(std::fs::write(out, b"\x89PNG\r\n\x1a\nfake")?)
    }
    fn pointer(&self, action: Pointer) -> Result<()> {
        self.note(match action {
            Pointer::Move { x, y } => json!({"do": "move", "x": x, "y": y}),
            Pointer::Click { x, y, right, count } => {
                json!({"do": "click", "x": x, "y": y, "right": right, "count": count})
            }
            Pointer::Drag { x, y, to_x, to_y } => json!({"do": "drag", "x": x, "y": y, "to_x": to_x, "to_y": to_y}),
            Pointer::Scroll { x, y, lines, over } => {
                json!({"do": "scroll", "x": x, "y": y, "lines": lines, "over": over})
            }
        });
        Ok(())
    }
    fn type_text(&self, text: &str) -> Result<()> {
        self.note(json!({"do": "type", "text": text}));
        Ok(())
    }
    fn key(&self, c: &KeyCombo) -> Result<()> {
        self.note(
            json!({"do": "key", "key": c.key, "cmd": c.command, "shift": c.shift, "ctrl": c.control, "alt": c.option}),
        );
        Ok(())
    }
    fn menu(&self, path: &[String]) -> Result<()> {
        self.note(json!({"do": "menu", "path": path}));
        Ok(())
    }
    fn record_start(&self, rect: Rect, out: &Utf8Path, seconds: u32, show_clicks: bool) -> Result<u32> {
        self.note(json!({"do": "record_start", "rect": rect, "out": out, "seconds": seconds, "clicks": show_clicks}));
        match &self.recording {
            Some(src) => {
                std::fs::copy(src, out)?;
            }
            None => std::fs::write(out, b"fake recording")?,
        }
        Ok(0)
    }
    fn record_running(&self, _: u32) -> bool {
        false
    }
    fn record_cancel(&self, pid: u32) {
        self.note(json!({"do": "record_cancel", "pid": pid}));
    }
    /// Only with a real recording to rewrite, and never by asking a cutline for its ffmpeg.
    fn ffmpeg(&self) -> Option<Utf8PathBuf> {
        self.recording.as_ref()?;
        self.ffmpeg.clone()
    }
}

/// The backend for this machine: the fake one when [`FAKE_ENV`] is set, else the platform's.
pub fn backend() -> Box<dyn Backend> {
    if let Some(log) = std::env::var(FAKE_ENV).ok().filter(|v| !v.is_empty()) {
        return Box::new(FakeBackend::new(log));
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacBackend)
    }
    #[cfg(windows)]
    {
        Box::new(windows::WinBackend)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Box::new(Unsupported)
    }
}

/// Permissions of the current process, for the app's settings (the app is what the system asks
/// about, and the helper inherits its answer).
pub fn permissions(request: bool) -> Permissions {
    let b = backend();
    if request {
        b.request_permissions()
    } else {
        b.permissions()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Recording {
    pid: u32,
    out: Utf8PathBuf,
    started_unix: u64,
    seconds: u32,
}

/// What the helper remembers between calls of one run. Kept next to the run context, where
/// Claude can't reach it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recording: Option<Recording>,
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Stops a recording a finished or cancelled run left behind.
pub fn stop_stray(state_file: &Utf8Path) {
    let Ok(state) = paths::read_json::<State>(state_file) else { return };
    if let Some(r) = state.recording {
        backend().record_cancel(r.pid);
    }
}

pub fn state_file(context_dir: &Utf8Path) -> Utf8PathBuf {
    context_dir.join("capture.json")
}

/// One `video-studio capture …` call.
pub struct Session {
    pub settings: CaptureSettings,
    pub run_folder: Utf8PathBuf,
    pub state_file: Utf8PathBuf,
    pub backend: Box<dyn Backend>,
}

/// Where pointer coordinates are measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// The top-left corner of the app's front window.
    Window,
    /// The top-left corner of the main display.
    Screen,
}

/// The run log of permissions capture needed and didn't have (`logs/capture-blocked.ndjson`).
pub const BLOCKED_LOG: &str = "capture-blocked.ndjson";
/// System Settings › Privacy & Security names, as the blocked log records them.
pub const SCREEN_RECORDING: &str = "Screen & System Audio Recording";
pub const ACCESSIBILITY: &str = "Accessibility";

/// The permissions capture found missing during a run, each once, in the order first met.
pub fn blocked_permissions(logs_dir: &Utf8Path) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let text = std::fs::read_to_string(logs_dir.join(BLOCKED_LOG)).unwrap_or_default();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        for m in v["missing"].as_array().into_iter().flatten().filter_map(|m| m.as_str()) {
            if !out.iter().any(|o| o == m) {
                out.push(m.to_string());
            }
        }
    }
    out
}

const ACCESSIBILITY_HELP: &str = "Clicking, typing and menus need the Accessibility permission. Tell the user: open System Settings › Privacy & Security › Accessibility and turn on Syncfusion Video Studio (or the terminal, when running the CLI), then run again.";
const SCREEN_HELP: &str = "Screenshots and recordings need the Screen & System Audio Recording permission. Tell the user: open System Settings › Privacy & Security › Screen & System Audio Recording and turn on Syncfusion Video Studio (or the terminal, when running the CLI), then run again.";

impl Session {
    fn load(&self) -> State {
        paths::read_json(&self.state_file).unwrap_or_default()
    }

    fn save(&self, s: &State) -> Result<()> {
        paths::write_json_atomic(&self.state_file, s)
    }

    /// `captures/<name>` inside the run folder, with the right extension.
    fn out_path(&self, name: &str, ext: &str) -> Result<Utf8PathBuf> {
        let rel = Utf8Path::new(name.trim());
        let rel = rel.strip_prefix(CAPTURES_DIR).unwrap_or(rel);
        let plain = !rel.as_str().is_empty()
            && rel.is_relative()
            && rel.components().all(|c| matches!(c, camino::Utf8Component::Normal(_)));
        if !plain {
            return Err(Error::Denied(format!(
                "--out is a file name inside {CAPTURES_DIR}/, for example `--out scene1.{ext}`; `{name}` isn't."
            )));
        }
        let mut p = self.run_folder.join(CAPTURES_DIR).join(rel);
        let given = p.extension().map(|e| e.to_ascii_lowercase());
        if given.as_deref() != Some(&ext.to_ascii_lowercase()) {
            if ext == RECORDING_EXT && matches!(given.as_deref(), Some("mov" | "mp4")) {
                // `scene1.mov` on Windows (or `.mp4` on a Mac) names the recording, not a format:
                // the system recorder decides the container.
                p.set_extension(ext);
            } else {
                p = Utf8PathBuf::from(format!("{p}.{ext}"));
            }
        }
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(p)
    }

    /// A path in the run folder as Claude should write it: relative, with forward slashes.
    fn rel(&self, p: &Utf8Path) -> String {
        paths::strip_path_prefix(&paths::normalize(p), &paths::normalize(&self.run_folder))
            .map(|r| r.as_str().replace('\\', "/"))
            .unwrap_or_else(|| p.to_string())
    }

    fn need_accessibility(&self) -> Result<()> {
        if self.backend.permissions().accessibility {
            Ok(())
        } else {
            self.note_blocked(&[ACCESSIBILITY]);
            Err(Error::Environment(ACCESSIBILITY_HELP.into()))
        }
    }

    /// Notes in the run's logs that capture needed a permission the app doesn't have, so the run's
    /// result says so even when it delivers something else (BUG-stellar-lily-K8CN).
    fn note_blocked(&self, missing: &[&str]) {
        let line = serde_json::json!({"ts": crate::timeutil::now_rfc3339(), "missing": missing}).to_string();
        let path = self.run_folder.join("logs").join(BLOCKED_LOG);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
    }

    /// Brings the target app to the front and returns its name. Every action that touches an app
    /// goes through here, so nothing is ever sent to an app the run may not drive.
    fn focus(&self, app: Option<&str>) -> Result<String> {
        let mut state = self.load();
        let target = app.map(str::to_string).or_else(|| state.app.clone()).ok_or_else(|| {
            Error::other("Name the app first: `video-studio capture open \"<app name>\"`, or pass `--app \"<name>\"`.")
        })?;
        self.settings.check_app(&target)?;
        let mut front = self.backend.frontmost()?;
        if !same_app(&front, &target) {
            self.backend.activate(&target)?;
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                front = self.backend.frontmost()?;
                if same_app(&front, &target) {
                    break;
                }
                if Instant::now() > deadline {
                    return Err(Error::other(format!(
                        "\"{target}\" didn't come to the front (\"{front}\" is in front). Check the app's name with `video-studio capture open`."
                    )));
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        // The name the system uses may be longer ("Chrome" → "Google Chrome"): check that too.
        self.settings.check_app(&front)?;
        if state.app.as_deref() != Some(front.as_str()) {
            state.app = Some(front.clone());
            self.save(&state)?;
        }
        Ok(front)
    }

    pub fn check(&self, request: bool) -> Result<String> {
        let p = if request { self.backend.request_permissions() } else { self.backend.permissions() };
        if !p.supported {
            return Unsupported::err();
        }
        let mut lines = vec![
            format!("Screen recording: {}", if p.screen_recording { "allowed" } else { "NOT allowed" }),
            format!("Accessibility (click, type, menus): {}", if p.accessibility { "allowed" } else { "NOT allowed" }),
        ];
        if !p.screen_recording {
            lines.push(SCREEN_HELP.into());
        }
        if !p.accessibility {
            lines.push(ACCESSIBILITY_HELP.into());
        }
        let missing: Vec<&str> =
            [(!p.screen_recording).then_some(SCREEN_RECORDING), (!p.accessibility).then_some(ACCESSIBILITY)]
                .into_iter()
                .flatten()
                .collect();
        if !missing.is_empty() {
            self.note_blocked(&missing);
        }
        lines.push(if self.settings.apps.is_empty() {
            "Apps: any app except terminals, system settings, password managers and Finder.".to_string()
        } else {
            format!("Apps this run may drive: {}.", self.settings.apps.join(", "))
        });
        Ok(lines.join("\n"))
    }

    /// Opens an app by name, or a web address in a browser.
    pub fn open(&self, target: &str, with: Option<&str>) -> Result<String> {
        let t = target.trim();
        let lower = t.to_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            if let Some(b) = with {
                self.settings.check_app(b)?;
            }
            self.backend.open_url(t, with)?;
            std::thread::sleep(Duration::from_millis(if self.is_fake() { 0 } else { 1500 }));
            let front = self.backend.frontmost()?;
            self.settings.check_app(&front)?;
            let mut state = self.load();
            state.app = Some(front.clone());
            self.save(&state)?;
            return Ok(format!("Opened {t} in {front}. {front} is now the app being driven."));
        }
        if t.contains("://") || t.contains('/') || with.is_some() {
            return Err(Error::Denied(
                "`capture open` takes an app name (\"Safari\") or an http(s) address, optionally `--with \"<browser>\"`. Files and other links can't be opened.".into(),
            ));
        }
        let front = self.focus(Some(t))?;
        let w = self.backend.window().ok();
        Ok(match w {
            Some(w) => format!("{front} is in front. Its window is {}×{} at {},{}.", w.w, w.h, w.x, w.y),
            None => format!("{front} is in front (it has no window yet)."),
        })
    }

    pub fn window(&self, app: Option<&str>, size: Option<(f64, f64)>, at: Option<(f64, f64)>) -> Result<String> {
        let front = self.focus(app)?;
        let w = if size.is_some() || at.is_some() {
            self.need_accessibility()?;
            if let Some((w, h)) = size {
                if !(200.0..=6000.0).contains(&w) || !(150.0..=4000.0).contains(&h) {
                    return Err(Error::other("--size is WIDTHxHEIGHT in points, for example 1280x800."));
                }
            }
            self.backend.set_window(size, at)?
        } else {
            self.backend.window()?
        };
        // Say what the window actually got: an app can refuse a size (Safari's minimum width is
        // 574 pt), and the caller shouldn't have to measure a screenshot to find out.
        let got = format!("window: {}x{} at {},{}", w.w.round(), w.h.round(), w.x.round(), w.y.round());
        let note = match size {
            Some((rw, rh)) if (w.w - rw).abs() > 2.0 || (w.h - rh).abs() > 2.0 => {
                let why = if w.w >= rw - 2.0 && w.h >= rh - 2.0 {
                    "the app's minimum is larger"
                } else {
                    "the app or the screen limited it"
                };
                format!(" (asked for {}x{}; {why})", rw.round(), rh.round())
            }
            _ => String::new(),
        };
        Ok(format!("{front}: {got}{note} (points)."))
    }

    fn frame_rect(&self, app: Option<&str>, frame: Frame) -> Result<Rect> {
        self.focus(app)?;
        match frame {
            Frame::Window => self.backend.window(),
            Frame::Screen => self.backend.screen(),
        }
    }

    pub fn screenshot(&self, out: &str, app: Option<&str>, frame: Frame) -> Result<String> {
        let path = self.out_path(out, "png")?;
        let rect = self.frame_rect(app, frame)?;
        self.backend.screenshot(rect, &path).map_err(|e| self.screen_error(e))?;
        Ok(format!(
            "Saved {} ({}×{}). Open it with the Read tool. One pixel is one point: use its pixel coordinates with click, move, drag and scroll{}.",
            self.rel(&path),
            rect.w,
            rect.h,
            if frame == Frame::Screen { " together with --screen" } else { "" }
        ))
    }

    fn screen_error(&self, e: Error) -> Error {
        if self.backend.permissions().screen_recording {
            e
        } else {
            self.note_blocked(&[SCREEN_RECORDING]);
            Error::Environment(format!("{e} {SCREEN_HELP}"))
        }
    }

    pub fn pointer(&self, action: Pointer, app: Option<&str>, frame: Frame) -> Result<String> {
        self.need_accessibility()?;
        let rect = self.frame_rect(app, frame)?;
        let place = |x: f64, y: f64| -> Result<(f64, f64)> {
            if !rect.contains(x, y) {
                return Err(Error::other(format!(
                    "{x},{y} is outside the {} ({}×{}). Take a fresh screenshot and use its pixel coordinates.",
                    if frame == Frame::Window { "window" } else { "screen" },
                    rect.w,
                    rect.h
                )));
            }
            Ok((rect.x + x, rect.y + y))
        };
        let (absolute, done) = match action {
            Pointer::Move { x, y } => {
                let (ax, ay) = place(x, y)?;
                (Pointer::Move { x: ax, y: ay }, format!("Moved the pointer to {x},{y}."))
            }
            Pointer::Click { x, y, right, count } => {
                let (ax, ay) = place(x, y)?;
                let what = match (right, count) {
                    (true, _) => "Right-clicked",
                    (false, 2) => "Double-clicked",
                    _ => "Clicked",
                };
                (Pointer::Click { x: ax, y: ay, right, count: count.clamp(1, 3) }, format!("{what} at {x},{y}."))
            }
            Pointer::Drag { x, y, to_x, to_y } => {
                let (ax, ay) = place(x, y)?;
                let (bx, by) = place(to_x, to_y)?;
                (Pointer::Drag { x: ax, y: ay, to_x: bx, to_y: by }, format!("Dragged from {x},{y} to {to_x},{to_y}."))
            }
            Pointer::Scroll { x, y, lines, over } => {
                if let Some(t) = over {
                    if !SCROLL_OVER_RANGE.contains(&t) {
                        return Err(Error::other(format!(
                            "--over is how many seconds the scroll takes: {} to {}.",
                            SCROLL_OVER_RANGE.start(),
                            SCROLL_OVER_RANGE.end()
                        )));
                    }
                }
                let (ax, ay) = place(x, y)?;
                let lines = lines.clamp(-50, 50);
                (
                    Pointer::Scroll { x: ax, y: ay, lines, over },
                    format!(
                        "Scrolled {} {} lines at {x},{y}{}.",
                        if lines < 0 { "down" } else { "up" },
                        lines.abs(),
                        over.map(|t| format!(" over {t} s")).unwrap_or_default()
                    ),
                )
            }
        };
        self.backend.pointer(absolute)?;
        Ok(done)
    }

    pub fn type_text(&self, text: &str, app: Option<&str>) -> Result<String> {
        self.need_accessibility()?;
        if text.is_empty() {
            return Err(Error::other("Give the text to type."));
        }
        if text.chars().count() > 2000 {
            return Err(Error::other("That is too much to type in one call (2000 characters at most)."));
        }
        let front = self.focus(app)?;
        self.backend.type_text(text)?;
        Ok(format!("Typed {} characters into {front}.", text.chars().count()))
    }

    pub fn key(&self, combo: &str, times: u32, app: Option<&str>) -> Result<String> {
        self.need_accessibility()?;
        let c = KeyCombo::parse(combo)?;
        let front = self.focus(app)?;
        for _ in 0..times.clamp(1, 50) {
            self.backend.key(&c)?;
        }
        Ok(format!("Pressed {combo} in {front}{}.", if times > 1 { format!(" {times} times") } else { String::new() }))
    }

    pub fn menu(&self, path: &[String], app: Option<&str>) -> Result<String> {
        self.need_accessibility()?;
        if path.len() < 2 {
            return Err(Error::other(
                "Give the menu and the item, for example `capture menu \"File\" \"New Project…\"`. Menu names must match exactly, including the … character.",
            ));
        }
        let front = self.focus(app)?;
        self.backend.menu(path)?;
        Ok(format!("Chose {} in {front}.", path.join(" › ")))
    }

    pub fn wait(&self, seconds: f64) -> Result<String> {
        if !(0.0..=30.0).contains(&seconds) {
            return Err(Error::other("Wait between 0 and 30 seconds per call."));
        }
        if !self.is_fake() {
            std::thread::sleep(Duration::from_secs_f64(seconds));
        }
        Ok(format!("Waited {seconds} s."))
    }

    fn is_fake(&self) -> bool {
        std::env::var(FAKE_ENV).map(|v| !v.is_empty()).unwrap_or(false)
    }

    /// Starts a recording of fixed length. The system recorder can't be stopped early without
    /// losing the file, so the length is chosen up front and the edit trims what it doesn't need.
    pub fn record_start(
        &self,
        out: &str,
        seconds: u32,
        app: Option<&str>,
        frame: Frame,
        show_clicks: bool,
    ) -> Result<String> {
        if !(1..=MAX_RECORDING_S).contains(&seconds) {
            return Err(Error::other(format!(
                "--seconds is how long to record: 1 to {MAX_RECORDING_S}. Allow for every step you will perform plus a few seconds; the edit trims the rest."
            )));
        }
        let mut state = self.load();
        if let Some(r) = &state.recording {
            if self.backend.record_running(r.pid) {
                return Err(Error::other(format!(
                    "A recording is already running ({}, about {} s left). Wait for it with `video-studio capture record wait`, or drop it with `video-studio capture record cancel`.",
                    self.rel(&r.out),
                    (r.seconds as u64).saturating_sub(now_unix().saturating_sub(r.started_unix))
                )));
            }
        }
        let path = self.out_path(out, RECORDING_EXT)?;
        let rect = self.frame_rect(app, frame)?;
        let _ = std::fs::remove_file(&path);
        let pid = self.backend.record_start(rect, &path, seconds, show_clicks).map_err(|e| self.screen_error(e))?;
        // `focus` may have saved the app name: keep it.
        state = self.load();
        state.recording = Some(Recording { pid, out: path.clone(), started_unix: now_unix(), seconds });
        self.save(&state)?;
        Ok(format!(
            "Recording {} to {} for {seconds} s ({}×{} points), starting now. Perform the steps, then run `video-studio capture record wait` to let it finish; the file is complete only after that. Don't move or resize the window while it records.",
            if frame == Frame::Window { "the window" } else { "the screen" },
            self.rel(&path),
            rect.w,
            rect.h
        ))
    }

    /// Waits for the running recording to end and reports the file.
    pub fn record_wait(&self) -> Result<String> {
        let mut state = self.load();
        let Some(r) = state.recording.clone() else {
            return Err(Error::other("No recording is running."));
        };
        let deadline = Instant::now() + Duration::from_secs(WAIT_SLICE_S);
        while self.backend.record_running(r.pid) {
            if Instant::now() > deadline {
                return Ok(format!(
                    "Still recording {} (about {} s left). Run `video-studio capture record wait` again.",
                    self.rel(&r.out),
                    (r.seconds as u64).saturating_sub(now_unix().saturating_sub(r.started_unix))
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        state.recording = None;
        self.save(&state)?;
        // The file appears just after the recorder exits.
        let mut size = 0;
        for _ in 0..20 {
            size = std::fs::metadata(&r.out).map(|m| m.len()).unwrap_or(0);
            if size > 0 || self.is_fake() {
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        if size == 0 {
            return Err(
                self.screen_error(Error::other(format!("The recording {} is empty or missing.", self.rel(&r.out))))
            );
        }
        // The recorder only writes a frame when the picture changes: make every frame explicit.
        let cfr = match self.backend.ffmpeg() {
            Some(ff) => rewrite_cfr(&ff, &r.out).map_err(|e| e.to_string()),
            None if self.is_fake() => Err(String::new()),
            None => Err("ffmpeg wasn't found (`cutline deps install --all`)".into()),
        };
        let size = std::fs::metadata(&r.out).map(|m| m.len()).unwrap_or(size);
        Ok(match cfr {
            Ok(()) => format!(
                "Saved {} ({}), rewritten at a constant {RECORDING_FPS} fps. Check the real length with `cutline probe`, then use it in project.json5 like any other clip and trim it to the part you need.",
                self.rel(&r.out),
                paths::format_bytes(size)
            ),
            Err(why) => format!(
                "Saved {} ({}). The recorder only writes frames when the picture changes, so check the real length with `cutline probe`, then use it in project.json5 like any other clip and trim it to the part you need.{}",
                self.rel(&r.out),
                paths::format_bytes(size),
                if why.is_empty() {
                    String::new()
                } else {
                    format!(" Warning: it couldn't be rewritten at a constant {RECORDING_FPS} fps ({why}), so it's kept as recorded.")
                }
            ),
        })
    }

    /// Drops the running recording.
    pub fn record_cancel(&self) -> Result<String> {
        let mut state = self.load();
        let Some(r) = state.recording.take() else {
            return Err(Error::other("No recording is running."));
        };
        self.backend.record_cancel(r.pid);
        self.save(&state)?;
        let _ = std::fs::remove_file(&r.out);
        Ok(format!("Cancelled the recording; {} was discarded.", self.rel(&r.out)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(dir: &Utf8Path, apps: &[&str]) -> Session {
        std::fs::create_dir_all(dir.join("run")).unwrap();
        Session {
            settings: CaptureSettings { apps: apps.iter().map(|s| s.to_string()).collect() },
            run_folder: dir.join("run"),
            state_file: dir.join("capture.json"),
            backend: Box::new(FakeBackend::new(dir.join("fake.ndjson"))),
        }
    }

    fn tmp() -> (tempfile::TempDir, Utf8PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let p = Utf8PathBuf::from_path_buf(t.path().canonicalize().unwrap()).unwrap();
        (t, p)
    }

    fn actions(dir: &Utf8Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(dir.join("fake.ndjson"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// BUG-stellar-lily-K8CN: a missing permission is logged for the run's result, once each.
    #[test]
    fn missing_permissions_are_logged_for_the_run() {
        let (_t, dir) = tmp();
        let mut s = session(&dir, &[]);
        std::fs::create_dir_all(dir.join("run/logs")).unwrap();
        let logs = dir.join("run/logs");
        assert!(s.check(false).unwrap().contains("Screen recording: allowed"));
        assert!(blocked_permissions(&logs).is_empty(), "nothing is missing yet");

        s.backend = Box::new(FakeBackend::new(dir.join("fake.ndjson")).with_permissions(false, true));
        assert!(s.check(false).unwrap().contains("Screen recording: NOT allowed"));
        s.backend = Box::new(FakeBackend::new(dir.join("fake.ndjson")).with_permissions(false, false));
        let _ = s.check(false);
        assert_eq!(blocked_permissions(&logs), vec![SCREEN_RECORDING.to_string(), ACCESSIBILITY.to_string()]);
    }

    #[test]
    fn key_combos() {
        let c = KeyCombo::parse("cmd+shift+N").unwrap();
        assert!(c.command && c.shift && !c.control && !c.option && c.key == "n");
        assert_eq!(KeyCombo::parse("Return").unwrap().key, "return");
        assert_eq!(KeyCombo::parse("enter").unwrap().key, "return");
        assert_eq!(KeyCombo::parse("ctrl+alt+f5").unwrap().key, "f5");
        let plus = KeyCombo::parse("cmd++").unwrap();
        assert!(plus.command && plus.key == "+");
        assert!(KeyCombo::parse("shift").is_err(), "a modifier alone isn't a key");
        assert!(
            KeyCombo::parse("cmd+").is_err()
                && KeyCombo::parse("hyper+k").is_err()
                && KeyCombo::parse("cmd+nope").is_err()
        );
    }

    #[test]
    fn apps_a_run_may_drive() {
        let any = CaptureSettings::default();
        assert!(any.check_app("Safari").is_ok() && any.check_app("Syncfusion Video Studio").is_ok());
        for guarded in ["Terminal", "terminal.app", "iTerm2", "System Settings", "Finder", "1Password"] {
            assert!(any.check_app(guarded).is_err(), "{guarded}");
        }
        let listed = CaptureSettings { apps: vec!["Chrome".into(), "Terminal".into()] };
        assert!(listed.check_app("Google Chrome").is_ok(), "the system's longer name matches");
        assert!(listed.check_app("Terminal").is_ok(), "listing a guarded app allows it");
        assert!(listed.check_app("Safari").unwrap_err().to_string().contains("Project › App recording"));
    }

    #[test]
    fn drives_only_the_app_it_was_pointed_at() {
        let (_t, dir) = tmp();
        let s = session(&dir, &[]);
        assert!(s.type_text("hello", None).unwrap_err().to_string().contains("Name the app first"));
        assert!(s.open("Terminal", None).is_err());
        assert!(s.open("/etc/passwd", None).is_err() && s.open("file:///etc/passwd", None).is_err());
        assert!(s.open("Notes", None).unwrap().contains("Notes is in front"));
        // Something else comes to the front: the next action brings the app back first.
        std::fs::write(dir.join("fake.ndjson.front"), "Mail").unwrap();
        assert!(s.type_text("hello", None).unwrap().contains("into Notes"));
        let a = actions(&dir);
        assert_eq!(a.iter().filter(|v| v["do"] == "activate" && v["app"] == "Notes").count(), 2);
        assert_eq!(a.last().unwrap()["text"], "hello");
        // A guarded app in front is never typed into, even by its longer name.
        assert!(s.key("cmd+n", 1, Some("Terminal")).is_err());
        assert!(s.open("https://example.com/app", None).unwrap().contains("Safari"));
        assert!(s.open("https://example.com/app", Some("Terminal")).is_err());
    }

    #[test]
    fn coordinates_are_window_relative_and_bounded() {
        let (_t, dir) = tmp();
        let s = session(&dir, &[]);
        s.open("Notes", None).unwrap();
        s.pointer(Pointer::Click { x: 10.0, y: 20.0, right: false, count: 2 }, None, Frame::Window).unwrap();
        let a = actions(&dir);
        let click = a.last().unwrap();
        assert_eq!(
            (click["x"].as_f64(), click["y"].as_f64(), click["count"].as_u64()),
            (Some(110.0), Some(100.0), Some(2))
        );
        let err = s.pointer(Pointer::Click { x: 5000.0, y: 20.0, right: false, count: 1 }, None, Frame::Window);
        assert!(err.unwrap_err().to_string().contains("outside the window"));
        s.pointer(Pointer::Move { x: 1500.0, y: 20.0 }, None, Frame::Screen).unwrap();
        assert_eq!(actions(&dir).last().unwrap()["x"].as_f64(), Some(1500.0));
    }

    #[test]
    fn files_stay_in_captures_and_recordings_have_a_fixed_length() {
        let (_t, dir) = tmp();
        let s = session(&dir, &[]);
        s.open("Notes", None).unwrap();
        for bad in ["../x.png", "/tmp/x.png", "a/../../x", ""] {
            assert!(s.screenshot(bad, None, Frame::Window).is_err(), "{bad}");
            assert!(s.record_start(bad, 10, None, Frame::Window, false).is_err(), "{bad}");
        }
        assert!(s.screenshot("captures/look", None, Frame::Window).unwrap().contains("captures/look.png"));
        assert!(dir.join("run/captures/look.png").is_file());
        assert!(s.record_wait().unwrap_err().to_string().contains("No recording"));
        assert!(s.record_start("long", 99_999, None, Frame::Window, false).is_err());
        assert!(s.record_start("none", 0, None, Frame::Window, false).is_err());
        let started = s.record_start("scene1", 45, None, Frame::Window, true).unwrap();
        let scene1 = format!("captures/scene1.{RECORDING_EXT}");
        assert!(started.contains(&scene1) && started.contains("for 45 s"), "{started}");
        assert!(s.record_wait().unwrap().contains(&scene1));
        assert!(s.record_wait().is_err());
        let a = actions(&dir);
        assert!(a.iter().any(|v| v["do"] == "record_start" && v["clicks"] == true && v["seconds"] == 45));
        s.record_start("scene2", 20, None, Frame::Window, false).unwrap();
        assert!(
            s.record_cancel().unwrap().contains("discarded")
                && !dir.join(format!("run/captures/scene2.{RECORDING_EXT}")).exists()
        );
        // a run that ends while recording has its recorder ended
        s.record_start("scene3", 20, None, Frame::Window, false).unwrap();
        std::env::set_var(FAKE_ENV, dir.join("fake.ndjson").as_str());
        stop_stray(&dir.join("capture.json"));
        std::env::remove_var(FAKE_ENV);
        assert_eq!(actions(&dir).iter().filter(|v| v["do"] == "record_cancel").count(), 2);
    }

    /// TASK-honest-cedar-A73E: a sparse recording (25 frames over 7 s) comes out of `record
    /// wait` at a constant 30 fps, as `cutline probe` sees it; a failed rewrite keeps the file.
    #[test]
    fn record_wait_rewrites_recordings_at_a_constant_frame_rate() {
        let Ok(tc) = cutline_deps::toolchain() else {
            eprintln!("SKIP: no ffmpeg toolchain");
            return;
        };
        let (_t, dir) = tmp();
        let src = dir.join("sparse.mov");
        let keep: Vec<String> = [
            0, 10, 13, 15, 16, 18, 39, 43, 44, 45, 59, 75, 82, 103, 114, 122, 128, 129, 132, 150, 158, 162, 170, 187,
            209,
        ]
        .iter()
        .map(|n| format!("eq(n,{n})"))
        .collect();
        let st = std::process::Command::new(&tc.ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x240:r=30:d=7"])
            .args([
                "-vf",
                &format!("select='{}'", keep.join("+")),
                "-fps_mode",
                "passthrough",
                "-c:v",
                "libx264",
                "-bf",
                "0",
            ])
            .args(["-pix_fmt", "yuv420p", src.as_str()])
            .status()
            .unwrap();
        assert!(st.success());
        let before = cutline_media::probe::probe(tc, src.as_std_path(), None).unwrap();
        assert!(before.video.as_ref().unwrap().vfr, "the fixture is variable-rate");

        let ff = Utf8PathBuf::from_path_buf(tc.ffmpeg.clone()).unwrap();
        let mut s = session(&dir, &[]);
        s.backend = Box::new(FakeBackend::new(dir.join("fake.ndjson")).with_recording(src.clone(), Some(ff)));
        s.open("Notes", None).unwrap();
        s.record_start("scene1", 7, None, Frame::Window, false).unwrap();
        let msg = s.record_wait().unwrap();
        assert!(msg.contains("rewritten at a constant 30 fps"), "{msg}");
        let out = dir.join(format!("run/captures/scene1.{RECORDING_EXT}"));
        let info = cutline_media::probe::probe(tc, out.as_std_path(), None).unwrap();
        let v = info.video.unwrap();
        assert!(!v.vfr && (v.fps_f64 - 30.0).abs() < 0.5, "{v:?}");
        assert!((info.duration - 7.0).abs() < 0.1, "{}", info.duration);
        assert!(!dir.join("run/captures").read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("cfr-tmp")));

        // ffmpeg fails (not a video): the file is kept as it was and the reply warns
        let junk = dir.join("junk.mov");
        std::fs::write(&junk, b"not a video").unwrap();
        let ff = Utf8PathBuf::from_path_buf(tc.ffmpeg.clone()).unwrap();
        s.backend = Box::new(FakeBackend::new(dir.join("fake.ndjson")).with_recording(junk, Some(ff)));
        s.record_start("scene2", 7, None, Frame::Window, false).unwrap();
        let msg = s.record_wait().unwrap();
        assert!(msg.contains("Warning: it couldn't be rewritten"), "{msg}");
        assert_eq!(std::fs::read(dir.join(format!("run/captures/scene2.{RECORDING_EXT}"))).unwrap(), b"not a video");
    }

    #[test]
    fn timed_scrolls_are_even_and_complete() {
        // 12 lines over 10 s on macOS (pixels): 100 steps 0.1 s apart, 120 px in all
        let (steps, gap) = scroll_plan(-12, 10.0, 10);
        assert_eq!(steps.len(), 100);
        assert_eq!(steps.iter().sum::<i32>(), -120);
        assert!(steps.iter().all(|d| (-2..=-1).contains(d)), "{steps:?}");
        assert_eq!(gap, Duration::from_millis(100));
        // Windows notches: a short, fast scroll still has a step per line
        let (steps, gap) = scroll_plan(30, 0.5, 120);
        assert_eq!((steps.len(), steps.iter().sum::<i32>()), (30, 3600));
        assert!((gap.as_secs_f64() - 0.5 / 30.0).abs() < 1e-9);
        let (steps, _) = scroll_plan(1, 0.1, 120);
        assert_eq!(steps, vec![120]);

        let (_t, dir) = tmp();
        let s = session(&dir, &[]);
        s.open("Notes", None).unwrap();
        let done = s.pointer(Pointer::Scroll { x: 566.0, y: 700.0, lines: -12, over: Some(10.0) }, None, Frame::Window);
        assert_eq!(done.unwrap(), "Scrolled down 12 lines at 566,700 over 10 s.");
        assert_eq!(actions(&dir).last().unwrap()["over"].as_f64(), Some(10.0));
        for bad in [0.0, 0.05, 61.0, f64::NAN] {
            let e = s.pointer(Pointer::Scroll { x: 566.0, y: 700.0, lines: -12, over: Some(bad) }, None, Frame::Window);
            assert!(e.unwrap_err().to_string().contains("--over"), "{bad}");
        }
        let plain = s.pointer(Pointer::Scroll { x: 566.0, y: 700.0, lines: -1, over: None }, None, Frame::Window);
        assert_eq!(plain.unwrap(), "Scrolled down 1 lines at 566,700.");
    }

    #[test]
    fn window_reports_the_size_it_got() {
        let (_t, dir) = tmp();
        let s = session(&dir, &[]);
        s.open("Safari", None).unwrap();
        assert_eq!(
            s.window(None, Some((540.0, 960.0)), Some((80.0, 40.0))).unwrap(),
            "Safari: window: 574x960 at 80,40 (asked for 540x960; the app's minimum is larger) (points)."
        );
        assert_eq!(
            s.window(None, Some((1280.0, 800.0)), Some((80.0, 40.0))).unwrap(),
            "Safari: window: 1280x800 at 80,40 (points)."
        );
        assert_eq!(s.window(None, Some((575.5, 801.0)), None).unwrap(), "Safari: window: 576x801 at 100,80 (points).");
    }
}
