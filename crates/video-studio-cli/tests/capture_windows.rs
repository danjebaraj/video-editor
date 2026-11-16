//! App recording on Windows with the real backend (ui/spec.md §23.7, §23.9 item 5):
//! `video-studio capture` drives `vs-capture-target` — open, window, screenshot, click, type, key,
//! menu, scroll, drag, wait and a 5-second recording checked with `cutline probe` — and the guard
//! rails refuse a terminal.
//!
//! It needs an interactive desktop (it moves the real pointer and keyboard) and cutline with its
//! ffmpeg for the recording; without them it says why and passes without checking.

#![cfg(windows)]

mod common;

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use common::*;
use serde_json::Value;
use video_studio_core::claude::Mode;
use video_studio_core::eleven::HelperContext;

const TARGET: &str = env!("CARGO_BIN_EXE_vs-capture-target");

fn cutline() -> Option<Utf8PathBuf> {
    if let Ok(c) = std::env::var("VIDEO_STUDIO_CUTLINE") {
        return Some(c.into());
    }
    let next = Utf8Path::new(VIDEO_STUDIO).parent()?.join("cutline.exe");
    next.is_file().then_some(next)
}

struct Target(Child);
impl Drop for Target {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn interactive() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // A service or a locked session has no foreground window to give.
    unsafe { !GetForegroundWindow().is_invalid() }
}

#[test]
fn drives_and_records_the_test_app() {
    if !interactive() {
        eprintln!("skipped: no interactive desktop");
        return;
    }
    let Some(cutline) = cutline() else {
        eprintln!("skipped: cutline isn't built next to video-studio.exe and VIDEO_STUDIO_CUTLINE isn't set");
        return;
    };
    let w = World::new();
    let run = w.root.join("run");
    std::fs::create_dir_all(run.join("logs")).unwrap();
    let ctx = HelperContext {
        run_id: "run_capture".into(),
        run_folder: run.clone(),
        mode: Mode::Production,
        logs_dir: run.join("logs"),
        cutline: Some(cutline.clone()),
        credit_cap: 0,
        cache_dir: w.lib.join("cache"),
        cache_limit_bytes: 0,
        default_voice: None,
        default_voice_settings: None,
        default_model: None,
        voice_preference: Default::default(),
        voice_gender: Default::default(),
        voice_provider: Default::default(),
        cartesia_base_url: None,
        cartesia_test_key: None,
        base_url: None,
        test_api_key: None,
        capture: Some(Default::default()),
    };
    let ctx_dir = w.lib.join("run-contexts/run_capture");
    std::fs::create_dir_all(&ctx_dir).unwrap();
    let ctx_file = ctx_dir.join("context.json");
    std::fs::write(&ctx_file, serde_json::to_string(&ctx).unwrap()).unwrap();
    let log = w.root.join("target.log");
    let _target =
        Target(Command::new(TARGET).arg("--tone").env("VS_CAPTURE_TARGET_LOG", log.as_str()).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(&log).unwrap_or_default().contains("started") {
        assert!(Instant::now() < deadline, "the test app didn't start");
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(500));

    let capture = |args: &[&str]| -> (i32, String) {
        let mut full = vec!["capture"];
        full.extend_from_slice(args);
        let o = w
            .command(&full)
            .env("VIDEO_STUDIO_RUN_CONTEXT", ctx_file.as_str())
            .env("VIDEO_STUDIO_CUTLINE", cutline.as_str())
            .current_dir(&run)
            .output()
            .unwrap();
        (o.status.code().unwrap_or(-1), format!("{}{}", stdout(&o), stderr(&o)))
    };
    let ok = |args: &[&str]| -> String {
        let (code, out) = capture(args);
        assert_eq!(code, 0, "capture {args:?}: {out}");
        out
    };
    let target_log = || std::fs::read_to_string(&log).unwrap_or_default();

    let check = ok(&["check"]);
    assert!(check.contains("Screen recording: allowed"), "{check}");
    let opened = ok(&["open", "vs-capture-target"]);
    assert!(opened.contains("vs-capture-target is in front"), "{opened}");
    let win = ok(&["window", "--size", "700x480", "--at", "100,100"]);
    assert!(win.contains("700×480") || win.contains("700x480"), "{win}");

    // Where the controls are, relative to the window frame the commands use.
    let (button, edit) = control_points();
    let shot = ok(&["screenshot", "--out", "look.png"]);
    assert!(shot.contains("captures/look.png"), "{shot}");
    let png = image::open(run.join("captures/look.png")).unwrap();
    assert!(png.width() >= 690 && png.height() >= 470, "{}×{}", png.width(), png.height());

    ok(&["click", &button.0.to_string(), &button.1.to_string()]);
    wait_for(|| target_log().contains("button clicked"), "the click");
    ok(&["click", &edit.0.to_string(), &edit.1.to_string()]);
    ok(&["type", "hello world"]);
    wait_for(|| target_log().contains("text: hello world"), "the typing");
    ok(&["key", "delete"]);
    wait_for(|| target_log().contains("text: hello worl\n") || target_log().ends_with("text: hello worl"), "Backspace");
    ok(&["menu", "File", "Say Hello"]);
    wait_for(|| target_log().contains("menu: Say Hello"), "the menu choice");
    ok(&["scroll", "300", "300", "-3"]);
    ok(&["drag", "300", "250", "420", "300"]);
    ok(&["wait", "0.5"]);

    // A 5-second recording with the app's tone on its sound track.
    let started = ok(&["record", "start", "--out", "clip", "--seconds", "5"]);
    assert!(started.contains("captures/clip.mp4"), "{started}");
    ok(&["click", &button.0.to_string(), &button.1.to_string()]);
    let mut waited = ok(&["record", "wait"]);
    while waited.contains("Still recording") {
        waited = ok(&["record", "wait"]);
    }
    assert!(waited.contains("captures/clip.mp4"), "{waited}");
    let clip = run.join("captures/clip.mp4");
    assert!(clip.is_file() && std::fs::metadata(&clip).unwrap().len() > 10_000);
    let probe = Command::new(cutline.as_str()).args(["probe", "--json", clip.as_str()]).output().unwrap();
    let v: Value = serde_json::from_slice(&probe.stdout).unwrap_or(Value::Null);
    let dur = v["duration"].as_f64().unwrap_or(0.0);
    assert!((4.0..=6.5).contains(&dur), "duration {dur}: {v}");
    assert!(v["video"]["width"].as_u64().unwrap_or(0) >= 690, "{v}");
    assert!(v["audio"].is_object(), "the app's sound was recorded: {v}");

    // Guard rails: a terminal is never opened or driven.
    let (code, out) = capture(&["open", "cmd"]);
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("can't be driven"), "{out}");
    let (code, out) = capture(&["open", "WindowsTerminal"]);
    assert_ne!(code, 0, "{out}");
    let (code, out) = capture(&["screenshot", "--out", "../escape.png"]);
    assert_ne!(code, 0, "{out}");
    assert!(!run.join("escape.png").exists());
}

fn wait_for(what: impl Fn() -> bool, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !what() {
        assert!(Instant::now() < deadline, "{label} didn't reach the test app");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Centers of the button and the text box, relative to the window's frame (what `capture click`
/// measures from).
fn control_points() -> ((i32, i32), (i32, i32)) {
    use windows::core::w;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, GetWindowRect};
    unsafe {
        let main = FindWindowW(w!("VsCaptureTarget"), None).unwrap();
        let mut frame = RECT::default();
        DwmGetWindowAttribute(main, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut _ as *mut _, 16).unwrap();
        let center = |class: windows::core::PCWSTR| {
            let h = FindWindowExW(Some(main), None, class, None).unwrap();
            let mut r = RECT::default();
            GetWindowRect(h, &mut r).unwrap();
            ((r.left + r.right) / 2 - frame.left, (r.top + r.bottom) / 2 - frame.top)
        };
        (center(w!("BUTTON")), center(w!("EDIT")))
    }
}
