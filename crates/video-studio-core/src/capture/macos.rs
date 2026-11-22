//! macOS backend: Quartz events for the pointer and keyboard, System Events (through
//! `osascript`) for windows and menus, and `screencapture` for stills and video. Everything used
//! here ships with macOS.
//!
//! Scripts are fixed text; values travel as `osascript` arguments, never spliced into a script.

use std::ffi::c_void;
use std::process::{Command, Stdio};
use std::time::Duration;

use camino::Utf8Path;

use super::{Backend, KeyCombo, Permissions, Pointer, Rect};
use crate::{Error, Result};

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventCreateMouseEvent(source: *const c_void, kind: u32, position: CGPoint, button: u32) -> *mut c_void;
    fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> *mut c_void;
    fn CGEventCreateScrollWheelEvent(source: *const c_void, units: u32, wheels: u32, wheel1: i32, ...) -> *mut c_void;
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventKeyboardSetUnicodeString(event: *mut c_void, length: std::ffi::c_ulong, text: *const u16);
    fn CGEventSetIntegerValueField(event: *mut c_void, field: u32, value: i64);
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn AXIsProcessTrusted() -> bool;
    fn CFRelease(object: *const c_void);
}

const LEFT_DOWN: u32 = 1;
const LEFT_UP: u32 = 2;
const RIGHT_DOWN: u32 = 3;
const RIGHT_UP: u32 = 4;
const MOVED: u32 = 5;
const LEFT_DRAGGED: u32 = 6;
const CLICK_STATE_FIELD: u32 = 1;
const FLAG_SHIFT: u64 = 0x0002_0000;
const FLAG_CONTROL: u64 = 0x0004_0000;
const FLAG_OPTION: u64 = 0x0008_0000;
const FLAG_COMMAND: u64 = 0x0010_0000;

fn post(event: *mut c_void) {
    if event.is_null() {
        return;
    }
    // SAFETY: `event` is a CGEvent we just created and own; it is posted, then released once.
    unsafe {
        CGEventPost(0, event);
        CFRelease(event);
    }
}

fn mouse(kind: u32, x: f64, y: f64, button: u32, clicks: i64) {
    // SAFETY: a null source is allowed; the returned event is released in `post`.
    let e = unsafe { CGEventCreateMouseEvent(std::ptr::null(), kind, CGPoint { x, y }, button) };
    if !e.is_null() && clicks > 0 {
        // SAFETY: `e` is a valid mouse event.
        unsafe { CGEventSetIntegerValueField(e, CLICK_STATE_FIELD, clicks) };
    }
    post(e);
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Moves the pointer along a short path so recordings show it travel instead of jumping.
fn glide(to_x: f64, to_y: f64, kind: u32) {
    let (fx, fy) = pointer_position().unwrap_or((to_x, to_y));
    let steps = 18;
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ease = t * t * (3.0 - 2.0 * t);
        mouse(kind, fx + (to_x - fx) * ease, fy + (to_y - fy) * ease, 0, 0);
        pause(12);
    }
}

fn pointer_position() -> Option<(f64, f64)> {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    }
    // SAFETY: CGEventCreate with a null source returns an event carrying the pointer's location.
    unsafe {
        let e = CGEventCreate(std::ptr::null());
        if e.is_null() {
            return None;
        }
        let p = CGEventGetLocation(e);
        CFRelease(e);
        Some((p.x, p.y))
    }
}

/// ANSI virtual key codes.
#[rustfmt::skip]
fn key_code(key: &str) -> Option<u16> {
    Some(match key {
        "a" => 0, "s" => 1, "d" => 2, "f" => 3, "h" => 4, "g" => 5, "z" => 6, "x" => 7, "c" => 8, "v" => 9,
        "b" => 11, "q" => 12, "w" => 13, "e" => 14, "r" => 15, "y" => 16, "t" => 17, "1" => 18, "2" => 19,
        "3" => 20, "4" => 21, "6" => 22, "5" => 23, "=" | "+" => 24, "9" => 25, "7" => 26, "-" => 27, "8" => 28,
        "0" => 29, "]" => 30, "o" => 31, "u" => 32, "[" => 33, "i" => 34, "p" => 35, "return" => 36, "l" => 37,
        "j" => 38, "'" => 39, "k" => 40, ";" => 41, "\\" => 42, "," => 43, "/" | "?" => 44, "n" => 45, "m" => 46,
        "." => 47, "tab" => 48, "space" => 49, "`" => 50, "delete" => 51, "escape" => 53, "f5" => 96, "f6" => 97,
        "f7" => 98, "f3" => 99, "f8" => 100, "f9" => 101, "f11" => 103, "f10" => 109, "f12" => 111, "home" => 115,
        "pageup" => 116, "forwarddelete" => 117, "f4" => 118, "end" => 119, "f2" => 120, "pagedown" => 121,
        "f1" => 122, "left" => 123, "right" => 124, "down" => 125, "up" => 126,
        _ => return None,
    })
}

fn key_event(code: u16, down: bool, flags: u64, text: Option<&[u16]>) {
    // SAFETY: a null source is allowed; the event is released in `post`.
    let e = unsafe { CGEventCreateKeyboardEvent(std::ptr::null(), code, down) };
    if e.is_null() {
        return;
    }
    // SAFETY: `e` is a valid keyboard event; `text` outlives the call.
    unsafe {
        CGEventSetFlags(e, flags);
        if let Some(t) = text {
            CGEventKeyboardSetUnicodeString(e, t.len() as std::ffi::c_ulong, t.as_ptr());
        }
    }
    post(e);
}

fn run(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(Error::other(if err.is_empty() { format!("{program} failed ({})", out.status) } else { err }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Runs a fixed AppleScript with `args` as its `argv`.
fn script(source: &str, args: &[&str]) -> Result<String> {
    let mut a = vec!["-e", source, "--"];
    a.extend_from_slice(args);
    run("/usr/bin/osascript", &a).map_err(|e| {
        let m = e.to_string();
        if m.contains("-1719") || m.contains("-25211") || m.contains("assistive access") {
            Error::Environment(super::ACCESSIBILITY_HELP.into())
        } else if m.contains("-1743") {
            Error::Environment("macOS blocked control of System Events. Tell the user: open System Settings › Privacy & Security › Automation and allow Syncfusion Video Studio (or the terminal) to control System Events.".into())
        } else {
            Error::other(m.rsplit("execution error: ").next().unwrap_or(&m).to_string())
        }
    })
}

const FRONT: &str = "tell application \"System Events\" to tell (first application process whose frontmost is true)";

fn parse_rect(s: &str) -> Result<Rect> {
    let n: Vec<f64> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    match n.as_slice() {
        [x, y, w, h] => Ok(Rect { x: *x, y: *y, w: *w, h: *h }),
        _ => Err(Error::other(format!("couldn't read the window's position ({s})"))),
    }
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Pixels in one scroll line, for timed scrolls in pixel units.
const PIXELS_PER_LINE: i32 = 10;

pub struct MacBackend;

impl Backend for MacBackend {
    fn permissions(&self) -> Permissions {
        // SAFETY: both calls only read the process's privacy grants.
        unsafe {
            Permissions {
                screen_recording: CGPreflightScreenCaptureAccess(),
                accessibility: AXIsProcessTrusted(),
                supported: true,
            }
        }
    }

    fn request_permissions(&self) -> Permissions {
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        }
        if !self.permissions().screen_recording {
            // SAFETY: shows the system's prompt (once per app) and returns the current grant.
            unsafe { CGRequestScreenCaptureAccess() };
        }
        if !self.permissions().accessibility {
            // The prompt needs an options dictionary; opening the settings pane works everywhere.
            // SAFETY: a null options pointer is allowed and only checks the grant.
            unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) };
            let _ = run(
                "/usr/bin/open",
                &["x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"],
            );
        }
        self.permissions()
    }

    fn frontmost(&self) -> Result<String> {
        let asn = run("/usr/bin/lsappinfo", &["front"])?;
        let info = run("/usr/bin/lsappinfo", &["info", "-only", "name", &asn])?;
        // `"LSDisplayName"="Safari"`, or on some systems `"Safari" ASN:…`.
        let quoted: Vec<&str> = info.split('"').collect();
        let name = match quoted.as_slice() {
            [_, key, _, value, ..] if key.contains("Name") => value,
            [_, first, ..] => first,
            _ => "",
        };
        if name.is_empty() {
            return Err(Error::other("couldn't tell which app is in front"));
        }
        Ok(name.to_string())
    }

    fn activate(&self, app: &str) -> Result<()> {
        run("/usr/bin/open", &["-a", app]).map(|_| ()).map_err(|_| {
            Error::NotFound(format!(
                "There is no app named \"{app}\" on this Mac. Use the name shown in the Applications folder."
            ))
        })
    }

    fn open_url(&self, url: &str, app: Option<&str>) -> Result<()> {
        match app {
            Some(a) => run("/usr/bin/open", &["-a", a, url]),
            None => run("/usr/bin/open", &[url]),
        }
        .map(|_| ())
    }

    fn window(&self) -> Result<Rect> {
        let source = format!(
            "on run argv\n{FRONT}\nif (count of windows) is 0 then error \"The app has no window open.\"\nset w to window 1\nset p to position of w\nset s to size of w\nreturn ((item 1 of p) as text) & \",\" & (item 2 of p) & \",\" & (item 1 of s) & \",\" & (item 2 of s)\nend tell\nend run"
        );
        parse_rect(&script(&source, &[])?)
    }

    fn set_window(&self, size: Option<(f64, f64)>, at: Option<(f64, f64)>) -> Result<Rect> {
        let source = format!(
            "on run argv\n{FRONT}\nif (count of windows) is 0 then error \"The app has no window open.\"\nset w to window 1\nif (item 1 of argv) is not \"\" then set position of w to {{(item 1 of argv) as integer, (item 2 of argv) as integer}}\nif (item 3 of argv) is not \"\" then set size of w to {{(item 3 of argv) as integer, (item 4 of argv) as integer}}\nend tell\nend run"
        );
        let n = |v: Option<f64>| v.map(|v| format!("{}", v.round() as i64)).unwrap_or_default();
        let args = [n(at.map(|a| a.0)), n(at.map(|a| a.1)), n(size.map(|s| s.0)), n(size.map(|s| s.1))];
        script(&source, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
        pause(350);
        self.window()
    }

    fn screen(&self) -> Result<Rect> {
        // SAFETY: both calls only read display geometry.
        let b = unsafe { CGDisplayBounds(CGMainDisplayID()) };
        Ok(Rect { x: b.origin.x, y: b.origin.y, w: b.size.width, h: b.size.height })
    }

    fn screenshot(&self, rect: Rect, out: &Utf8Path) -> Result<()> {
        let r = format!("{},{},{},{}", rect.x.round(), rect.y.round(), rect.w.round(), rect.h.round());
        let _ = std::fs::remove_file(out);
        run("/usr/sbin/screencapture", &["-x", "-R", &r, out.as_str()])?;
        if !out.is_file() {
            return Err(Error::other("The screenshot wasn't written."));
        }
        // Retina displays capture two pixels per point: scale down so pixels are points.
        let width = format!("{}", rect.w.round() as i64);
        let _ = run("/usr/bin/sips", &["--resampleWidth", &width, out.as_str()]);
        Ok(())
    }

    fn pointer(&self, action: Pointer) -> Result<()> {
        match action {
            Pointer::Move { x, y } => glide(x, y, MOVED),
            Pointer::Click { x, y, right, count } => {
                glide(x, y, MOVED);
                pause(80);
                let (down, up, button) = if right { (RIGHT_DOWN, RIGHT_UP, 1) } else { (LEFT_DOWN, LEFT_UP, 0) };
                for n in 1..=count.max(1) as i64 {
                    mouse(down, x, y, button, n);
                    pause(40);
                    mouse(up, x, y, button, n);
                    pause(70);
                }
            }
            Pointer::Drag { x, y, to_x, to_y } => {
                glide(x, y, MOVED);
                pause(80);
                mouse(LEFT_DOWN, x, y, 0, 1);
                pause(120);
                glide(to_x, to_y, LEFT_DRAGGED);
                pause(120);
                mouse(LEFT_UP, to_x, to_y, 0, 1);
            }
            Pointer::Scroll { x, y, lines, over: None } => {
                glide(x, y, MOVED);
                let step = if lines < 0 { -1 } else { 1 };
                for _ in 0..lines.abs() {
                    // SAFETY: one wheel, line units; the event is released in `post`.
                    post(unsafe { CGEventCreateScrollWheelEvent(std::ptr::null(), 1, 1, step) });
                    pause(25);
                }
            }
            Pointer::Scroll { x, y, lines, over: Some(over) } => {
                glide(x, y, MOVED);
                // pixel units (0), the system's 10 points a line, evenly over `over` seconds
                let (steps, gap) = super::scroll_plan(lines, over, PIXELS_PER_LINE);
                for (k, d) in steps.iter().enumerate() {
                    if k > 0 {
                        std::thread::sleep(gap);
                    }
                    // SAFETY: one wheel, pixel units; the event is released in `post`.
                    post(unsafe { CGEventCreateScrollWheelEvent(std::ptr::null(), 0, 1, *d) });
                }
            }
        }
        pause(120);
        Ok(())
    }

    fn type_text(&self, text: &str) -> Result<()> {
        // One character at a time, at a pace that reads as typing in a recording.
        for ch in text.chars() {
            match ch {
                '\n' => {
                    key_event(36, true, 0, None);
                    key_event(36, false, 0, None);
                }
                '\t' => {
                    key_event(48, true, 0, None);
                    key_event(48, false, 0, None);
                }
                '\r' => {}
                _ => {
                    let mut buf = [0u16; 2];
                    let units = ch.encode_utf16(&mut buf);
                    key_event(0, true, 0, Some(units));
                    key_event(0, false, 0, Some(units));
                }
            }
            pause(32);
        }
        pause(100);
        Ok(())
    }

    fn key(&self, c: &KeyCombo) -> Result<()> {
        let code = key_code(&c.key)
            .ok_or_else(|| Error::other(format!("\"{}\" has no key on this keyboard layout.", c.key)))?;
        let mut flags = 0;
        if c.command {
            flags |= FLAG_COMMAND;
        }
        if c.shift || c.key == "+" || c.key == "?" {
            flags |= FLAG_SHIFT;
        }
        if c.control {
            flags |= FLAG_CONTROL;
        }
        if c.option {
            flags |= FLAG_OPTION;
        }
        key_event(code, true, flags, None);
        pause(40);
        key_event(code, false, flags, None);
        pause(150);
        Ok(())
    }

    fn menu(&self, path: &[String]) -> Result<()> {
        let source = format!(
            "on run argv\n{FRONT}\nset m to menu 1 of menu bar item (item 1 of argv) of menu bar 1\nrepeat with i from 2 to ((count of argv) - 1)\nset m to menu 1 of menu item (item i of argv) of m\nend repeat\nclick menu item (item -1 of argv) of m\nend tell\nend run"
        );
        script(&source, &path.iter().map(String::as_str).collect::<Vec<_>>()).map(|_| ()).map_err(|e| match e {
            Error::Environment(_) => e,
            other => Error::NotFound(format!(
                "Couldn't choose {} ({other}). Menu and item names must match exactly, including the … character; take a screenshot of the open menu to check.",
                path.join(" › ")
            )),
        })?;
        pause(300);
        Ok(())
    }

    fn record_start(&self, rect: Rect, out: &Utf8Path, seconds: u32, show_clicks: bool) -> Result<u32> {
        let r = format!("{},{},{},{}", rect.x.round(), rect.y.round(), rect.w.round(), rect.h.round());
        let length = seconds.to_string();
        let mut args = vec!["-x", "-v", "-V", &length, "-R", &r];
        if show_clicks {
            args.push("-k");
        }
        args.push(out.as_str());
        let child = Command::new("/usr/sbin/screencapture")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let pid = child.id();
        // The recorder needs a moment before the first frame; a refusal shows up as an early exit.
        pause(1000);
        if !alive(pid) {
            return Err(Error::other("The screen recorder stopped right away."));
        }
        // Dropping `child` leaves the recorder running; it ends by itself after `seconds`.
        drop(child);
        Ok(pid)
    }

    fn record_running(&self, pid: u32) -> bool {
        pid != 0 && alive(pid)
    }

    fn record_cancel(&self, pid: u32) {
        if pid != 0 && alive(pid) {
            // SAFETY: ends the recorder this module started; screencapture has no clean early stop.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_codes_cover_the_named_keys() {
        for k in super::super::KEY_NAMES.iter().filter(|k| !matches!(**k, "enter" | "esc")) {
            assert!(key_code(k).is_some(), "{k}");
        }
        for ch in "abcdefghijklmnopqrstuvwxyz0123456789-=[];',./\\`".chars() {
            assert!(key_code(&ch.to_string()).is_some(), "{ch}");
        }
        assert_eq!(parse_rect("10, 20, 1280, 800").unwrap(), Rect { x: 10.0, y: 20.0, w: 1280.0, h: 800.0 });
        assert!(parse_rect("nope").is_err());
    }
}
