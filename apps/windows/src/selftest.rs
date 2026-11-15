//! The in-app self test (ui/spec.md §23.9 item 3) and window snapshots.
//!
//! `build.ps1 selftest` (or `VIDEO_STUDIO_SELFTEST=1`) starts the app against a scratch library,
//! the fake Claude and the real cutline. The web UI performs each step it is sent on the
//! `selftest` topic and reports back with `selftest.report`; every step saves a snapshot next to
//! the log. The process exits non-zero when any step fails.

use std::cell::RefCell;
use std::io::Write;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};
use webview2_com::CapturePreviewCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
use windows::core::HSTRING;
use windows::Win32::System::Com::STGM_CREATE;
use windows::Win32::System::Com::STGM_WRITE;
use windows::Win32::UI::Shell::SHCreateStreamOnFileEx;

use crate::app;
use crate::protocol::{SelfTestReport, Topic, WindowKind};

/// Saves a PNG of a window's content; `done` is called on the UI thread.
pub fn snapshot(window: u64, path: &str, done: impl FnOnce(Result<(), String>) + 'static) {
    let Some(w) = app::window(window) else {
        done(Err("no window".into()));
        return;
    };
    let Some(wv) = w.webview.borrow().clone() else {
        done(Err("no web view".into()));
        return;
    };
    if let Some(dir) = Utf8Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let stream =
        unsafe { SHCreateStreamOnFileEx(&HSTRING::from(path), (STGM_CREATE | STGM_WRITE).0, 0x80, true, None) };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            done(Err(e.message().to_string()));
            return;
        }
    };
    let done = RefCell::new(Some(done));
    let keep = stream.clone();
    let handler = CapturePreviewCompletedHandler::create(Box::new(move |code| {
        let _ = &keep;
        if let Some(d) = done.borrow_mut().take() {
            d(code.map_err(|e| e.message().to_string()));
        }
        Ok(())
    }));
    unsafe {
        if let Err(e) = wv.CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler) {
            app::error_box(&format!("snapshot failed: {e}"));
        }
    }
}

thread_local! {
    static STATE: RefCell<Option<SelfTest>> = const { RefCell::new(None) };
}

struct SelfTest {
    out: Utf8PathBuf,
    log: std::fs::File,
    failures: usize,
    steps: usize,
}

/// A step finished (or the UI reports `done`).
pub fn report(r: SelfTestReport) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(st) = s.as_mut() else { return };
        if r.step == "done" {
            let ok = st.failures == 0 && r.ok;
            let _ = writeln!(
                st.log,
                "{} {} step(s), {} failed{}",
                if ok { "PASS" } else { "FAIL" },
                st.steps,
                st.failures,
                if r.detail.is_empty() { String::new() } else { format!(": {}", r.detail) }
            );
            let code = if ok { 0 } else { 1 };
            eprintln!(
                "self test {}: {} step(s), {} failed. Log: {}",
                if ok { "passed" } else { "FAILED" },
                st.steps,
                st.failures,
                st.out.join("selftest.log")
            );
            std::process::exit(code);
        }
        st.steps += 1;
        if !r.ok {
            st.failures += 1;
        }
        let line = format!(
            "{} {}{}",
            if r.ok { "ok  " } else { "FAIL" },
            r.step,
            if r.detail.is_empty() { String::new() } else { format!(" — {}", r.detail) }
        );
        let _ = writeln!(st.log, "{line}");
        eprintln!("{line}");
    });
}

/// Folder operations for the relink step, confined to the scratch folder.
pub fn scratch_fs(op: &str, from: &str, to: Option<&str>) -> Result<bool, String> {
    let scratch = STATE.with(|s| s.borrow().as_ref().map(|st| st.out.clone())).ok_or("no self test is running")?;
    let inside = |p: &str| {
        let p = video_studio_core::paths::normalize(Utf8Path::new(p));
        video_studio_core::paths::starts_with_path(&p, &scratch)
            && p.components().count() > scratch.components().count()
    };
    if !inside(from) || to.map(|t| !inside(t)).unwrap_or(false) {
        return Err("outside the self test's scratch folder".into());
    }
    match (op, to) {
        ("rename", Some(t)) => std::fs::rename(from, t).map_err(|e| e.to_string())?,
        ("copy", Some(t)) => copy_dir(Utf8Path::new(from), Utf8Path::new(t)).map_err(|e| e.to_string())?,
        ("remove", _) => std::fs::remove_dir_all(from).map_err(|e| e.to_string())?,
        _ => return Err(format!("unknown operation {op}")),
    }
    Ok(true)
}

fn copy_dir(from: &Utf8Path, to: &Utf8Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name();
        let src = from.join(name.to_string_lossy().as_ref());
        let dst = to.join(name.to_string_lossy().as_ref());
        if e.file_type()?.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Runs the self test: prepares the scratch library and fixture, opens the windows and lets the
/// UI drive the steps. Returns the exit code when the message loop ends without a verdict.
pub fn run() -> i32 {
    let fixture = match crate::selftest_fixture::prepare() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("self test: couldn't prepare the fixture: {e}");
            return 2;
        }
    };
    let out = fixture.out.clone();
    let log = match std::fs::File::create(out.join("selftest.log")) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("self test: {e}");
            return 2;
        }
    };
    STATE.with(|s| *s.borrow_mut() = Some(SelfTest { out: out.clone(), log, failures: 0, steps: 0 }));
    let Some(id) = app::open_window(WindowKind::Project, Some(fixture.project.to_string()), None) else {
        return 2;
    };
    app::send_event(
        id,
        Topic::SelfTest,
        json!({"step": "start", "args": serde_json::to_value(&fixture).unwrap_or(Value::Null)}),
    );
    let code = app::message_loop();
    if code == 0 {
        1
    } else {
        code
    }
}
