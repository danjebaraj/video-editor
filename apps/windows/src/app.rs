//! Start-up, the window registry and the UI-thread dispatcher.
//!
//! WebView2 objects live on the UI thread. Work that finishes elsewhere (core calls, run and
//! assistant events, the single-instance pipe, file watching) is queued with [`post`] and runs
//! on the UI thread when the hidden dispatcher window gets `WM_APP`.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use camino::Utf8PathBuf;
use serde_json::Value;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, HWND_MESSAGE, MB_ICONERROR, MB_OK, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WNDCLASSW,
};

use crate::protocol::{HostMessage, Topic, WindowKind};
use crate::window::AppWindow;

const WM_APP_RUN: u32 = WM_APP + 1;

static DISPATCH_HWND: AtomicIsize = AtomicIsize::new(0);
static NEXT_WINDOW: AtomicU64 = AtomicU64::new(1);

type Job = Box<dyn FnOnce() + Send>;

fn queue() -> &'static Mutex<VecDeque<Job>> {
    static Q: OnceLock<Mutex<VecDeque<Job>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Runs `f` on the UI thread, soon. Callable from any thread.
pub fn post(f: impl FnOnce() + Send + 'static) {
    queue().lock().unwrap().push_back(Box::new(f));
    let h = DISPATCH_HWND.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as _)), WM_APP_RUN, WPARAM(0), LPARAM(0));
        }
    }
}

/// Sends a message to a window's web UI. Callable from any thread.
pub fn send(window_id: u64, msg: HostMessage) {
    let text = serde_json::to_string(&msg).unwrap_or_default();
    post(move || {
        if let Some(w) = window(window_id) {
            w.post_json(&text);
        }
    });
}

pub fn send_event(window_id: u64, topic: Topic, data: Value) {
    send(window_id, HostMessage::Event { topic, data });
}

thread_local! {
    static WINDOWS: RefCell<HashMap<u64, Rc<AppWindow>>> = RefCell::new(HashMap::new());
    static SETTINGS: RefCell<Options> = RefCell::new(Options::default());
}

#[derive(Default, Clone)]
pub struct Options {
    pub selftest: bool,
    pub debug: bool,
}

pub fn options() -> Options {
    SETTINGS.with(|s| s.borrow().clone())
}

pub fn window(id: u64) -> Option<Rc<AppWindow>> {
    WINDOWS.with(|w| w.borrow().get(&id).cloned())
}

pub fn windows() -> Vec<Rc<AppWindow>> {
    WINDOWS.with(|w| w.borrow().values().cloned().collect())
}

/// Opens a window. A project that is already open is brought to the front instead.
pub fn open_window(kind: WindowKind, path: Option<String>, options: Option<Value>) -> Option<u64> {
    if let Some(p) = &path {
        if let Some(w) = windows().into_iter().find(|w| {
            w.kind() == kind
                && w.path()
                    .map(|wp| video_studio_core::paths::same_path(wp.as_str().into(), p.as_str().into()))
                    .unwrap_or(false)
        }) {
            w.activate();
            return Some(w.id());
        }
    }
    if matches!(kind, WindowKind::Library | WindowKind::Settings | WindowKind::Start) {
        if let Some(w) = windows().into_iter().find(|w| w.kind() == kind) {
            w.activate();
            return Some(w.id());
        }
    }
    let id = NEXT_WINDOW.fetch_add(1, Ordering::SeqCst);
    match AppWindow::create(id, kind, path.map(Utf8PathBuf::from), options) {
        Ok(w) => {
            WINDOWS.with(|ws| ws.borrow_mut().insert(id, w));
            Some(id)
        }
        Err(e) => {
            error_box(&format!("The window couldn't be opened.\n\n{e}"));
            None
        }
    }
}

/// Forgets a closed window; the app ends with its last window.
pub fn window_closed(id: u64) {
    let empty = WINDOWS.with(|ws| {
        let mut ws = ws.borrow_mut();
        ws.remove(&id);
        ws.is_empty()
    });
    if empty {
        unsafe { PostQuitMessage(0) };
    }
}

/// Opens files given on the command line, by double-click or by a second launch.
pub fn open_paths(paths: Vec<String>) {
    let mut opened = false;
    for p in paths {
        let lower = p.to_ascii_lowercase();
        if lower.ends_with(".cutproj") {
            opened |= open_window(WindowKind::Project, Some(p), None).is_some();
        } else if lower.ends_with(".cutbrand") {
            if let Some(id) = open_window(WindowKind::Library, None, None) {
                send_event(id, Topic::OpenFiles, serde_json::json!({"paths": [p]}));
                opened = true;
            }
        }
    }
    if !opened {
        match windows().into_iter().next() {
            Some(w) => w.activate(),
            None => {
                open_window(WindowKind::Start, None, None);
            }
        }
    }
}

pub fn error_box(text: &str) {
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), w!("Syncfusion Video Studio"), MB_OK | MB_ICONERROR);
    }
}

extern "system" fn dispatch_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_APP_RUN {
        loop {
            let job = queue().lock().unwrap().pop_front();
            match job {
                Some(f) => f(),
                None => break,
            }
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn create_dispatcher() {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(dispatch_proc),
            hInstance: instance.into(),
            lpszClassName: w!("SyncfusionVideoStudioDispatcher"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("SyncfusionVideoStudioDispatcher"),
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .expect("dispatcher window");
        DISPATCH_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    }
}

/// The app's entry point; returns the process exit code.
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let selftest = args.iter().any(|a| a == "--selftest") || std::env::var_os("VIDEO_STUDIO_SELFTEST").is_some();
    let debug = cfg!(debug_assertions) || args.iter().any(|a| a == "--devtools");
    let mut paths: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(|a| std::path::absolute(a).ok().and_then(|p| p.to_str().map(str::to_string)).unwrap_or_else(|| a.clone()))
        .collect();
    let start_library = args.iter().any(|a| a == "--library");
    let start_settings = args.iter().any(|a| a == "--settings");
    SETTINGS.with(|s| *s.borrow_mut() = Options { selftest, debug });

    unsafe {
        // The packaged app has an identity of its own; unpackaged, this groups windows and toasts.
        let _ = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(
            video_studio_core::platform::windows::APP_USER_MODEL_ID,
        ));
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let _instance = if selftest {
        None
    } else {
        match crate::instance::acquire() {
            Ok(i) => Some(i),
            Err(()) => {
                // Already running: hand the files over and leave.
                return if crate::instance::forward(&paths) { 0 } else { 1 };
            }
        }
    };

    if let Err(e) = crate::webview::check_runtime() {
        error_box(&format!(
            "Syncfusion Video Studio needs the Microsoft Edge WebView2 Runtime, which isn't installed or can't be started.\n\n\
             It is part of Windows 11. To install it, download the Evergreen Runtime from \
             https://developer.microsoft.com/microsoft-edge/webview2/ and run it, then start the app again.\n\n({e})"
        ));
        return 1;
    }

    create_dispatcher();
    if _instance.is_some() {
        crate::instance::serve(|paths| post(move || open_paths(paths)));
    }
    crate::shell::start_file_watcher();

    if selftest {
        return crate::selftest::run();
    }
    if start_library {
        open_window(WindowKind::Library, None, None);
    } else if start_settings {
        open_window(WindowKind::Settings, None, None);
    } else {
        paths.retain(|p| !p.is_empty());
        open_paths(paths);
    }
    message_loop()
}

pub fn message_loop() -> i32 {
    let mut msg = MSG::default();
    unsafe {
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0).0;
            if r == 0 {
                return msg.wParam.0 as i32;
            }
            if r == -1 {
                return 1;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
