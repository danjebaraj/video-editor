//! The WebView2 host (ui/spec.md §23.3).
//!
//! - The UI is served from a virtual host, `https://app.video-studio.invalid/`, mapped to the
//!   built UI folder. Nothing remote loads: other navigations are cancelled (http(s) links open
//!   in the default browser) and the page's CSP allows only the two app hosts.
//! - Media thumbnails, players and pasted images come from `https://media.video-studio.invalid/`,
//!   answered by [`crate::media_server`] only for the window's project media and sidecar.
//! - DevTools and the default context menu are off in Release builds; the browser's own
//!   accelerator keys (F5 reload, Ctrl+P print, …) are off so the app's shortcuts work.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use camino::Utf8PathBuf;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::core::{Interface, HSTRING, PWSTR};
use windows::Win32::Foundation::E_POINTER;

use crate::app;
use crate::window::AppWindow;

pub const APP_HOST: &str = "app.video-studio.invalid";
pub const MEDIA_HOST: &str = "media.video-studio.invalid";

/// The installed WebView2 runtime's version, or why there is none.
pub fn check_runtime() -> std::result::Result<String, String> {
    let mut v = PWSTR::null();
    unsafe {
        GetAvailableCoreWebView2BrowserVersionString(windows::core::PCWSTR::null(), &mut v)
            .map_err(|e| e.message().to_string())?;
    }
    let s = CoTaskMemPWSTR::from(v).to_string();
    if s.is_empty() {
        Err("no WebView2 runtime was found".into())
    } else {
        Ok(s)
    }
}

pub fn runtime_version() -> String {
    check_runtime().unwrap_or_default()
}

/// The built web UI: `ui\` next to the executable (installed, or after `build.ps1`), else the
/// Vite output in the source tree (a plain `cargo run`).
pub fn ui_dir() -> Utf8PathBuf {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("ui"))) {
        if dir.join("index.html").is_file() {
            if let Ok(d) = Utf8PathBuf::from_path_buf(dir) {
                return d;
            }
        }
    }
    Utf8PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/ui/dist"))
}

/// WebView2's profile folder: under Local AppData, never roaming.
fn user_data_dir() -> Utf8PathBuf {
    let base = std::env::var("LOCALAPPDATA").map(Utf8PathBuf::from).unwrap_or_else(|_| Utf8PathBuf::from("."));
    let mut dir = base.join(video_studio_core::APP_NAME).join("WebView2");
    // Self tests and UI tests get their own profile so they never share state with the real app.
    if std::env::var_os("VIDEO_STUDIO_HOME").is_some() {
        dir = video_studio_core::paths::library_dir().join("WebView2");
    }
    dir
}

thread_local! {
    static ENV: RefCell<Option<ICoreWebView2Environment>> = const { RefCell::new(None) };
}

fn environment() -> windows::core::Result<ICoreWebView2Environment> {
    if let Some(e) = ENV.with(|e| e.borrow().clone()) {
        return Ok(e);
    }
    let folder = HSTRING::from(user_data_dir().as_str());
    let options = CoreWebView2EnvironmentOptions::default();
    let (tx, rx) = mpsc::channel();
    let options: ICoreWebView2EnvironmentOptions = options.into();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            CreateCoreWebView2EnvironmentWithOptions(windows::core::PCWSTR::null(), &folder, &options, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |code, env| {
            code?;
            tx.send(env.ok_or_else(|| windows::core::Error::from(E_POINTER))).expect("channel");
            Ok(())
        }),
    )
    .map_err(to_win)?;
    let env = rx.recv().map_err(|_| windows::core::Error::from(E_POINTER))??;
    ENV.with(|e| *e.borrow_mut() = Some(env.clone()));
    Ok(env)
}

fn to_win(e: webview2_com::Error) -> windows::core::Error {
    match e {
        webview2_com::Error::WindowsError(w) => w,
        other => windows::core::Error::new(windows::core::HRESULT(-1), format!("{other:?}")),
    }
}

fn wide(s: &str) -> HSTRING {
    HSTRING::from(s)
}

/// Creates the window's WebView, wires its events and loads the UI.
pub fn attach(win: &Rc<AppWindow>) -> windows::core::Result<()> {
    let env = environment()?;
    let hwnd = win.hwnd();
    let (tx, rx) = mpsc::channel();
    let env2 = env.clone();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            env2.CreateCoreWebView2Controller(hwnd, &handler).map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |code, controller| {
            code?;
            tx.send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER))).expect("channel");
            Ok(())
        }),
    )
    .map_err(to_win)?;
    let controller = rx.recv().map_err(|_| windows::core::Error::from(E_POINTER))??;
    let webview = unsafe { controller.CoreWebView2()? };
    let debug = app::options().debug;
    unsafe {
        // A transparent page background lets the Mica backdrop show through.
        if let Ok(c2) = controller.cast::<ICoreWebView2Controller2>() {
            let _ = c2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR { A: 0, R: 0, G: 0, B: 0 });
        }
        let settings = webview.Settings()?;
        settings.SetAreDevToolsEnabled(debug)?;
        settings.SetAreDefaultContextMenusEnabled(debug)?;
        settings.SetIsStatusBarEnabled(false)?;
        settings.SetIsZoomControlEnabled(false)?;
        if let Ok(s3) = settings.cast::<ICoreWebView2Settings3>() {
            s3.SetAreBrowserAcceleratorKeysEnabled(false)?;
        }
        if let Ok(w3) = webview.cast::<ICoreWebView2_3>() {
            w3.SetVirtualHostNameToFolderMapping(
                &wide(APP_HOST),
                &wide(ui_dir().as_str()),
                COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
            )?;
        }
        webview.AddWebResourceRequestedFilter(
            &wide(&format!("https://{MEDIA_HOST}/*")),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        )?;
    }

    let id = win.id();
    let mut token = 0i64;
    unsafe {
        // UI → shell messages
        webview.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_wv, args| {
                if let Some(args) = args {
                    let mut msg = PWSTR::null();
                    if args.WebMessageAsJson(&mut msg).is_ok() {
                        let text = CoTaskMemPWSTR::from(msg).to_string();
                        crate::bridge::handle(id, &text);
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
        // Only the app's own pages load in the window.
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_wv, args| {
                if let Some(args) = args {
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let uri = CoTaskMemPWSTR::from(uri).to_string();
                    if !uri.starts_with(&format!("https://{APP_HOST}/")) {
                        args.SetCancel(true)?;
                        open_external(&uri);
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_wv, args| {
                if let Some(args) = args {
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    open_external(&CoTaskMemPWSTR::from(uri).to_string());
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        let env3 = env.clone();
        webview.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_wv, args| {
                if let Some(args) = args {
                    crate::media_server::respond(&env3, id, &args);
                }
                Ok(())
            })),
            &mut token,
        )?;
        // A crashed renderer is reloaded rather than left blank.
        webview.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |wv, _args| {
                if let Some(wv) = wv {
                    if let Some(w) = app::window(id) {
                        w.ready.set(false);
                    }
                    let _ = wv.Reload();
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    *win.controller.borrow_mut() = Some(controller.clone());
    *win.webview.borrow_mut() = Some(webview.clone());
    unsafe {
        let mut rc = windows::Win32::Foundation::RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc);
        controller.SetBounds(rc)?;
        controller.SetIsVisible(true)?;
        let route = match win.kind() {
            crate::protocol::WindowKind::Start => "start",
            crate::protocol::WindowKind::Project => "project",
            crate::protocol::WindowKind::Library => "library",
            crate::protocol::WindowKind::Settings => "settings",
        };
        webview.Navigate(&wide(&format!("https://{APP_HOST}/index.html#/{route}?window={id}")))?;
    }
    Ok(())
}

/// http(s) and mailto links open in the default browser; anything else is ignored.
fn open_external(uri: &str) {
    let lower = uri.to_ascii_lowercase();
    let external = lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:");
    if external && !lower.contains(".video-studio.invalid") {
        crate::shell::open_url(uri);
    }
}
