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
        if GUARDED_APPS.iter().any(|g| g.eq_ignore_ascii_case(app.trim().trim_end_matches(".app"))) {
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
    Move { x: f64, y: f64 },
    Click { x: f64, y: f64, right: bool, count: u8 },
    Drag { x: f64, y: f64, to_x: f64, to_y: f64 },
    Scroll { x: f64, y: f64, lines: i32 },
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
}

impl FakeBackend {
    pub fn new(log: impl Into<Utf8PathBuf>) -> Self {
        FakeBackend { log: log.into() }
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
        Permissions { screen_recording: true, accessibility: true, supported: true }
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
            Pointer::Scroll { x, y, lines } => json!({"do": "scroll", "x": x, "y": y, "lines": lines}),
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
        std::fs::write(out, b"fake recording")?;
        Ok(0)
    }
    fn record_running(&self, _: u32) -> bool {
        false
    }
    fn record_cancel(&self, pid: u32) {
        self.note(json!({"do": "record_cancel", "pid": pid}));
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
    #[cfg(not(target_os = "macos"))]
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
        if p.extension().map(|e| e.eq_ignore_ascii_case(ext)) != Some(true) {
            p = Utf8PathBuf::from(format!("{p}.{ext}"));
        }
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(p)
    }

    fn rel(&self, p: &Utf8Path) -> String {
        p.strip_prefix(&self.run_folder).map(|r| r.to_string()).unwrap_or_else(|_| p.to_string())
    }

    fn need_accessibility(&self) -> Result<()> {
        if self.backend.permissions().accessibility {
            Ok(())
        } else {
            Err(Error::Environment(ACCESSIBILITY_HELP.into()))
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
        Ok(format!("{front}: window {}×{} at {},{} (points).", w.w, w.h, w.x, w.y))
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
            Pointer::Scroll { x, y, lines } => {
                let (ax, ay) = place(x, y)?;
                (
                    Pointer::Scroll { x: ax, y: ay, lines: lines.clamp(-50, 50) },
                    format!("Scrolled {} {} lines at {x},{y}.", if lines < 0 { "down" } else { "up" }, lines.abs()),
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
        let path = self.out_path(out, "mov")?;
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
        Ok(format!(
            "Saved {} ({}). The recorder only writes frames when the picture changes, so check the real length with `cutline probe`, then use it in project.json5 like any other clip and trim it to the part you need.",
            self.rel(&r.out),
            paths::format_bytes(size)
        ))
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
        assert!(started.contains("captures/scene1.mov") && started.contains("for 45 s"), "{started}");
        assert!(s.record_wait().unwrap().contains("captures/scene1.mov"));
        assert!(s.record_wait().is_err());
        let a = actions(&dir);
        assert!(a.iter().any(|v| v["do"] == "record_start" && v["clicks"] == true && v["seconds"] == 45));
        s.record_start("scene2", 20, None, Frame::Window, false).unwrap();
        assert!(s.record_cancel().unwrap().contains("discarded") && !dir.join("run/captures/scene2.mov").exists());
        // a run that ends while recording has its recorder ended
        s.record_start("scene3", 20, None, Frame::Window, false).unwrap();
        std::env::set_var(FAKE_ENV, dir.join("fake.ndjson").as_str());
        stop_stray(&dir.join("capture.json"));
        std::env::remove_var(FAKE_ENV);
        assert_eq!(actions(&dir).iter().filter(|v| v["do"] == "record_cancel").count(), 2);
    }
}
