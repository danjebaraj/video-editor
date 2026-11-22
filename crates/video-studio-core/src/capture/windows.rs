//! The Windows app-recording backend (ui/spec.md §23.7).
//!
//! - **Finding and opening apps:** windows are found with `EnumWindows` and named by their
//!   process (UWP apps through their frame's child). Apps open by executable or App Paths name,
//!   by AUMID (`Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`), or as an http(s) address.
//!   Windows come to the front through UI Automation, with `SetForegroundWindow` as fallback.
//! - **Input:** `SendInput`, with window-relative, DPI-aware coordinates (the process is per-monitor
//!   DPI aware, so one pixel of a screenshot is one pixel of input). `menu` uses UI Automation's
//!   ExpandCollapse and Invoke patterns on the app's menu bar.
//! - **Stills:** the screen region, through GDI. **Recordings:** a worker process (the helper
//!   started again with [`RECORD_WORKER`]) captures the window with Windows.Graphics.Capture and
//!   the app's own sound with WASAPI process loopback, encodes H.264 + AAC MP4 with the ffmpeg
//!   cutline installs, and writes the file only when it is complete.
//! - **Guard rails** beyond the shared ones: an elevated target is refused with a clear error,
//!   because Windows blocks input from a normal-integrity process to it.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, RECT, STILL_ACTIVE};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcess, GetCurrentThreadId, GetExitCodeProcess, OpenProcess, OpenProcessToken,
    QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    TreeScope_Descendants, UIA_ControlTypePropertyId, UIA_ExpandCollapsePatternId, UIA_InvokePatternId,
    UIA_MenuItemControlTypeId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{Backend, KeyCombo, Permissions, Pointer, Rect};
use crate::{Error, Result};

/// The helper's first argument when it is started as the recording worker.
pub const RECORD_WORKER: &str = "__capture-record-worker";

pub struct WinBackend;

// MARK: Windows and processes

fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn exe_stem(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut n = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok()?;
        let path = String::from_utf16_lossy(&buf[..n as usize]);
        Utf8Path::new(&path).file_stem().map(str::to_string)
    }
}

fn title(hwnd: HWND) -> String {
    let mut buf = vec![0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// The app a window belongs to, by its process name. UWP apps live in ApplicationFrameHost:
/// their real process owns a child window.
fn app_of(hwnd: HWND) -> String {
    let stem = exe_stem(pid_of(hwnd)).unwrap_or_default();
    if stem.eq_ignore_ascii_case("ApplicationFrameHost") {
        let mut found: Option<String> = None;
        unsafe extern "system" fn child(h: HWND, l: LPARAM) -> BOOL {
            let out = &mut *(l.0 as *mut (u32, Option<String>));
            let pid = pid_of(h);
            if pid != out.0 {
                out.1 = exe_stem(pid);
                return BOOL(0);
            }
            BOOL(1)
        }
        let mut state = (pid_of(hwnd), None::<String>);
        unsafe {
            let _ = EnumChildWindows(Some(hwnd), Some(child), LPARAM(&mut state as *mut _ as isize));
        }
        found = found.or(state.1);
        if let Some(f) = found {
            return f;
        }
    }
    stem
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4).is_ok() && cloaked != 0 }
}

/// Top-level windows a person would see: visible, not cloaked, not tool windows, with a title.
fn app_windows() -> Vec<HWND> {
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let list = &mut *(l.0 as *mut Vec<HWND>);
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        if IsWindowVisible(h).as_bool()
            && ex & WS_EX_TOOLWINDOW.0 == 0
            && GetWindowTextLengthW(h) > 0
            && GetWindow(h, GW_OWNER).map(|o| o.is_invalid()).unwrap_or(true)
            && !is_cloaked(h)
        {
            list.push(h);
        }
        BOOL(1)
    }
    let mut list: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

fn find_window(app: &str) -> Option<HWND> {
    app_windows().into_iter().find(|h| super::same_app(&app_of(*h), app) || super::same_app(&title(*h), app))
}

fn foreground() -> Option<HWND> {
    let h = unsafe { GetForegroundWindow() };
    if h.is_invalid() {
        return None;
    }
    Some(unsafe { GetAncestor(h, GA_ROOTOWNER) })
}

fn frame_bounds(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, 16).is_err() {
            GetWindowRect(hwnd, &mut r).ok()?;
        }
    }
    Some(Rect { x: r.left as f64, y: r.top as f64, w: (r.right - r.left) as f64, h: (r.bottom - r.top) as f64 })
}

/// Whether a process runs elevated (Windows blocks input to it from a normal app).
pub fn is_elevated(pid: u32) -> bool {
    unsafe {
        let proc = if pid == 0 {
            GetCurrentProcess()
        } else {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => h,
                Err(_) => return false,
            }
        };
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(proc, TOKEN_QUERY, &mut token).is_ok();
        if pid != 0 {
            let _ = CloseHandle(proc);
        }
        if !opened {
            // A token we can't even query belongs to a more privileged process.
            return pid != 0;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenElevation, Some(&mut e as *mut _ as *mut _), 4, &mut len).is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    }
}

fn check_not_elevated(hwnd: HWND) -> Result<()> {
    let pid = pid_of(hwnd);
    if !is_elevated(0) && is_elevated(pid) {
        return Err(Error::Denied(format!(
            "\"{}\" runs as administrator, and Windows doesn't let a normal app send it clicks or keys. Ask the user to start it normally.",
            app_of(hwnd)
        )));
    }
    Ok(())
}

fn uia() -> Result<IUIAutomation> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| Error::other(format!("UI Automation isn't available: {e}")))
    }
}

/// Brings a window to the front: UI Automation focus first, then the usual Win32 dance.
fn bring_to_front(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if let Ok(a) = uia() {
            if let Ok(el) = a.ElementFromHandle(hwnd) {
                let _ = el.SetFocus();
            }
        }
        if foreground() != Some(hwnd) {
            let fg = GetForegroundWindow();
            let fg_thread = GetWindowThreadProcessId(fg, None);
            let me = GetCurrentThreadId();
            let attached = fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
            // A tap of Alt lets this process move the foreground (the system's focus-steal rule).
            let alt = [key_input(VK_MENU.0, false), key_input(VK_MENU.0, true)];
            SendInput(&alt, std::mem::size_of::<INPUT>() as i32);
            let _ = SetForegroundWindow(hwnd);
            let _ = BringWindowToTop(hwnd);
            if attached {
                let _ = AttachThreadInput(me, fg_thread, false);
            }
        }
    }
}

fn shell_open(file: &str, params: Option<&str>) -> Result<()> {
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};
    let verb = HSTRING::from("open");
    let file_w = HSTRING::from(file);
    let params_w = params.map(HSTRING::from);
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: params_w.as_ref().map(|p| PCWSTR(p.as_ptr())).unwrap_or(PCWSTR::null()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut sei) }
        .map_err(|e| Error::NotFound(format!("couldn't open \"{file}\": {}", e.message())))
}

fn activate_aumid(aumid: &str) -> Result<()> {
    use windows::Win32::UI::Shell::{ApplicationActivationManager, IApplicationActivationManager, AO_NONE};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let m: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| Error::other(e.message().to_string()))?;
        m.ActivateApplication(&HSTRING::from(aumid), PCWSTR::null(), AO_NONE)
            .map(|_| ())
            .map_err(|e| Error::NotFound(format!("couldn't start {aumid}: {}", e.message())))
    }
}

// MARK: Input

/// A key event carrying its scan code as well, as a keyboard sends it: browsers derive
/// KeyboardEvent.code from the scan code, and web apps' shortcuts often match on it.
fn key_input(vk: u16, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
    // arrows and the navigation block are extended keys (0xE0 prefix)
    if matches!(vk, 0x21..=0x28 | 0x2D | 0x2E) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn unicode_input(unit: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: if up { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } else { KEYEVENTF_UNICODE },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn virtual_screen() -> (f64, f64, f64, f64) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN) as f64,
            GetSystemMetrics(SM_YVIRTUALSCREEN) as f64,
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as f64,
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as f64,
        )
    }
}

fn mouse_input(x: f64, y: f64, flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    let (vx, vy, vw, vh) = virtual_screen();
    let nx = ((x - vx) * 65535.0 / (vw - 1.0)).round() as i32;
    let ny = ((y - vy) * 65535.0 / (vh - 1.0)).round() as i32;
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: nx,
                dy: ny,
                mouseData: data as u32,
                dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send(inputs: &[INPUT]) -> Result<()> {
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if n as usize != inputs.len() {
        return Err(Error::other("Windows didn't accept the input (is a UAC prompt or an elevated window in front?)"));
    }
    Ok(())
}

fn cursor() -> (f64, f64) {
    let mut p = windows::Win32::Foundation::POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x as f64, p.y as f64)
}

/// Moves the pointer there in a short glide, so recordings show the movement.
fn glide(x: f64, y: f64) -> Result<()> {
    let (sx, sy) = cursor();
    let steps = 12;
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let e = t * t * (3.0 - 2.0 * t);
        send(&[mouse_input(sx + (x - sx) * e, sy + (y - sy) * e, MOUSEEVENTF_MOVE, 0)])?;
        std::thread::sleep(Duration::from_millis(12));
    }
    Ok(())
}

/// The virtual key for a key name from [`super::KEY_NAMES`], or a printable character.
fn vk_for(name: &str) -> Option<u16> {
    let k = match name {
        "return" => VK_RETURN,
        "tab" => VK_TAB,
        "space" => VK_SPACE,
        "delete" => VK_BACK,
        "forwarddelete" => VK_DELETE,
        "escape" => VK_ESCAPE,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        f if f.starts_with('f') && f.len() <= 3 && f[1..].parse::<u16>().is_ok() => {
            let n: u16 = f[1..].parse().ok()?;
            return (1..=12).contains(&n).then(|| VK_F1.0 + n - 1);
        }
        c if c.chars().count() == 1 => {
            let ch = c.chars().next()?;
            let r = unsafe { VkKeyScanW(ch as u16) };
            if r == -1 {
                return None;
            }
            return Some((r as u16) & 0xff);
        }
        _ => return None,
    };
    Some(k.0)
}

// MARK: UI Automation menus

fn named(a: &IUIAutomation, root: &IUIAutomationElement, name: &str) -> Option<IUIAutomationElement> {
    unsafe {
        let cond = a
            .CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &windows::Win32::System::Variant::VARIANT::from(UIA_MenuItemControlTypeId.0),
            )
            .ok()?;
        let all = root.FindAll(TreeScope_Descendants, &cond).ok()?;
        let n = all.Length().ok()?;
        let norm = |s: &str| s.replace('&', "").replace("...", "…").trim().to_lowercase();
        let want = norm(name);
        let mut prefix: Option<IUIAutomationElement> = None;
        for i in 0..n {
            let el = all.GetElement(i).ok()?;
            let label = norm(&el.CurrentName().map(|b| b.to_string()).unwrap_or_default());
            // "Save\tCtrl+S" in some menus: compare the label before the accelerator
            let label = label.split('\t').next().unwrap_or("").trim().to_string();
            if label == want {
                return Some(el);
            }
            if prefix.is_none() && label.starts_with(&want) {
                prefix = Some(el);
            }
        }
        prefix
    }
}

fn press(el: &IUIAutomationElement, last: bool) -> Result<()> {
    unsafe {
        if !last {
            if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId) {
                return p.Expand().map_err(|e| Error::other(e.message().to_string()));
            }
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
            return p.Invoke().map_err(|e| Error::other(e.message().to_string()));
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId) {
            return p.Expand().map_err(|e| Error::other(e.message().to_string()));
        }
    }
    Err(Error::other("the menu item can't be chosen"))
}

// MARK: Stills

/// A PNG of a screen region, one pixel per pixel.
fn grab(rect: Rect, out: &Utf8Path) -> Result<()> {
    let (x, y, w, h) = (rect.x as i32, rect.y as i32, rect.w as i32, rect.h as i32);
    if w <= 0 || h <= 0 {
        return Err(Error::other("the region is empty"));
    }
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let blit = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = GetDIBits(mem, bmp, 0, h as u32, Some(pixels.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        blit.map_err(|e| Error::other(format!("couldn't read the screen: {}", e.message())))?;
        if lines == 0 {
            return Err(Error::other("couldn't read the screen"));
        }
    }
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    image::save_buffer(out.as_std_path(), &pixels, w as u32, h as u32, image::ColorType::Rgba8)
        .map_err(|e| Error::other(format!("couldn't write {out}: {e}")))
}

// MARK: Backend

impl Backend for WinBackend {
    fn permissions(&self) -> Permissions {
        let capture = windows::Graphics::Capture::GraphicsCaptureSession::IsSupported().unwrap_or(false);
        Permissions { screen_recording: capture, accessibility: true, supported: true }
    }

    fn frontmost(&self) -> Result<String> {
        let h = foreground().ok_or_else(|| Error::other("no window is in front"))?;
        let name = app_of(h);
        if name.is_empty() {
            return Err(Error::other("couldn't tell which app is in front"));
        }
        Ok(name)
    }

    fn activate(&self, app: &str) -> Result<()> {
        if let Some(h) = find_window(app) {
            check_not_elevated(h)?;
            bring_to_front(h);
            return Ok(());
        }
        if app.contains('!') {
            activate_aumid(app)?;
        } else {
            shell_open(app, None).map_err(|_| {
                Error::NotFound(format!(
                    "There is no app \"{app}\" on this PC. Use its program name (notepad, msedge, winword) or its AUMID."
                ))
            })?;
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
            if let Some(h) = find_window(app).or_else(foreground) {
                if super::same_app(&app_of(h), app) || super::same_app(&title(h), app) {
                    check_not_elevated(h)?;
                    bring_to_front(h);
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn open_url(&self, url: &str, app: Option<&str>) -> Result<()> {
        match app {
            Some(a) => shell_open(a, Some(&format!("\"{url}\""))),
            None => shell_open(url, None),
        }
    }

    fn window(&self) -> Result<Rect> {
        let h = foreground().ok_or_else(|| Error::other("no window is in front"))?;
        frame_bounds(h).ok_or_else(|| Error::other("couldn't read the window's position"))
    }

    fn set_window(&self, size: Option<(f64, f64)>, at: Option<(f64, f64)>) -> Result<Rect> {
        let h = foreground().ok_or_else(|| Error::other("no window is in front"))?;
        check_not_elevated(h)?;
        let cur = frame_bounds(h).ok_or_else(|| Error::other("couldn't read the window's position"))?;
        let (x, y) = at.unwrap_or((cur.x, cur.y));
        let (w, hh) = size.unwrap_or((cur.w, cur.h));
        unsafe {
            if IsZoomed(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            // The extended frame excludes the invisible resize borders: account for them.
            let mut wr = RECT::default();
            let _ = GetWindowRect(h, &mut wr);
            let (bl, bt) = ((cur.x as i32) - wr.left, (cur.y as i32) - wr.top);
            let (bw, bh) = ((wr.right - wr.left) - cur.w as i32, (wr.bottom - wr.top) - cur.h as i32);
            SetWindowPos(
                h,
                None,
                x as i32 - bl,
                y as i32 - bt,
                w as i32 + bw,
                hh as i32 + bh,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .map_err(|e| Error::other(e.message().to_string()))?;
        }
        std::thread::sleep(Duration::from_millis(150));
        self.window()
    }

    fn screen(&self) -> Result<Rect> {
        unsafe {
            Ok(Rect {
                x: 0.0,
                y: 0.0,
                w: GetSystemMetrics(SM_CXSCREEN) as f64,
                h: GetSystemMetrics(SM_CYSCREEN) as f64,
            })
        }
    }

    fn screenshot(&self, rect: Rect, out: &Utf8Path) -> Result<()> {
        grab(rect, out)
    }

    fn pointer(&self, action: Pointer) -> Result<()> {
        if let Some(h) = foreground() {
            check_not_elevated(h)?;
        }
        match action {
            Pointer::Move { x, y } => glide(x, y),
            Pointer::Click { x, y, right, count } => {
                glide(x, y)?;
                let (down, up) = if right {
                    (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
                } else {
                    (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
                };
                for _ in 0..count.max(1) {
                    send(&[mouse_input(x, y, down, 0), mouse_input(x, y, up, 0)])?;
                    std::thread::sleep(Duration::from_millis(60));
                }
                Ok(())
            }
            Pointer::Drag { x, y, to_x, to_y } => {
                glide(x, y)?;
                send(&[mouse_input(x, y, MOUSEEVENTF_LEFTDOWN, 0)])?;
                std::thread::sleep(Duration::from_millis(80));
                glide(to_x, to_y)?;
                std::thread::sleep(Duration::from_millis(80));
                send(&[mouse_input(to_x, to_y, MOUSEEVENTF_LEFTUP, 0)])
            }
            Pointer::Scroll { x, y, lines, over: None } => {
                glide(x, y)?;
                send(&[mouse_input(x, y, MOUSEEVENTF_WHEEL, lines * WHEEL_DELTA as i32)])
            }
            Pointer::Scroll { x, y, lines, over: Some(over) } => {
                glide(x, y)?;
                // fractions of a notch (high-resolution wheel deltas), evenly over `over` seconds
                let (steps, gap) = super::scroll_plan(lines, over, WHEEL_DELTA as i32);
                for (k, d) in steps.iter().enumerate() {
                    if k > 0 {
                        std::thread::sleep(gap);
                    }
                    send(&[mouse_input(x, y, MOUSEEVENTF_WHEEL, *d)])?;
                }
                Ok(())
            }
        }
    }

    fn type_text(&self, text: &str) -> Result<()> {
        if let Some(h) = foreground() {
            check_not_elevated(h)?;
        }
        for ch in text.chars() {
            if ch == '\n' {
                send(&[key_input(VK_RETURN.0, false), key_input(VK_RETURN.0, true)])?;
            } else {
                let mut buf = [0u16; 2];
                for unit in ch.encode_utf16(&mut buf) {
                    send(&[unicode_input(*unit, false), unicode_input(*unit, true)])?;
                }
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        Ok(())
    }

    /// `cmd` (and `meta`, `win`) mean Ctrl on Windows, so the same steps work on both systems.
    fn key(&self, c: &KeyCombo) -> Result<()> {
        if let Some(h) = foreground() {
            check_not_elevated(h)?;
        }
        let vk = vk_for(&c.key).ok_or_else(|| Error::other(format!("\"{}\" isn't a key on this keyboard", c.key)))?;
        let mut mods: Vec<u16> = Vec::new();
        if c.command || c.control {
            mods.push(VK_CONTROL.0);
        }
        if c.shift {
            mods.push(VK_SHIFT.0);
        }
        if c.option {
            mods.push(VK_MENU.0);
        }
        let mut inputs: Vec<INPUT> = mods.iter().map(|m| key_input(*m, false)).collect();
        inputs.push(key_input(vk, false));
        inputs.push(key_input(vk, true));
        inputs.extend(mods.iter().rev().map(|m| key_input(*m, true)));
        send(&inputs)
    }

    fn menu(&self, path: &[String]) -> Result<()> {
        let h = foreground().ok_or_else(|| Error::other("no window is in front"))?;
        check_not_elevated(h)?;
        let a = uia()?;
        let root = unsafe { a.ElementFromHandle(h) }.map_err(|e| Error::other(e.message().to_string()))?;
        for (i, name) in path.iter().enumerate() {
            let last = i + 1 == path.len();
            // An opened menu is a popup of its own: search the desktop for later levels.
            let scope = if i == 0 {
                root.clone()
            } else {
                unsafe { a.GetRootElement() }.map_err(|e| Error::other(e.message().to_string()))?
            };
            let mut el = None;
            for _ in 0..20 {
                el = named(&a, &scope, name);
                if el.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let el = el.ok_or_else(|| {
                Error::NotFound(format!(
                    "There is no menu item \"{name}\" in {}. Check the menu names in a screenshot.",
                    app_of(h)
                ))
            })?;
            press(&el, last)?;
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(())
    }

    fn record_start(&self, rect: Rect, out: &Utf8Path, seconds: u32, _show_clicks: bool) -> Result<u32> {
        let fg = foreground();
        let window_frame = fg
            .and_then(frame_bounds)
            .map(|b| (b.x - rect.x).abs() < 2.0 && (b.w - rect.w).abs() < 2.0)
            .unwrap_or(false);
        let exe = std::env::current_exe().map_err(Error::from)?;
        let mut cmd = Command::new(exe);
        cmd.arg(RECORD_WORKER).args(["--out", out.as_str(), "--seconds", &seconds.to_string()]);
        match (window_frame, fg) {
            (true, Some(h)) => {
                cmd.args(["--hwnd", &(h.0 as isize).to_string(), "--pid", &pid_of(h).to_string()]);
            }
            _ => {
                cmd.args(["--screen"]);
                if let Some(h) = fg {
                    cmd.args(["--pid", &pid_of(h).to_string()]);
                }
            }
        }
        if let Some(f) = ffmpeg() {
            cmd.args(["--ffmpeg", f.as_str()]);
        } else {
            return Err(Error::Environment(
                "Recording needs ffmpeg: install cutline's dependencies (`cutline deps install --all`).".into(),
            ));
        }
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000 | 0x0000_0200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        }
        let started = started_marker(out);
        let _ = std::fs::remove_file(&started);
        let child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
        let pid = child.id();
        // The worker says when the first frame is captured; a refusal shows up as an early exit.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if started.exists() {
                return Ok(pid);
            }
            if !self.record_running(pid) {
                let why = std::fs::read_to_string(error_marker(out)).unwrap_or_default();
                return Err(Error::other(if why.is_empty() { "The recorder stopped right away.".into() } else { why }));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(pid)
    }

    fn record_running(&self, pid: u32) -> bool {
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code).is_ok();
            let _ = CloseHandle(h);
            ok && code == STILL_ACTIVE.0 as u32
        }
    }

    fn record_cancel(&self, pid: u32) {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let _ = TerminateProcess(h, 1);
                let _ = CloseHandle(h);
            }
        }
    }
}

fn started_marker(out: &Utf8Path) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{out}.recording"))
}

fn error_marker(out: &Utf8Path) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{out}.error"))
}

pub use super::ffmpeg;

// MARK: The recording worker

/// The recording worker's main: captures until the length is reached (or `<out>.stop` appears),
/// then writes `<out>`. Returns the exit code.
pub fn record_worker(args: &[String]) -> i32 {
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let Some(out) = get("--out").map(Utf8PathBuf::from) else { return 2 };
    let seconds: u32 = get("--seconds").and_then(|s| s.parse().ok()).unwrap_or(10);
    let hwnd = get("--hwnd").and_then(|s| s.parse::<isize>().ok()).map(|h| HWND(h as _));
    let pid: Option<u32> = get("--pid").and_then(|s| s.parse().ok());
    let Some(ffmpeg) = get("--ffmpeg").map(Utf8PathBuf::from) else { return 2 };
    match recorder::record(&out, seconds, hwnd, pid, &ffmpeg) {
        Ok(()) => 0,
        Err(e) => {
            let _ = std::fs::write(error_marker(&out), e.to_string());
            1
        }
    }
}

mod recorder {
    //! Windows.Graphics.Capture frames → raw BGRA → ffmpeg (H.264), with WASAPI process-loopback
    //! audio → WAV, muxed to MP4 at the end.

    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use camino::{Utf8Path, Utf8PathBuf};
    use windows::core::{implement, Interface, Ref, HRESULT};
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{HMODULE, HWND};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTOPRIMARY};
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CoInitializeEx, IAgileObject, IAgileObject_Impl, COINIT_MULTITHREADED};
    use windows::Win32::System::WinRT::Direct3D11::{
        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
    };
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

    use super::Utf8PathBufExt;
    use crate::{Error, Result};

    const FPS: u32 = 30;

    fn win(e: windows::core::Error) -> Error {
        Error::other(format!("{} (0x{:08x})", e.message(), e.code().0))
    }

    pub fn record(out: &Utf8Path, seconds: u32, hwnd: Option<HWND>, pid: Option<u32>, ffmpeg: &Utf8Path) -> Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let (device, context, d3d) = device()?;
        let item: GraphicsCaptureItem = unsafe {
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(win)?;
            match hwnd {
                Some(h) => interop.CreateForWindow(h).map_err(win)?,
                None => interop
                    .CreateForMonitor(MonitorFromWindow(GetDesktopWindow(), MONITOR_DEFAULTTOPRIMARY))
                    .map_err(win)?,
            }
        };
        let size = item.Size().map_err(win)?;
        let (w, h) = ((size.Width.max(2) & !1) as u32, (size.Height.max(2) & !1) as u32);
        let pool =
            Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)
                .map_err(win)?;
        let session = pool.CreateCaptureSession(&item).map_err(win)?;
        // No yellow border where Windows allows it, and the pointer in the picture.
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(true);
        session.StartCapture().map_err(win)?;

        let video_tmp = out.with_suffix(".video.mp4");
        let audio_tmp = out.with_suffix(".audio.wav");
        let mut enc = std::process::Command::new(ffmpeg.as_str());
        enc.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgra"])
            .args(["-s", &format!("{w}x{h}"), "-framerate", &FPS.to_string(), "-i", "pipe:0"])
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "18",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
            ])
            .arg(video_tmp.as_str())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            enc.creation_flags(0x0800_0000);
        }
        let mut child = enc.spawn().map_err(|e| Error::other(format!("couldn't start ffmpeg: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped");

        // The app's own sound, while the frames are captured.
        let stop = Arc::new(AtomicBool::new(false));
        let audio = pid.map(|p| {
            let stop = stop.clone();
            let path = audio_tmp.clone();
            std::thread::spawn(move || loopback(p, &path, &stop).is_ok())
        });

        let stop_file = out.with_suffix(".stop");
        let mut frame = vec![0u8; (w * h * 4) as usize];
        let mut staging: Option<(ID3D11Texture2D, u32, u32)> = None;
        let total = seconds * FPS;
        let start = Instant::now();
        let mut started = false;
        for i in 0..total {
            if let Ok(f) = pool.TryGetNextFrame() {
                let surface = f.Surface().map_err(win)?;
                let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(win)?;
                let tex: ID3D11Texture2D = unsafe { access.GetInterface().map_err(win)? };
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                unsafe { tex.GetDesc(&mut desc) };
                if staging.as_ref().map(|s| (s.1, s.2) != (desc.Width, desc.Height)).unwrap_or(true) {
                    let sd = D3D11_TEXTURE2D_DESC {
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        MiscFlags: 0,
                        ..desc
                    };
                    let mut t: Option<ID3D11Texture2D> = None;
                    unsafe { device.CreateTexture2D(&sd, None, Some(&mut t)).map_err(win)? };
                    staging = t.map(|t| (t, desc.Width, desc.Height));
                }
                if let Some((st, sw, sh)) = &staging {
                    unsafe {
                        context.CopyResource(st, &tex);
                        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                        context.Map(st, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(win)?;
                        let src =
                            std::slice::from_raw_parts(mapped.pData as *const u8, (mapped.RowPitch * sh) as usize);
                        // copy what fits; a window that grew is cropped, one that shrank leaves black
                        frame.fill(0);
                        let rows = (*sh).min(h) as usize;
                        let cols = ((*sw).min(w) * 4) as usize;
                        for r in 0..rows {
                            let s = r * mapped.RowPitch as usize;
                            let d = r * (w * 4) as usize;
                            frame[d..d + cols].copy_from_slice(&src[s..s + cols]);
                        }
                        context.Unmap(st, 0);
                    }
                }
                if !started {
                    started = true;
                    let _ = std::fs::write(super::started_marker(out), b"");
                }
            }
            if stdin.write_all(&frame).is_err() {
                break;
            }
            if stop_file.exists() {
                break;
            }
            // pace to real time
            let due = start + Duration::from_micros(((i + 1) as u64 * 1_000_000) / FPS as u64);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            if !started && start.elapsed() > Duration::from_secs(5) {
                return Err(Error::other("Windows.Graphics.Capture delivered no frames for this window."));
            }
        }
        let _ = session.Close();
        let _ = pool.Close();
        drop(stdin);
        let status = child.wait().map_err(|e| Error::other(e.to_string()))?;
        stop.store(true, Ordering::SeqCst);
        let have_audio = audio.map(|t| t.join().unwrap_or(false)).unwrap_or(false)
            && std::fs::metadata(&audio_tmp).map(|m| m.len() > 4096).unwrap_or(false);
        if !status.success() {
            return Err(Error::other("ffmpeg couldn't encode the recording"));
        }
        let finished = out.with_suffix(".tmp.mp4");
        if have_audio {
            let mut mux = std::process::Command::new(ffmpeg.as_str());
            mux.args(["-hide_banner", "-loglevel", "error", "-y", "-i", video_tmp.as_str(), "-i", audio_tmp.as_str()])
                .args([
                    "-map",
                    "0:v:0",
                    "-map",
                    "1:a:0",
                    "-c:v",
                    "copy",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "192k",
                    "-shortest",
                    "-movflags",
                    "+faststart",
                ])
                .arg(finished.as_str());
            {
                use std::os::windows::process::CommandExt;
                mux.creation_flags(0x0800_0000);
            }
            let ok = mux.status().map(|s| s.success()).unwrap_or(false);
            if !ok {
                std::fs::rename(&video_tmp, &finished).map_err(Error::from)?;
            }
        } else {
            std::fs::rename(&video_tmp, &finished).map_err(Error::from)?;
        }
        let _ = std::fs::remove_file(&video_tmp);
        let _ = std::fs::remove_file(&audio_tmp);
        std::fs::rename(&finished, out).map_err(Error::from)?;
        let _ = std::fs::remove_file(super::started_marker(out));
        let _ = std::fs::remove_file(&stop_file);
        Ok(())
    }

    fn device() -> Result<(ID3D11Device, ID3D11DeviceContext, IDirect3DDevice)> {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(win)?;
        }
        let device = device.ok_or_else(|| Error::other("no Direct3D device"))?;
        let context = context.ok_or_else(|| Error::other("no Direct3D context"))?;
        let dxgi: IDXGIDevice = device.cast().map_err(win)?;
        let insp = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi).map_err(win)? };
        Ok((device, context, insp.cast().map_err(win)?))
    }

    #[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
    struct Activated(std::sync::mpsc::Sender<()>);

    impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
        fn ActivateCompleted(&self, _op: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
            let _ = self.0.send(());
            Ok(())
        }
    }

    impl IAgileObject_Impl for Activated_Impl {}

    /// Captures the sound of one process tree (WASAPI process loopback, Windows 10 2004 and
    /// later) to a 48 kHz stereo 16-bit WAV until `stop`.
    fn loopback(pid: u32, out: &Utf8PathBuf, stop: &AtomicBool) -> Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let params = AUDIOCLIENT_ACTIVATION_PARAMS {
                ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                    ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                        TargetProcessId: pid,
                        ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                    },
                },
            };
            // PROPVARIANT { vt: VT_BLOB, blob: { cbSize, pBlobData } }. The blob points at `params`
            // on the stack: never let PropVariantClear (Drop) free it.
            let mut pv: std::mem::ManuallyDrop<PROPVARIANT> = std::mem::ManuallyDrop::new(std::mem::zeroed());
            {
                let raw = &mut *pv as *mut PROPVARIANT as *mut u8;
                *(raw as *mut u16) = 65; // VT_BLOB
                let blob = raw.add(8) as *mut u32;
                *blob = std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
                let ptr = raw.add(8 + std::mem::size_of::<usize>()) as *mut *const AUDIOCLIENT_ACTIVATION_PARAMS;
                *ptr = &params;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let handler: IActivateAudioInterfaceCompletionHandler = Activated(tx).into();
            let op = ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&*pv as *const PROPVARIANT),
                &handler,
            )
            .map_err(win)?;
            rx.recv_timeout(Duration::from_secs(5)).map_err(|_| Error::other("audio activation timed out"))?;
            let mut hr = HRESULT(0);
            let mut unk: Option<windows::core::IUnknown> = None;
            op.GetActivateResult(&mut hr, &mut unk).map_err(win)?;
            hr.ok().map_err(win)?;
            let client: IAudioClient = unk.ok_or_else(|| Error::other("no audio client"))?.cast().map_err(win)?;
            let fmt = WAVEFORMATEX {
                wFormatTag: 1, // PCM
                nChannels: 2,
                nSamplesPerSec: 48000,
                nAvgBytesPerSec: 48000 * 4,
                nBlockAlign: 4,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    2_000_000,
                    0,
                    &fmt,
                    None,
                )
                .map_err(win)?;
            let capture: IAudioCaptureClient = client.GetService().map_err(win)?;
            client.Start().map_err(win)?;
            let mut file = std::fs::File::create(out).map_err(Error::from)?;
            file.write_all(&[0u8; 44]).map_err(Error::from)?;
            let mut bytes: u32 = 0;
            let started = Instant::now();
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
                loop {
                    let n = capture.GetNextPacketSize().unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    let mut data: *mut u8 = std::ptr::null_mut();
                    let mut frames = 0u32;
                    let mut flags = 0u32;
                    if capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
                        break;
                    }
                    let len = frames as usize * 4;
                    let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                    if silent || data.is_null() {
                        file.write_all(&vec![0u8; len]).map_err(Error::from)?;
                    } else {
                        file.write_all(std::slice::from_raw_parts(data, len)).map_err(Error::from)?;
                    }
                    bytes += len as u32;
                    let _ = capture.ReleaseBuffer(frames);
                }
                // Process loopback delivers nothing while the app is silent: keep the track's
                // length in step with the picture.
                let expected = (started.elapsed().as_secs_f64() * 48000.0) as u32 * 4;
                if expected > bytes + 48000 {
                    let pad = (expected - bytes) as usize;
                    file.write_all(&vec![0u8; pad]).map_err(Error::from)?;
                    bytes += pad as u32;
                }
            }
            let _ = client.Stop();
            // RIFF header
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(0)).map_err(Error::from)?;
            let mut hdr = Vec::with_capacity(44);
            hdr.extend_from_slice(b"RIFF");
            hdr.extend_from_slice(&(36 + bytes).to_le_bytes());
            hdr.extend_from_slice(b"WAVEfmt ");
            hdr.extend_from_slice(&16u32.to_le_bytes());
            hdr.extend_from_slice(&1u16.to_le_bytes());
            hdr.extend_from_slice(&2u16.to_le_bytes());
            hdr.extend_from_slice(&48000u32.to_le_bytes());
            hdr.extend_from_slice(&(48000u32 * 4).to_le_bytes());
            hdr.extend_from_slice(&4u16.to_le_bytes());
            hdr.extend_from_slice(&16u16.to_le_bytes());
            hdr.extend_from_slice(b"data");
            hdr.extend_from_slice(&bytes.to_le_bytes());
            file.write_all(&hdr).map_err(Error::from)?;
        }
        Ok(())
    }
}

trait Utf8PathBufExt {
    fn with_suffix(&self, suffix: &str) -> Utf8PathBuf;
}

impl Utf8PathBufExt for Utf8Path {
    fn with_suffix(&self, suffix: &str) -> Utf8PathBuf {
        Utf8PathBuf::from(format!("{self}{suffix}"))
    }
}
