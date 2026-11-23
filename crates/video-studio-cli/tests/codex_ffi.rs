//! FEAT-spry-alpaca-5RQZ: an assistant turn started the way the apps start one (FFI
//! `start_assistant`) uses Codex when it's selected in Settings, on a machine without Claude Code.
//! One test per binary: it sets process-wide environment variables.

use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};
use video_studio_core::settings::Settings;

const FAKE: &str = env!("CARGO_BIN_EXE_vs-fake-codex");

struct Listener(Mutex<Sender<Value>>);

impl video_studio_ffi::EventListener for Listener {
    fn on_event(&self, json: String) {
        let _ = self.0.lock().unwrap().send(serde_json::from_str(&json).unwrap_or(Value::Null));
    }
}

#[test]
fn an_app_assistant_turn_uses_codex_without_claude() {
    let dir = tempfile::tempdir().unwrap();
    let root = video_studio_core::paths::canonical_or_normal(Utf8Path::from_path(dir.path()).unwrap());
    let lib = root.join("library");
    std::fs::create_dir_all(&lib).unwrap();
    std::env::set_var("VIDEO_STUDIO_HOME", lib.as_str());
    std::env::set_var("VIDEO_STUDIO_NO_CLAUDE", "1");
    let mut s = Settings::load(&lib);
    s.provider = video_studio_core::codex::Provider::Codex;
    s.codex_path = FAKE.to_string();
    s.save(&lib).unwrap();

    let script = root.join("script.json");
    std::fs::write(&script, json!([[{"say": "The hook runs five seconds."}]]).to_string()).unwrap();
    let log = root.join("codex.ndjson");
    let session: Utf8PathBuf = root.join("assistant/p1/c1");
    let request = json!({
        "session_dir": session,
        "spec": "# Launch\n\n## Scene 1: Hook\n- Duration: 5 s\n",
        "prompt": "How long is the hook?",
        "model": "opus",
        "web_access": false,
        "read_paths": [],
        "reference_paths": [],
        "project_summary": "Project: Launch.",
        "media": [],
        "extra_env": {"FAKE_CODEX_SCRIPT": script, "FAKE_CODEX_LOG": log},
    });
    let (tx, rx) = channel();
    let turn = video_studio_ffi::start_assistant(request.to_string(), Box::new(Listener(Mutex::new(tx))))
        .unwrap_or_else(|e| panic!("the turn starts without Claude: {e:?}"));
    let mut events = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(30)).expect("the turn finishes");
        let done = e["type"] == "finished" || e["kind"] == "finished" || e.to_string().contains("\"finished\"");
        events.push(e);
        if done {
            break;
        }
    }
    turn.wait();
    let text = serde_json::to_string(&events).unwrap();
    assert!(text.contains("The hook runs five seconds."), "{text}");
    assert!(text.contains("codex:"), "the session token names the Codex thread: {text}");
    let sent = std::fs::read_to_string(&log).unwrap();
    assert!(sent.contains("\"thread/start\"") && sent.contains("\"turn/start\""), "{sent}");

    // BUG-fond-crane-62ZP: Claude's model ("opus" above) never reaches Codex; Codex default
    // sends no model, and the project's Codex assistant model reaches Codex as is
    let thread_starts = |log: &Utf8Path| -> Vec<Value> {
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|m| m["method"] == "thread/start" || m["method"] == "turn/start")
            .collect()
    };
    for m in thread_starts(&log) {
        assert!(m["params"].get("model").is_none(), "{m}");
    }
    let log2 = root.join("codex2.ndjson");
    let mut req2 = request.clone();
    req2["codex_model"] = json!("gpt-5.5-codex");
    req2["session_dir"] = json!(root.join("assistant/p1/c2"));
    req2["extra_env"]["FAKE_CODEX_LOG"] = json!(log2);
    let (tx, rx) = channel();
    let turn = video_studio_ffi::start_assistant(req2.to_string(), Box::new(Listener(Mutex::new(tx)))).unwrap();
    while let Ok(e) = rx.recv_timeout(Duration::from_secs(30)) {
        if e.to_string().contains("\"finished\"") {
            break;
        }
    }
    turn.wait();
    let starts = thread_starts(&log2);
    assert!(!starts.is_empty() && starts.iter().all(|m| m["params"]["model"] == "gpt-5.5-codex"), "{starts:?}");
    // the display answer matches what was sent
    let shown: Value = serde_json::from_str(
        &video_studio_ffi::call(
            "models.effective".into(),
            json!({"models": {"assistant": "fable", "dry_run": "", "codex_assistant": ""}, "use": "assistant"})
                .to_string(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(shown["model"], Value::Null, "{shown}");
    assert_eq!(shown["codex_default"], true);

    // a configured path that can't run is reported with that path, not as "isn't installed"
    s.codex_path = root.join("nope/codex").to_string();
    s.save(&lib).unwrap();
    let (tx, _rx) = channel();
    let err = match video_studio_ffi::start_assistant(request.to_string(), Box::new(Listener(Mutex::new(tx)))) {
        Err(e) => format!("{e:?}"),
        Ok(_) => panic!("a bad Codex path must not start a turn"),
    };
    assert!(err.contains("nope/codex") && err.contains("doesn't exist"), "{err}");
}
