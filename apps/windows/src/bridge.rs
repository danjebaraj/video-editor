//! Requests from the web UI (ui/spec.md §23.3, "Bridge"). Core operations run on worker threads
//! and answer when done, so the UI never waits on the shell; dialogs and window operations run on
//! the UI thread. Runs and assistant turns report on the `run` and `assistant` topics.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};

use base64::Engine;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use video_studio_ffi as ffi;

use crate::app;
use crate::protocol::*;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

struct RunEntry {
    window: u64,
    run: Arc<ffi::Run>,
}

struct TurnEntry {
    window: u64,
    turn: Arc<ffi::AssistantTurn>,
}

fn runs() -> &'static Mutex<HashMap<u64, RunEntry>> {
    static R: OnceLock<Mutex<HashMap<u64, RunEntry>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Files the user chose in a window's Save dialog, which `shell.write_text` may write once.
fn save_choices() -> &'static Mutex<std::collections::HashSet<(u64, String)>> {
    static S: OnceLock<Mutex<std::collections::HashSet<(u64, String)>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn turns() -> &'static Mutex<HashMap<u64, TurnEntry>> {
    static T: OnceLock<Mutex<HashMap<u64, TurnEntry>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Media files are found by their Windows file ID first (spec §6.2).
struct WinRefs;

impl ffi::FileRefsProvider for WinRefs {
    fn create(&self, path: String) -> Option<String> {
        use video_studio_core::platform::FileRefs;
        video_studio_core::platform::windows::WinFileRefs.create(camino::Utf8Path::new(&path))
    }
    fn resolve(&self, reference: String) -> Option<String> {
        use video_studio_core::platform::FileRefs;
        video_studio_core::platform::windows::WinFileRefs.resolve(&reference).map(|p| p.to_string())
    }
}

fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| ffi::set_file_refs_provider(Box::new(WinRefs)));
}

/// A window closed: its runs keep going (they belong to the project, not the window) but no
/// longer report to it; its assistant turns stop.
pub fn forget_window(window: u64) {
    let stale: Vec<u64> = turns().lock().unwrap().iter().filter(|(_, t)| t.window == window).map(|(k, _)| *k).collect();
    for k in stale {
        if let Some(t) = turns().lock().unwrap().remove(&k) {
            t.turn.stop();
        }
    }
}

/// Whether any run is still going (the app asks before quitting).
pub fn running_runs() -> usize {
    runs().lock().unwrap().values().filter(|r| !r.run.is_finished()).count()
}

fn err(code: &str, message: impl Into<String>) -> HostError {
    HostError { code: code.into(), message: message.into() }
}

fn from_core(e: ffi::CoreError) -> HostError {
    match e {
        ffi::CoreError::Failed { code, message } => HostError { code, message },
    }
}

fn parse<T: DeserializeOwned>(args: &Value) -> Result<T, HostError> {
    serde_json::from_value(args.clone()).map_err(|e| err("bad_request", e.to_string()))
}

fn respond(window: u64, id: u64, result: Result<Value, HostError>) {
    let msg = match result {
        Ok(v) => HostMessage::Response { id, ok: true, result: Some(v), error: None },
        Err(e) => HostMessage::Response { id, ok: false, result: None, error: Some(e) },
    };
    app::send(window, msg);
}

/// Runs `f` on a worker thread and answers with its result.
fn on_worker(window: u64, id: u64, f: impl FnOnce() -> Result<Value, HostError> + Send + 'static) {
    std::thread::spawn(move || respond(window, id, f()));
}

/// Forwards core events (run or assistant) to a window.
struct Listener {
    window: u64,
    token: u64,
    topic: Topic,
}

impl ffi::EventListener for Listener {
    fn on_event(&self, json_text: String) {
        let event: Value = serde_json::from_str(&json_text).unwrap_or(Value::Null);
        let data = match self.topic {
            Topic::Run => serde_json::to_value(RunEvent { token: self.token, event: Some(event), finished: None })
                .unwrap_or(Value::Null),
            _ => json!({"token": self.token, "event": event}),
        };
        app::send_event(self.window, self.topic, data);
    }
}

/// Settings are app-wide: after one window saves them, every other open window gets the saved
/// settings on the `settings_changed` topic and re-checks the selected provider
/// (FEAT-spry-alpaca-5RQZ). Runs and turns already started keep their provider.
fn broadcast_settings(from: u64, settings: Value) {
    app::post(move || {
        for w in app::windows() {
            if w.id() != from {
                w.event(Topic::SettingsChanged, settings.clone());
            }
        }
    });
}

/// Handles one message from a window's page (on the UI thread).
pub fn handle(window: u64, text: &str) {
    init();
    // The page announces itself before its first request.
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if v["kind"] == "ready" {
            if let Some(w) = app::window(window) {
                w.mark_ready();
            }
            return;
        }
    }
    let req: Request = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            let id = serde_json::from_str::<Value>(text).ok().and_then(|v| v["id"].as_u64()).unwrap_or(0);
            respond(window, id, Err(err("bad_request", format!("not a request: {e}"))));
            return;
        }
    };
    let id = req.id;
    let args = req.args;
    match req.method {
        Method::Core => on_worker(window, id, move || {
            let a: CoreArgs = parse(&args)?;
            let saves_settings = a.method == "settings.save";
            let out = ffi::call(a.method, a.args.to_string()).map_err(from_core)?;
            let v: Value = serde_json::from_str(&out).map_err(|e| err("decode", e.to_string()))?;
            if saves_settings {
                broadcast_settings(window, v.clone());
            }
            Ok(v)
        }),
        Method::StorePastedImage => on_worker(window, id, move || {
            let a: PastedImageArgs = parse(&args)?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(a.image_base64.as_bytes())
                .map_err(|e| err("bad_request", e.to_string()))?;
            Ok(Value::String(ffi::store_pasted_image(a.project_path, bytes).map_err(from_core)?))
        }),
        Method::RunStart => on_worker(window, id, move || {
            let a: RunStartArgs = parse(&args)?;
            let mut options = if a.options.is_object() { a.options } else { json!({}) };
            options["helper"] = json!(crate::shell::helper_path().to_string());
            if options.get("eleven_key_present").is_none() {
                let present = crate::shell::secret("status", None).map(|v| v["present"] == true).unwrap_or(false);
                options["eleven_key_present"] = json!(present);
            }
            let token = NEXT_TOKEN.fetch_add(1, Ordering::SeqCst);
            let listener = Listener { window, token, topic: Topic::Run };
            let run =
                ffi::start_run(a.request.to_string(), options.to_string(), Box::new(listener)).map_err(from_core)?;
            let started = RunStarted { token, run_id: run.run_id(), folder: run.folder() };
            runs().lock().unwrap().insert(token, RunEntry { window, run: run.clone() });
            // Report the end, with the outcome, when the run finishes.
            std::thread::spawn(move || {
                let outcome: Value = serde_json::from_str(&run.wait()).unwrap_or(Value::Null);
                let w = runs().lock().unwrap().remove(&token).map(|e| e.window).unwrap_or(window);
                let data = RunEvent { token, event: None, finished: Some(outcome) };
                app::send_event(w, Topic::Run, serde_json::to_value(data).unwrap_or(Value::Null));
            });
            serde_json::to_value(started).map_err(|e| err("decode", e.to_string()))
        }),
        Method::RunCancel | Method::RunKill => {
            let r = parse::<TokenArgs>(&args).and_then(|a| {
                let runs = runs().lock().unwrap();
                let e = runs.get(&a.token).ok_or_else(|| err("not_found", "no such run"))?;
                if req.method == Method::RunCancel {
                    e.run.cancel();
                } else {
                    e.run.kill();
                }
                Ok(Value::Bool(true))
            });
            respond(window, id, r);
        }
        Method::RunAnswer => {
            let r = parse::<AnswerArgs>(&args).and_then(|a| {
                let runs = runs().lock().unwrap();
                let e = runs.get(&a.token).ok_or_else(|| err("not_found", "no such run"))?;
                e.run
                    .answer(a.question_id, serde_json::to_string(&a.answers).unwrap_or_default())
                    .map_err(from_core)?;
                Ok(Value::Bool(true))
            });
            respond(window, id, r);
        }
        Method::AssistantStart => on_worker(window, id, move || {
            let a: AssistantStartArgs = parse(&args)?;
            let token = NEXT_TOKEN.fetch_add(1, Ordering::SeqCst);
            let listener = Listener { window, token, topic: Topic::Assistant };
            let turn = ffi::start_assistant(a.request.to_string(), Box::new(listener)).map_err(from_core)?;
            turns().lock().unwrap().insert(token, TurnEntry { window, turn: turn.clone() });
            std::thread::spawn(move || {
                turn.wait();
                turns().lock().unwrap().remove(&token);
                app::send_event(window, Topic::Assistant, json!({"token": token, "finished": true}));
            });
            Ok(json!({"token": token}))
        }),
        Method::AssistantStop | Method::AssistantAnswer | Method::AssistantDismiss => {
            let r = (|| {
                let token = args["token"].as_u64().ok_or_else(|| err("bad_request", "no token"))?;
                let turns = turns().lock().unwrap();
                let t = turns.get(&token).ok_or_else(|| err("not_found", "no such assistant turn"))?;
                match req.method {
                    Method::AssistantStop => t.turn.stop(),
                    Method::AssistantDismiss => {
                        t.turn.dismiss(args["question_id"].as_str().unwrap_or("").to_string());
                    }
                    _ => {
                        let a: AnswerArgs = parse(&args)?;
                        t.turn
                            .answer(a.question_id, serde_json::to_string(&a.answers).unwrap_or_default())
                            .map_err(from_core)?;
                    }
                }
                Ok(Value::Bool(true))
            })();
            respond(window, id, r);
        }
        // Modal dialogs run after this WebView2 callback returns: a nested modal loop inside
        // WebMessageReceived isn't supported by WebView2.
        Method::DialogOpen => app::post(move || {
            let r = parse::<OpenDialogArgs>(&args).map(|a| {
                let hwnd = app::window(window).map(|w| w.hwnd()).unwrap_or_default();
                json!(crate::dialogs::open(hwnd, &a))
            });
            respond(window, id, r);
        }),
        Method::DialogSave => app::post(move || {
            let r = parse::<SaveDialogArgs>(&args).map(|a| {
                let hwnd = app::window(window).map(|w| w.hwnd()).unwrap_or_default();
                let chosen = crate::dialogs::save(hwnd, &a);
                if let Some(p) = &chosen {
                    save_choices().lock().unwrap().insert((window, p.to_lowercase()));
                }
                json!(chosen)
            });
            respond(window, id, r);
        }),
        Method::WriteText => {
            let r = parse::<WriteTextArgs>(&args).and_then(|a| {
                // Only a file the user picked in this window's Save dialog, once.
                if !save_choices().lock().unwrap().remove(&(window, a.path.to_lowercase())) {
                    return Err(err("forbidden", "only a file chosen in a Save dialog can be written"));
                }
                video_studio_core::paths::write_atomic(camino::Utf8Path::new(&a.path), a.text.as_bytes())
                    .map_err(|e| err("io", e.to_string()))?;
                Ok(Value::Bool(true))
            });
            respond(window, id, r);
        }
        Method::CopyText => {
            let r = parse::<CopyTextArgs>(&args)
                .and_then(|a| crate::shell::copy_text(&a.text).map_err(|e| err("shell", e)).map(|_| Value::Bool(true)));
            respond(window, id, r);
        }
        Method::ClipboardText => respond(window, id, Ok(json!(crate::shell::clipboard_text()))),
        Method::Recycle => {
            let r = parse::<RecycleArgs>(&args).map(|a| {
                let moved: Vec<String> =
                    a.paths.into_iter().filter(|p| crate::shell::recycle_sidecar_file(p)).collect();
                json!({"moved": moved})
            });
            respond(window, id, r);
        }
        Method::DialogConfirm => app::post(move || {
            let r = parse::<ConfirmArgs>(&args).map(|a| {
                let hwnd = app::window(window).map(|w| w.hwnd()).unwrap_or_default();
                json!(crate::dialogs::confirm(hwnd, &a))
            });
            respond(window, id, r);
        }),
        Method::Info => {
            let r = app::window(window).ok_or_else(|| err("shell", "no window")).map(|w| {
                let info = ShellInfo {
                    app_version: env!("CARGO_PKG_VERSION").into(),
                    core_version: ffi::core_version(),
                    platform: "windows".into(),
                    arch: std::env::consts::ARCH.into(),
                    window_kind: w.kind(),
                    window_id: window,
                    helper: crate::shell::helper_path().to_string(),
                    webview_version: crate::webview::runtime_version(),
                    debug: app::options().debug,
                    selftest: app::options().selftest,
                    theme: crate::shell::theme(w.scale()),
                    path: w.path().map(|p| p.to_string()),
                    options: w.options(),
                };
                serde_json::to_value(info).unwrap_or(Value::Null)
            });
            respond(window, id, r);
        }
        Method::Reveal => {
            let r = parse::<PathArgs>(&args)
                .and_then(|a| crate::shell::reveal(&a.path).map_err(|e| err("shell", e)).map(|_| Value::Bool(true)));
            respond(window, id, r);
        }
        Method::Open => {
            let r = parse::<OpenArgs>(&args).and_then(|a| {
                crate::shell::open(&a.path, a.application.as_deref())
                    .map_err(|e| err("shell", e))
                    .map(|_| Value::Bool(true))
            });
            respond(window, id, r);
        }
        Method::OpenUrl => {
            let r = parse::<UrlArgs>(&args).and_then(|a| {
                let lower = a.url.to_ascii_lowercase();
                if lower.starts_with("https://") || lower.starts_with("http://") {
                    crate::shell::open_url(&a.url);
                    Ok(Value::Bool(true))
                } else {
                    Err(err("bad_request", "only http(s) links open in the browser"))
                }
            });
            respond(window, id, r);
        }
        Method::Notify => {
            let r = parse::<NotifyArgs>(&args).map(|a| {
                std::thread::spawn(move || crate::shell::notify(&a.title, &a.body));
                Value::Bool(true)
            });
            respond(window, id, r);
        }
        Method::Progress => {
            let r = parse::<ProgressArgs>(&args).map(|a| {
                if let Some(w) = app::window(window) {
                    crate::shell::set_progress(w.hwnd(), a.state, a.value);
                }
                Value::Bool(true)
            });
            respond(window, id, r);
        }
        Method::SetTitle => {
            let r = parse::<TitleArgs>(&args).map(|a| {
                if let Some(w) = app::window(window) {
                    w.set_title(&a.title);
                }
                Value::Bool(true)
            });
            respond(window, id, r);
        }
        Method::SetDirty => {
            let r = parse::<DirtyArgs>(&args).map(|a| {
                if let Some(w) = app::window(window) {
                    w.set_dirty(a.dirty);
                }
                Value::Bool(true)
            });
            respond(window, id, r);
        }
        Method::OpenWindow => {
            let r = parse::<OpenWindowArgs>(&args).map(|a| {
                // Opening happens after this answer: creating a window pumps messages.
                app::post(move || {
                    app::open_window(a.kind, a.path, a.options);
                });
                Value::Bool(true)
            });
            respond(window, id, r);
        }
        Method::CloseWindow => {
            respond(window, id, Ok(Value::Bool(true)));
            app::post(move || {
                if let Some(w) = app::window(window) {
                    w.close_now();
                }
            });
        }
        Method::Quit => app::post(move || {
            let running = running_runs();
            if running > 0 {
                let hwnd = app::window(window).map(|w| w.hwnd()).unwrap_or_default();
                let choice = crate::dialogs::confirm(
                    hwnd,
                    &ConfirmArgs {
                        title: format!("{running} run(s) are still going"),
                        message: "Quitting stops them. Their logs are kept up to the point they stop.".into(),
                        buttons: vec!["Stop and Quit".into(), "Cancel".into()],
                        warning: true,
                    },
                );
                if choice != 0 {
                    respond(window, id, Ok(Value::Bool(false)));
                    return;
                }
                for r in runs().lock().unwrap().values() {
                    r.run.cancel();
                }
            }
            respond(window, id, Ok(Value::Bool(true)));
            app::post(|| {
                for w in app::windows() {
                    w.event(Topic::CloseRequested, json!({"quitting": true}));
                }
            });
        }),
        Method::Recent => respond(window, id, Ok(json!(crate::shell::recent()))),
        Method::AddRecent => {
            let r = parse::<PathArgs>(&args).map(|a| {
                crate::shell::add_recent(&a.path);
                if let Some(w) = app::window(window) {
                    if w.kind() == WindowKind::Project {
                        w.set_path(Some(a.path.clone().into()));
                    }
                }
                Value::Bool(true)
            });
            respond(window, id, r);
        }
        Method::Watch => {
            let r = parse::<PathArgs>(&args).map(|a| {
                crate::shell::watch(window, &a.path);
                Value::Bool(true)
            });
            respond(window, id, r);
        }
        Method::AllowMedia => {
            let r = parse::<AllowMediaArgs>(&args).map(|a| {
                if a.add {
                    crate::media_server::add_allowed(window, &a.files);
                } else {
                    crate::media_server::set_allowed(window, &a.files, a.sidecar.as_deref());
                }
                Value::Bool(true)
            });
            respond(window, id, r);
        }
        Method::MediaUrl => {
            let r = parse::<PathArgs>(&args).and_then(|a| {
                if crate::media_server::is_allowed(window, camino::Utf8Path::new(&a.path)) {
                    Ok(Value::String(crate::media_server::url_for(&a.path)))
                } else {
                    Err(err("forbidden", "only the project's media can be shown"))
                }
            });
            respond(window, id, r);
        }
        Method::SecretStatus | Method::SecretSet | Method::SecretTest | Method::SecretDelete => {
            let action = match req.method {
                Method::SecretStatus => "status",
                Method::SecretSet => "set",
                Method::SecretTest => "test",
                _ => "delete",
            };
            on_worker(window, id, move || {
                let a: SecretArgs = parse(&args).unwrap_or_default();
                crate::shell::secret(action, a.value.as_deref()).map_err(|e| err("environment", e))
            });
        }
        Method::LoadSessions | Method::SaveSessions | Method::DeleteSession => {
            let method = req.method;
            on_worker(window, id, move || {
                let a: SessionsArgs = parse(&args)?;
                // Only plain IDs: the folder is the core's per-project sessions folder.
                let plain = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c));
                if !plain(&a.project_id) {
                    return Err(err("bad_request", "not a project id"));
                }
                let lib = video_studio_core::paths::library_dir();
                let dir = video_studio_core::assistant::sessions_dir(&lib, &a.project_id);
                let file = dir.join("sessions.json");
                match method {
                    Method::LoadSessions => {
                        Ok(video_studio_core::paths::read_json::<Value>(&file).unwrap_or(json!([])))
                    }
                    Method::SaveSessions => {
                        video_studio_core::paths::write_json_atomic(&file, &a.sessions.unwrap_or(json!([])))
                            .map_err(|e| err("io", e.to_string()))?;
                        Ok(Value::Bool(true))
                    }
                    _ => {
                        let sid = a.session_id.unwrap_or_default();
                        if !plain(&sid) {
                            return Err(err("bad_request", "not a session id"));
                        }
                        let _ = std::fs::remove_dir_all(dir.join(&sid));
                        Ok(Value::Bool(true))
                    }
                }
            });
        }
        Method::Cutline => on_worker(window, id, move || {
            let a: CutlineArgs = parse(&args)?;
            let s = video_studio_core::settings::Settings::load(&video_studio_core::paths::library_dir());
            let c = video_studio_core::claude::locate::find_cutline(&s.cutline_path, &s.cutline_repo)
                .ok_or_else(|| err("environment", "cutline wasn't found"))?;
            let mut cmd = std::process::Command::new(&c.path);
            let timeout = match a.action {
                CutlineAction::Doctor => {
                    cmd.arg("doctor");
                    120
                }
                CutlineAction::InstallDeps => {
                    cmd.args(["deps", "install", "--all"]);
                    1800
                }
            };
            let (code, out, errout) =
                video_studio_core::platform::run_capture(&mut cmd, std::time::Duration::from_secs(timeout))
                    .map_err(|e| err("environment", e.to_string()))?;
            Ok(json!({"code": code, "output": format!("{out}{errout}")}))
        }),
        Method::Snapshot => {
            let r = parse::<PathArgs>(&args);
            match r {
                Ok(a) => crate::selftest::snapshot(window, &a.path, move |res| {
                    respond(window, id, res.map(|_| Value::Bool(true)).map_err(|e| err("shell", e)))
                }),
                Err(e) => respond(window, id, Err(e)),
            }
        }
        Method::SelfTestFs => {
            let r = parse::<SelfTestFsArgs>(&args).and_then(|a| {
                if !app::options().selftest {
                    return Err(err("forbidden", "only in the self test"));
                }
                crate::selftest::scratch_fs(&a.op, &a.from, a.to.as_deref()).map_err(|e| err("io", e)).map(Value::Bool)
            });
            respond(window, id, r);
        }
        Method::SelfTestReport => {
            let r = parse::<SelfTestReport>(&args).map(|a| {
                crate::selftest::report(a);
                Value::Bool(true)
            });
            respond(window, id, r);
        }
    }
}
