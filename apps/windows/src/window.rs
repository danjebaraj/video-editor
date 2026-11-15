//! One top-level window: a project, the App Library, Settings or the start window. The client
//! area is a WebView2 control; the frame has the Mica backdrop, the system theme's title bar and
//! (for project windows) the Win32 menu bar.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use camino::Utf8PathBuf;
use serde_json::{json, Value};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app;
use crate::protocol::{Topic, WindowKind};

pub struct AppWindow {
    id: u64,
    kind: WindowKind,
    hwnd: HWND,
    path: RefCell<Option<Utf8PathBuf>>,
    options: Option<Value>,
    title: RefCell<String>,
    dirty: Cell<bool>,
    pub(crate) controller: RefCell<Option<ICoreWebView2Controller>>,
    pub(crate) webview: RefCell<Option<ICoreWebView2>>,
    /// The UI has loaded and answers close requests itself.
    pub(crate) ready: Cell<bool>,
    closing: Cell<bool>,
    /// Messages for the UI sent before the WebView was ready.
    pending: RefCell<Vec<String>>,
}

const CLASS: windows::core::PCWSTR = w!("SyncfusionVideoStudioWindow");

fn register_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            hIcon: LoadIconW(Some(instance.into()), windows::core::PCWSTR(1 as _)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            // Black lets DWM draw the Mica backdrop under the (transparent) WebView.
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                windows::Win32::Graphics::Gdi::GetStockObject(windows::Win32::Graphics::Gdi::BLACK_BRUSH).0,
            ),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassW(&class);
    });
}

impl AppWindow {
    pub fn create(
        id: u64,
        kind: WindowKind,
        path: Option<Utf8PathBuf>,
        options: Option<Value>,
    ) -> windows::core::Result<Rc<AppWindow>> {
        register_class();
        let (w, h) = match kind {
            WindowKind::Project => (1440, 900),
            WindowKind::Library => (1000, 700),
            WindowKind::Settings => (820, 640),
            WindowKind::Start => (860, 560),
        };
        let title = match (&kind, &path) {
            (WindowKind::Project, Some(p)) => {
                format!("{} — Syncfusion Video Studio", p.file_stem().unwrap_or("Project"))
            }
            (WindowKind::Project, None) => "Untitled — Syncfusion Video Studio".into(),
            (WindowKind::Library, _) => "App Library — Syncfusion Video Studio".into(),
            (WindowKind::Settings, _) => "Settings — Syncfusion Video Studio".into(),
            (WindowKind::Start, _) => "Syncfusion Video Studio".into(),
        };
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None)?;
            let menu = if kind == WindowKind::Project { Some(crate::menu::project_menu()) } else { None };
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS,
                &HSTRING::from(title.as_str()),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                w,
                h,
                None,
                menu,
                Some(instance.into()),
                None,
            )?;
            // Sizes above are at 96 DPI: scale to the monitor the window opened on.
            let scale = GetDpiForWindow(hwnd) as f64 / 96.0;
            if scale > 1.0 {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    (w as f64 * scale) as i32,
                    (h as f64 * scale) as i32,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, id as isize);
            hwnd
        };
        let win = Rc::new(AppWindow {
            id,
            kind,
            hwnd,
            path: RefCell::new(path),
            options,
            title: RefCell::new(title),
            dirty: Cell::new(false),
            controller: RefCell::new(None),
            webview: RefCell::new(None),
            ready: Cell::new(false),
            closing: Cell::new(false),
            pending: RefCell::new(Vec::new()),
        });
        win.apply_frame_theme();
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        }
        crate::webview::attach(&win)?;
        Ok(win)
    }

    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn kind(&self) -> WindowKind {
        self.kind
    }
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
    pub fn path(&self) -> Option<Utf8PathBuf> {
        self.path.borrow().clone()
    }
    pub fn options(&self) -> Option<Value> {
        self.options.clone()
    }

    pub fn set_path(&self, p: Option<Utf8PathBuf>) {
        *self.path.borrow_mut() = p;
    }

    pub fn activate(&self) {
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn set_title(&self, title: &str) {
        *self.title.borrow_mut() = title.to_string();
        self.refresh_title();
    }

    pub fn set_dirty(&self, dirty: bool) {
        self.dirty.set(dirty);
        self.refresh_title();
    }

    fn refresh_title(&self) {
        let t = if self.dirty.get() { format!("• {}", self.title.borrow()) } else { self.title.borrow().clone() };
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(t));
        }
    }

    /// Sends one JSON message to the web UI (queued until the page is ready).
    pub fn post_json(&self, text: &str) {
        if !self.ready.get() {
            self.pending.borrow_mut().push(text.to_string());
            return;
        }
        if let Some(wv) = self.webview.borrow().as_ref() {
            unsafe {
                let _ = wv.PostWebMessageAsJson(&HSTRING::from(text));
            }
        }
    }

    /// The page said it is ready: deliver what was queued.
    pub fn mark_ready(&self) {
        self.ready.set(true);
        let pending: Vec<String> = std::mem::take(&mut *self.pending.borrow_mut());
        for p in pending {
            self.post_json(&p);
        }
    }

    pub fn event(&self, topic: Topic, data: Value) {
        let text = serde_json::to_string(&crate::protocol::HostMessage::Event { topic, data }).unwrap_or_default();
        self.post_json(&text);
    }

    /// Closes for real (the UI has saved).
    pub fn close_now(&self) {
        self.closing.set(true);
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    fn resize_webview(&self) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        if let Some(c) = self.controller.borrow().as_ref() {
            unsafe {
                let _ = c.SetBounds(rc);
            }
        }
    }

    pub fn scale(&self) -> f64 {
        unsafe { GetDpiForWindow(self.hwnd) as f64 / 96.0 }
    }

    /// Mica backdrop and a title bar that follows the system's light or dark theme.
    pub fn apply_frame_theme(&self) {
        let theme = crate::shell::theme(self.scale());
        unsafe {
            let dark: i32 = if theme.dark && !theme.high_contrast { 1 } else { 0 };
            let _ = DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as *const _,
                std::mem::size_of::<i32>() as u32,
            );
            // DWMSBT_MAINWINDOW (Mica); none in high contrast, where the theme's colors rule.
            let backdrop: i32 = if theme.high_contrast { 1 } else { 2 };
            let _ = DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &backdrop as *const _ as *const _,
                std::mem::size_of::<i32>() as u32,
            );
            // The backdrop reaches the client area (under the transparent WebView) only when the
            // frame is extended over all of it.
            let margins = windows::Win32::UI::Controls::MARGINS {
                cxLeftWidth: -1,
                cxRightWidth: -1,
                cyTopHeight: -1,
                cyBottomHeight: -1,
            };
            let _ = windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea(self.hwnd, &margins);
        }
    }

    fn theme_changed(&self) {
        self.apply_frame_theme();
        let t = crate::shell::theme(self.scale());
        self.event(Topic::Theme, serde_json::to_value(t).unwrap_or(Value::Null));
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let id = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as u64;
    let win = if id != 0 { app::window(id) } else { None };
    if let Some(w) = win {
        match msg {
            WM_SIZE => {
                w.resize_webview();
                return LRESULT(0);
            }
            WM_MOVE | WM_MOVING => {
                if let Some(c) = w.controller.borrow().as_ref() {
                    unsafe {
                        let _ = c.NotifyParentWindowPositionChanged();
                    }
                }
            }
            WM_SETFOCUS => {
                if let Some(c) = w.controller.borrow().as_ref() {
                    unsafe {
                        let _ = c.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
                    }
                }
            }
            WM_GETMINMAXINFO => {
                let mmi = lparam.0 as *mut MINMAXINFO;
                let s = w.scale();
                unsafe {
                    (*mmi).ptMinTrackSize.x = (720.0 * s) as i32;
                    (*mmi).ptMinTrackSize.y = (480.0 * s) as i32;
                }
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let r = unsafe { &*(lparam.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                w.theme_changed();
                return LRESULT(0);
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
                w.theme_changed();
            }
            WM_COMMAND => {
                let cmd = (wparam.0 & 0xffff) as u16;
                if let Some(name) = crate::menu::command_name(cmd) {
                    w.event(Topic::Menu, json!({"command": name}));
                    return LRESULT(0);
                }
            }
            WM_CLOSE => {
                if w.ready.get() && !w.closing.get() {
                    // The UI saves (autosave flushes on close), then calls shell.close_window.
                    w.event(Topic::CloseRequested, json!({}));
                    return LRESULT(0);
                }
            }
            WM_DESTROY => {
                if let Some(c) = w.controller.borrow_mut().take() {
                    unsafe {
                        let _ = c.Close();
                    }
                }
                w.webview.borrow_mut().take();
                crate::shell::clear_window(w.id());
                app::window_closed(w.id());
                return LRESULT(0);
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
