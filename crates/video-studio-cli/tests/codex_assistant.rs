//! FEAT-spry-alpaca-5RQZ: the spec assistant over Codex, against the fake app server
//! (`vs-fake-codex`): initialization before the thread, streamed replies, questions answered with
//! the request's own ID, edits on the session copy reviewed per hunk, thread resume and its
//! failure, the broker's refusals, Stop, an unexpected exit and a failed sign-in.

use std::collections::BTreeMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};
use video_studio_core::assistant::{
    self, apply_hunks, diff_hunks, AssistantEvent, AssistantHandle, AssistantRequest, AssistantSink, CodexSetup,
};

const FAKE: &str = env!("CARGO_BIN_EXE_vs-fake-codex");
const SPEC: &str =
    "# Launch\n\n## Scene 1: Hook\n- Duration: 5 s\n\n## Scene 2: Close\n- Duration: 10 s\n\n## Call to action\nGo.\n";

struct Sink(Mutex<Sender<AssistantEvent>>);

impl AssistantSink for Sink {
    fn emit(&self, event: &AssistantEvent) {
        let _ = self.0.lock().unwrap().send(event.clone());
    }
}

struct World {
    _dir: tempfile::TempDir,
    root: Utf8PathBuf,
    script: Utf8PathBuf,
}

impl World {
    fn new(turns: Value) -> World {
        let dir = tempfile::tempdir().unwrap();
        let root = video_studio_core::paths::canonical_or_normal(Utf8Path::from_path(dir.path()).unwrap());
        let script = root.join("script.json");
        std::fs::write(&script, turns.to_string()).unwrap();
        World { _dir: dir, root, script }
    }

    fn session(&self, project: &str, conversation: &str) -> Utf8PathBuf {
        self.root.join("assistant").join(project).join(conversation)
    }

    fn request(&self, session_dir: &Utf8Path, prompt: &str, resume: Option<&str>, log: &str) -> AssistantRequest {
        let mut extra_env = BTreeMap::new();
        extra_env.insert("FAKE_CODEX_SCRIPT".to_string(), self.script.to_string());
        extra_env.insert("FAKE_CODEX_LOG".to_string(), self.root.join(log).to_string());
        AssistantRequest {
            session_dir: session_dir.to_owned(),
            spec: SPEC.into(),
            prompt: prompt.into(),
            model: "opus".into(),
            codex_model: None,
            effort: Some("max".into()),
            web_access: false,
            resume: resume.map(str::to_string),
            read_paths: vec![],
            reference_paths: vec![],
            project_summary: "Project: Launch teaser (marketing, 45 s).".into(),
            media: vec![],
            cutline: None,
            helper: None,
            extra_env,
        }
    }

    fn log(&self, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.root.join(name))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

fn setup() -> CodexSetup {
    CodexSetup { executable: FAKE.into(), model: Some("gpt-5.5-codex".into()) }
}

/// Starts a turn and follows it to `finished`, letting `react` answer or stop on the way.
fn run_turn(req: &AssistantRequest, mut react: impl FnMut(&AssistantEvent, &AssistantHandle)) -> Vec<AssistantEvent> {
    let (tx, rx): (Sender<AssistantEvent>, Receiver<AssistantEvent>) = channel();
    let handle = assistant::start_codex(req, &setup(), Arc::new(Sink(Mutex::new(tx)))).unwrap();
    let mut events = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(30)).expect("the turn finishes");
        react(&e, &handle);
        let done = matches!(e, AssistantEvent::Finished { .. });
        events.push(e);
        if done {
            break;
        }
    }
    handle.wait();
    assert!(handle.is_finished());
    events
}

fn finished(events: &[AssistantEvent]) -> (bool, bool, Option<String>) {
    match events.last() {
        Some(AssistantEvent::Finished { ok, stopped, session_id }) => (*ok, *stopped, session_id.clone()),
        other => panic!("{other:?}"),
    }
}

fn errors(events: &[AssistantEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AssistantEvent::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn methods(log: &[Value]) -> Vec<String> {
    log.iter().filter_map(|m| m["method"].as_str().map(str::to_string)).collect()
}

fn count(log: &[Value], method: &str) -> usize {
    log.iter().filter(|m| m["method"] == method).count()
}

fn response<'a>(log: &'a [Value], id: &str) -> &'a Value {
    log.iter().find(|m| m["id"] == id && m.get("method").is_none()).unwrap_or_else(|| panic!("no answer to {id}"))
}

#[test]
fn streams_asks_and_proposes_edits_for_per_hunk_review() {
    let w = World::new(json!([[
        {"say": "Looking at the hook."},
        {"usage": [1200, 80]},
        {"ask": {"id": "q-7", "questions": [{"id": "scene", "header": "Scene", "question": "Which scene should change?", "isOther": false, "isSecret": false, "options": [{"label": "Scene 1", "description": "The hook"}, {"label": "Scene 2", "description": "The close"}]}]}},
        {"edit": {"path": "spec.md", "edits": [["- Duration: 5 s", "- Duration: 4 s"], ["Go.", "Start a free trial."]]}},
        {"say": "Tightened the hook and sharpened the call to action."}
    ]]));
    let dir = w.session("p1", "c1");
    let prompt = assistant::compose_prompt("Tighten the hook", Some(("- Duration: 5 s", 4, 4)), &["m-hero".into()]);
    let req = w.request(&dir, &prompt, None, "t1.ndjson");
    let events = run_turn(&req, |e, h| {
        if let AssistantEvent::Question { id, questions } = e {
            assert_eq!(id, "q-7", "the question keeps the server's string ID");
            assert_eq!(questions[0].options.len(), 2);
            let mut a = BTreeMap::new();
            a.insert(questions[0].question.clone(), "Scene 1".to_string());
            h.answer(id, a);
        }
    });
    let (ok, stopped, session) = finished(&events);
    assert!(ok && !stopped, "{events:#?}");
    let token = session.expect("a session token");
    assert!(token.starts_with("codex:thr-"), "{token}");
    assert!(errors(&events).is_empty(), "{events:#?}");

    // initialization comes before the thread, the thread before the turn
    let log = w.log("t1.ndjson");
    let m = methods(&log);
    let pos = |name: &str| m.iter().position(|x| x == name).unwrap_or_else(|| panic!("no {name} in {m:?}"));
    assert_eq!(&m[..3], ["initialize", "initialized", "account/read"]);
    assert!(pos("account/read") < pos("thread/start") && pos("thread/start") < pos("turn/start"));
    // the editor context reaches Codex: summary and instructions as developer instructions, the
    // selection and attachments in the prompt, the session folder as the only writable root
    let start = log.iter().find(|x| x["method"] == "thread/start").unwrap();
    let instructions = start["params"]["developerInstructions"].as_str().unwrap();
    assert!(instructions.contains("Launch teaser") && instructions.contains("Working in Codex"));
    assert_eq!(start["params"]["approvalsReviewer"], "user");
    assert_eq!(start["params"]["config"]["mcp_servers"], json!({}));
    let turn = log.iter().find(|x| x["method"] == "turn/start").unwrap();
    let text = turn["params"]["input"][0]["text"].as_str().unwrap();
    assert!(text.contains("Tighten the hook") && text.contains("lines 4–4") && text.contains("media:m-hero"));
    assert_eq!(turn["params"]["sandboxPolicy"]["writableRoots"], json!([dir.to_string()]));
    assert_eq!(turn["params"]["model"], "gpt-5.5-codex");
    assert_eq!(turn["params"]["effort"], "xhigh");
    // the answer carries the request's own ID and Codex's question ID
    let answer = response(&log, "q-7");
    assert_eq!(answer["result"]["answers"]["scene"]["answers"], json!(["Scene 1"]));

    // streamed text, then the whole message; usage; the spec change for review
    assert!(events.iter().any(|e| matches!(e, AssistantEvent::TextDelta { text } if text.starts_with("Looking"))));
    assert!(events.iter().any(|e| matches!(e, AssistantEvent::Text { text } if text == "Looking at the hook.")));
    assert!(events.iter().any(|e| matches!(e, AssistantEvent::Usage { input_tokens: 1200, output_tokens: 80, .. })));
    assert!(events.iter().any(|e| matches!(e, AssistantEvent::Started { session_id, .. } if *session_id == token)));
    let proposed = events
        .iter()
        .find_map(|e| match e {
            AssistantEvent::SpecChanged { spec } => Some(spec.clone()),
            _ => None,
        })
        .expect("Codex's edit is proposed for review");
    // Codex wrote the session copy, never the editor document
    assert_eq!(std::fs::read_to_string(dir.join("spec.md")).unwrap(), proposed);
    let hunks = diff_hunks(SPEC, &proposed);
    assert_eq!(hunks.len(), 2, "{hunks:#?}");
    // accepting the first hunk changes only that hunk in the saved spec
    let accepted = apply_hunks(SPEC, &hunks, &[true, false]);
    let d = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(d.path().join("launch.cutproj")).unwrap();
    let mut p = video_studio_core::project::Project::new("Launch");
    p.spec = accepted;
    p.save(&path).unwrap();
    let saved = video_studio_core::project::Project::load(&path).unwrap().spec;
    assert!(saved.contains("- Duration: 4 s") && saved.contains("\nGo.\n") && !saved.contains("Start a free trial"));
    assert_eq!(saved.replace("- Duration: 4 s", "- Duration: 5 s"), SPEC);
}

#[test]
fn resumes_only_the_same_conversation_and_provider() {
    let w = World::new(json!([[{"say": "First."}], [{"say": "Second."}]]));
    let a = w.session("p1", "c1");
    let first = run_turn(&w.request(&a, "Hello", None, "r1.ndjson"), |_, _| {});
    let token = finished(&first).2.unwrap();

    // the second turn of the conversation resumes its thread
    let second = run_turn(&w.request(&a, "Again", Some(&token), "r2.ndjson"), |_, _| {});
    assert_eq!(finished(&second).2.as_deref(), Some(token.as_str()));
    let log = w.log("r2.ndjson");
    assert_eq!((count(&log, "thread/resume"), count(&log, "thread/start")), (1, 0));
    let thread = token.strip_prefix("codex:").unwrap();
    assert_eq!(log.iter().find(|m| m["method"] == "thread/resume").unwrap()["params"]["threadId"], thread);
    assert!(errors(&second).is_empty());

    // another project's conversation never resumes it: a separate thread
    let b = w.session("p2", "c1");
    let other = run_turn(&w.request(&b, "Hello", Some(&token), "r3.ndjson"), |_, _| {});
    let log = w.log("r3.ndjson");
    assert_eq!((count(&log, "thread/resume"), count(&log, "thread/start")), (0, 1));
    assert_ne!(finished(&other).2, Some(token.clone()));

    // a Claude session (switching providers) starts a separate Codex conversation, quietly
    let switched = run_turn(&w.request(&a, "Hello", Some("5f0c2b1e-claude-session"), "r4.ndjson"), |_, _| {});
    let log = w.log("r4.ndjson");
    assert_eq!((count(&log, "thread/resume"), count(&log, "thread/start")), (0, 1));
    assert!(errors(&switched).is_empty());
    assert!(finished(&switched).0);
}

#[test]
fn a_rejected_resume_is_announced_and_starts_exactly_one_thread() {
    let w = World::new(json!([[{"say": "Hi."}]]));
    let a = w.session("p1", "c1");
    let token = finished(&run_turn(&w.request(&a, "Hello", None, "x1.ndjson"), |_, _| {})).2.unwrap();
    let mut req = w.request(&a, "Again", Some(&token), "x2.ndjson");
    req.extra_env.insert("FAKE_CODEX_REJECT_RESUME".into(), "1".into());
    let events = run_turn(&req, |_, _| {});
    let (ok, _, new_token) = finished(&events);
    assert!(ok);
    let notices = errors(&events);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("couldn't continue the earlier conversation") && notices[0].contains("lost"));
    // the notice comes before anything of the new thread
    let notice_at = events.iter().position(|e| matches!(e, AssistantEvent::Error { .. })).unwrap();
    let started_at = events.iter().position(|e| matches!(e, AssistantEvent::Started { .. })).unwrap();
    assert!(notice_at < started_at);
    let log = w.log("x2.ndjson");
    assert_eq!((count(&log, "thread/resume"), count(&log, "thread/start"), count(&log, "turn/start")), (1, 1, 1));
    let new_token = new_token.unwrap();
    assert_ne!(new_token, token);
    // the next turn resumes the new thread
    finished(&run_turn(&w.request(&a, "Once more", Some(&new_token), "x3.ndjson"), |_, _| {}));
    let log = w.log("x3.ndjson");
    assert_eq!(
        log.iter().find(|m| m["method"] == "thread/resume").unwrap()["params"]["threadId"],
        new_token.strip_prefix("codex:").unwrap()
    );
}

#[test]
fn refused_commands_writes_and_unsupported_requests_each_get_an_answer() {
    let w = World::new(json!([]));
    let marker = w.root.join("pwned.txt");
    let outside = w.root.join("outside.md");
    std::fs::write(&outside, "keep me").unwrap();
    std::fs::write(
        &w.script,
        json!([[
            {"command": format!("touch {marker}")},
            {"write": {"path": outside.as_str(), "content": "overwritten"}},
            {"request": {"id": "dyn-1", "method": "item/tool/call", "params": {"threadId": "t", "turnId": "u", "callId": "c", "namespace": null, "tool": "browser", "arguments": {}}}},
            {"request": {"id": "perm-1", "method": "item/permissions/requestApproval", "params": {"threadId": "t", "turnId": "u", "itemId": "i", "environmentId": null, "startedAtMs": 0, "cwd": "/", "reason": "wants the network", "permissions": {"network": {"enabled": true}}}}},
            {"request": {"id": "mcp-1", "method": "mcpServer/elicitation/request", "params": {"threadId": "t", "turnId": "u", "serverName": "x", "message": "token?"}}},
            {"request": {"id": "unk-1", "method": "something/new", "params": {}}},
            {"say": "Done."}
        ]])
        .to_string(),
    )
    .unwrap();
    let dir = w.session("p1", "c1");
    let events = run_turn(&w.request(&dir, "Go", None, "d.ndjson"), |_, _| {});
    assert!(finished(&events).0, "{events:#?}");
    assert!(!marker.exists(), "the refused command never ran");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me", "the external file is untouched");
    let blocked: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AssistantEvent::Blocked { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(blocked.len(), 5, "{blocked:?}");
    assert!(blocked[0].contains("Shell commands aren't available"));
    assert!(blocked[1].contains("isn't allowed") || blocked[1].contains("Only spec.md"), "{}", blocked[1]);
    let log = w.log("d.ndjson");
    let decisions: Vec<&Value> = log
        .iter()
        .filter(|m| {
            m.get("method").is_none()
                && m["id"].as_str().is_some_and(|i| i.starts_with("appr-") || i.starts_with("patch-"))
        })
        .collect();
    assert_eq!(decisions.len(), 2);
    assert!(decisions.iter().all(|d| d["result"]["decision"] == "decline"), "{decisions:?}");
    assert_eq!(response(&log, "dyn-1")["result"]["success"], false);
    assert_eq!(response(&log, "perm-1")["result"], json!({"permissions": {}, "scope": "turn"}));
    assert_eq!(response(&log, "mcp-1")["result"]["action"], "decline");
    assert_eq!(response(&log, "unk-1")["error"]["code"], -32601);
    // the refused tool calls aren't shown as failures of their own
    assert!(!events.iter().any(|e| matches!(e, AssistantEvent::ToolResult { is_error: true, .. })), "{events:#?}");
}

#[test]
fn stop_during_a_question_interrupts_the_turn_and_frees_the_next() {
    let w = World::new(json!([
        [
            {"ask": {"id": "q-9", "questions": [{"id": "len", "header": "Length", "question": "How long?", "isOther": false, "isSecret": false, "options": [{"label": "30 s", "description": ""}]}]}},
            // a real Codex stops at the interrupt; the fake waits for it after the answer
            {"hang": true},
            {"say": "should not be said"}
        ],
        [{"say": "Fresh start."}]
    ]));
    let dir = w.session("p1", "c1");
    let events = run_turn(&w.request(&dir, "Go", None, "s1.ndjson"), |e, h| {
        if matches!(e, AssistantEvent::Question { .. }) {
            h.stop();
        }
    });
    let (ok, stopped, _) = finished(&events);
    assert!(stopped && !ok);
    assert!(errors(&events).is_empty(), "a stop isn't an error: {events:#?}");
    assert!(!events.iter().any(|e| matches!(e, AssistantEvent::Text { text } if text.contains("should not"))));
    let log = w.log("s1.ndjson");
    // the pending question was released with its own ID, then the turn interrupted
    assert_eq!(response(&log, "q-9")["result"]["answers"], json!({}));
    let interrupt = log.iter().find(|m| m["method"] == "turn/interrupt").expect("turn/interrupt");
    assert_eq!(
        (interrupt["params"]["threadId"].as_str(), interrupt["params"]["turnId"].as_str()),
        (Some("thr-1"), Some("turn-1"))
    );
    // nothing is left behind: a new turn starts
    let next = run_turn(&w.request(&dir, "Again", None, "s2.ndjson"), |_, _| {});
    assert!(finished(&next).0);
}

#[test]
fn an_unexpected_exit_and_a_failed_sign_in_are_reported_and_cleaned_up() {
    let w = World::new(json!([[{"say": "Working"}, {"exit": 3}], [{"say": "Recovered."}]]));
    let dir = w.session("p1", "c1");
    let crashed = run_turn(&w.request(&dir, "Go", None, "e1.ndjson"), |_, _| {});
    let (ok, stopped, _) = finished(&crashed);
    assert!(!ok && !stopped);
    let e = errors(&crashed);
    assert!(e.len() == 1 && e[0].contains("Codex exited unexpectedly (status 3)"), "{e:?}");

    let mut signed_out = w.request(&dir, "Go", None, "e2.ndjson");
    signed_out.extra_env.insert("FAKE_CODEX_SIGNED_OUT".into(), "1".into());
    let refused = run_turn(&signed_out, |_, _| {});
    assert!(!finished(&refused).0);
    let e = errors(&refused);
    assert!(e.len() == 1 && e[0].contains("codex login"), "{e:?}");
    assert_eq!(count(&w.log("e2.ndjson"), "thread/start"), 0, "nothing starts without an account");

    // retry works once the problem is gone
    let ok = run_turn(&w.request(&dir, "Go", None, "e3.ndjson"), |_, _| {});
    assert!(finished(&ok).0, "{ok:#?}");
    assert!(ok.iter().any(|e| matches!(e, AssistantEvent::Text { text } if text == "Recovered.")));
}

#[test]
fn a_failed_turn_is_an_error() {
    let w = World::new(json!([[{"fail": "The model is overloaded."}]]));
    let events = run_turn(&w.request(&w.session("p", "c"), "Go", None, "f.ndjson"), |_, _| {});
    assert!(!finished(&events).0);
    assert!(errors(&events).iter().any(|e| e.contains("overloaded")));
}
