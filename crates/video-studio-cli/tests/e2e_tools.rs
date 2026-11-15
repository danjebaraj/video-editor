//! End-to-end tests of the smaller commands and of the core APIs the app uses directly:
//! list / relink on a moved project, the ElevenLabs helper against a fake server, the hook, the
//! spec assistant with per-hunk review, answering questions in a run, and Revise.

// The fakes run commands through `sh`, as Claude Code does on macOS and Linux.
#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::io::Write;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use camino::{Utf8Path, Utf8PathBuf};
use common::*;
use serde_json::{json, Value};
use video_studio_core::assistant::{self, AssistantEvent, AssistantRequest, AssistantSink};
use video_studio_core::claude::broker::{Mode, Policy};
use video_studio_core::claude::locate::{ClaudeInfo, CutlineInfo};
use video_studio_core::eleven::HelperContext;
use video_studio_core::models::ModelList;
use video_studio_core::platform::NoFileRefs;
use video_studio_core::presets::Platforms;
use video_studio_core::project::{RunKind, RunStatus};
use video_studio_core::run::events::{EventKind, MemorySink};
use video_studio_core::run::prepare::Revise;
use video_studio_core::run::{start_run, RunEnv, RunRequest};
use video_studio_core::settings::Settings;

#[test]
fn list_and_relink_a_project_moved_to_a_new_machine() {
    let w = World::new();
    // more media, then everything moves: the footage to another volume, the project elsewhere
    std::fs::create_dir_all(w.root.join("footage/music")).unwrap();
    std::fs::write(w.root.join("footage/music/track.wav"), b"music bytes").unwrap();
    std::fs::write(w.root.join("footage/logo.png"), b"png bytes").unwrap();
    w.edit_project(|p| {
        video_studio_core::media::add_folder(p, Some(&w.project), &w.root.join("footage"), None, &NoFileRefs).unwrap();
    });
    let listing = w.run(&["list", w.project.as_str()]);
    assert_eq!(listing.status.code(), Some(0));
    let text = stdout(&listing);
    assert!(
        text.contains("Social pack")
            && text.contains("YouTube Shorts  1080×1920 9:16 (shorts)")
            && text.contains("ok       m-hero")
            && text.contains("m-track"),
        "{text}"
    );

    std::fs::create_dir_all(w.root.join("Volumes/Work")).unwrap();
    std::fs::rename(w.root.join("footage"), w.root.join("Volumes/Work/footage")).unwrap();
    std::fs::create_dir_all(w.root.join("new-home")).unwrap();
    let moved = w.root.join("new-home/launch.cutproj");
    std::fs::rename(&w.project, &moved).unwrap();
    let j: Value = serde_json::from_str(&stdout(&w.run(&["list", moved.as_str(), "--json"]))).unwrap();
    assert!(j["media"].as_array().unwrap().iter().all(|m| m["status"] == "missing"), "{j}");
    assert_eq!(j["run_configs"][0]["outputs"].as_array().unwrap().len(), 2);

    // a prefix rewrite: shown first, saved only with --write
    let prefix = format!("{}={}", w.root, w.root.join("Volumes/Work"));
    let dry = w.run(&["relink", moved.as_str(), "--prefix", &prefix]);
    assert_eq!(dry.status.code(), Some(0), "{}", stdout(&dry));
    assert!(stdout(&dry).contains("3 relinked, 0 still missing.") && stdout(&dry).contains("Nothing was saved"));
    let still: Value = serde_json::from_str(&stdout(&w.run(&["list", moved.as_str(), "--json"]))).unwrap();
    assert!(
        still["media"].as_array().unwrap().iter().all(|m| m["status"] == "missing"),
        "nothing was written without --write"
    );
    // one file is renamed on the way: only a folder search (by fingerprint) finds it
    std::fs::rename(w.root.join("Volumes/Work/footage/logo.png"), w.root.join("Volumes/Work/footage/brand-logo.png"))
        .unwrap();
    let partial = w.run(&["relink", moved.as_str(), "--prefix", &prefix, "--json"]);
    assert_eq!(partial.status.code(), Some(1), "one item is still missing");
    let pj: Value = serde_json::from_str(&stdout(&partial)).unwrap();
    assert_eq!(pj["still_missing"], json!(["m-logo"]));
    let search = w.run(&["relink", moved.as_str(), "--search", w.root.join("Volumes").as_str(), "--write"]);
    assert_eq!(search.status.code(), Some(0), "{}", stdout(&search));
    assert!(
        stdout(&search).contains("relinked  m-logo")
            && stdout(&search).contains("brand-logo.png")
            && stdout(&search).contains("Saved")
    );
    let after: Value = serde_json::from_str(&stdout(&w.run(&["list", moved.as_str(), "--json"]))).unwrap();
    assert!(after["media"].as_array().unwrap().iter().all(|m| m["status"] == "ok"), "{after}");
    let p = video_studio_core::project::Project::load(&moved).unwrap();
    assert_eq!(p.media_by_id("m-hero").unwrap().paths.relative.as_deref(), Some("../Volumes/Work/footage/hero.mov"));
}

#[test]
fn new_creates_a_project_from_the_intent_template() {
    let w = World::new();
    let path = w.root.join("demo.cutproj");
    let o = w.run(&[
        "new",
        path.as_str(),
        "--intent",
        "product_demo",
        "--name",
        "Grid demo",
        "--media",
        w.root.join("footage").as_str(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let p = video_studio_core::project::Project::load(&path).unwrap();
    assert!(p.spec.starts_with("# Grid demo: product demo\n"));
    assert_eq!(
        p.output_configs.iter().map(|o| o.preset.as_str()).collect::<Vec<_>>(),
        vec!["youtube_1080p", "linkedin"]
    );
    assert_eq!((p.run_configs.len(), p.media.len()), (1, 1));
    assert_eq!(w.run(&["new", path.as_str()]).status.code(), Some(2), "an existing file isn't overwritten");
    // the template's placeholders are warnings, not blockers
    let v = w.run(&["validate", path.as_str(), "--config", "Default"]);
    assert_eq!(v.status.code(), Some(1), "{}", stdout(&v));
    assert!(stdout(&v).contains("Placeholder text"));
}

fn helper_context(w: &World, url: &str, cap: u64, mode: Mode) -> (Utf8PathBuf, Utf8PathBuf) {
    let run = w.root.join("run");
    std::fs::create_dir_all(run.join("logs")).unwrap();
    let ctx = HelperContext {
        run_id: "run_t".into(),
        run_folder: run.clone(),
        mode,
        logs_dir: run.join("logs"),
        cutline: Some(FAKE_CUTLINE.into()),
        credit_cap: cap,
        cache_dir: w.lib.join("cache/eleven"),
        cache_limit_bytes: 0,
        default_voice: Some("v1".into()),
        default_voice_settings: None,
        default_model: None,
        base_url: Some(url.to_string()),
        test_api_key: None,
        capture: Some(Default::default()),
    };
    let file = w.root.join("context.json");
    std::fs::write(&file, serde_json::to_string(&ctx).unwrap()).unwrap();
    (run, file)
}

#[test]
fn eleven_helper_budget_cache_and_logging() {
    let w = World::new();
    let server = fake_eleven();
    let (run, ctx) = helper_context(&w, &server.url, 60, Mode::Production);
    let eleven = |args: &[&str]| {
        let mut full = vec!["eleven"];
        full.extend_from_slice(args);
        w.command(&full).env("VIDEO_STUDIO_RUN_CONTEXT", ctx.as_str()).current_dir(&run).output().unwrap()
    };
    // outside a run the helper refuses
    let outside = w.run(&["eleven", "voices"]);
    assert_eq!(outside.status.code(), Some(126));
    assert!(stderr(&outside).contains("only works inside a Syncfusion Video Studio run"));

    std::fs::write(run.join("vo.txt"), "Meet the fastest way to ship.\n").unwrap();
    let quote = eleven(&["quote", "tts", "--text-file", "vo.txt"]);
    assert_eq!(stdout(&quote).trim(), "about 29 credits (0 of 60 used in this run, 60 left)");
    assert_eq!(server.count("/v1/text-to-speech"), 0, "a quote generates nothing");
    // the brand's default voice is used when none is given
    let tts = eleven(&["tts", "--text-file", "vo.txt", "--out", "scene1.mp3"]);
    assert_eq!(tts.status.code(), Some(0), "{}", stderr(&tts));
    assert_eq!(stdout(&tts).trim(), "audio/scene1.mp3 (29 credits; 29 of 60 used in this run)");
    assert_eq!(
        std::fs::read_to_string(run.join("audio/scene1.mp3")).unwrap(),
        "FAKE-MP3 voice text: Meet the fastest way to ship."
    );
    assert!(server.requests.lock().unwrap().iter().any(|r| r.contains("/v1/text-to-speech/v1?")));
    // identical request: cache hit, no API call, 0 credits
    let again = eleven(&["tts", "--text-file", "vo.txt", "--out", "scene1-copy.mp3"]);
    assert_eq!(stdout(&again).trim(), "audio/scene1-copy.mp3 (from cache, 0 credits; 29 of 60 used in this run)");
    assert_eq!(server.count("/v1/text-to-speech"), 1);
    // a call that would pass the cap fails with a message Claude can act on
    let over = eleven(&[
        "tts",
        "--voice",
        "v2",
        "--text",
        "This second line is long enough to pass the cap.",
        "--out",
        "scene2.mp3",
    ]);
    assert_eq!(over.status.code(), Some(3));
    assert!(
        stderr(&over).contains("about 48 credits")
            && stderr(&over).contains("used 29 of its 60 credit cap (31 left)")
            && stderr(&over).contains("Shorten the text"),
        "{}",
        stderr(&over)
    );
    assert_eq!(server.count("/v1/text-to-speech"), 1, "the API isn't called for a refused request");
    assert!(!run.join("audio/scene2.mp3").exists());
    // a shorter text fits
    let fits = eleven(&["tts", "--voice", "v2", "--text", "Short line.", "--out", "scene2.mp3"]);
    assert_eq!(stdout(&fits).trim(), "audio/scene2.mp3 (11 credits; 40 of 60 used in this run)");
    // sound effects, transcripts and voices; music is over the remaining budget
    assert!(stderr(&eleven(&["music", "--prompt", "calm piano", "--length", "30", "--out", "bed.mp3"]))
        .contains("about 900 credits"));
    assert_eq!(eleven(&["sfx", "--prompt", "whoosh", "--length", "0.4", "--out", "whoosh.mp3"]).status.code(), Some(0));
    assert_eq!(
        eleven(&["sfx", "--prompt", "whoosh", "--length", "90", "--out", "long.mp3"]).status.code(),
        Some(1),
        "length is checked"
    );
    let voices = stdout(&eleven(&["voices", "--search", "narrator"]));
    assert!(voices.contains("v1  Aria  [premade]  — Warm narrator") && voices.contains("v2  Bram"));
    // out paths can't leave the run folder
    let escape = eleven(&["tts", "--text", "Hi", "--out", "../../stolen.mp3"]);
    assert_eq!(escape.status.code(), Some(126));
    assert!(stderr(&escape).contains("inside the run folder"));
    // every call is logged, and the key never appears
    let log = ndjson(&run.join("logs/eleven.ndjson"));
    let rows: Vec<(String, bool, u64, String)> = log
        .iter()
        .map(|l| {
            (
                l["command"].as_str().unwrap().to_string(),
                l["ok"].as_bool().unwrap(),
                l["credits"].as_u64().unwrap(),
                l["cache"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(rows[0], ("tts".into(), true, 29, "miss".into()));
    assert_eq!(rows[1], ("tts".into(), true, 0, "hit".into()));
    assert_eq!((rows[2].1, rows[2].2), (false, 0));
    assert_eq!(rows[3], ("tts".into(), true, 11, "miss".into()));
    assert!(
        log[0]["request"]["text"].as_str().unwrap().contains("fastest way")
            && log[0]["out"].as_str().unwrap().ends_with("audio/scene1.mp3")
            && log[0]["response"]["request_id"] == "req-test"
    );
    assert!(!std::fs::read_to_string(run.join("logs/eleven.ndjson")).unwrap().contains("test-key"));

    // scribe with enough budget, in a fresh run context
    let (run2, ctx2) = {
        std::fs::remove_dir_all(&run).unwrap();
        helper_context(&w, &server.url, 10_000, Mode::Production)
    };
    std::fs::create_dir_all(run2.join("audio")).unwrap();
    std::fs::write(run2.join("audio/vo.mp3"), b"audio").unwrap();
    let scribe = w
        .command(&["eleven", "scribe", "--in", "audio/vo.mp3", "--out", "vo.json"])
        .env("VIDEO_STUDIO_RUN_CONTEXT", ctx2.as_str())
        .current_dir(&run2)
        .output()
        .unwrap();
    assert_eq!(scribe.status.code(), Some(0), "{}", stderr(&scribe));
    let words: Value = serde_json::from_str(&std::fs::read_to_string(run2.join("audio/vo.json")).unwrap()).unwrap();
    assert_eq!(words["words"][1]["text"], "there");

    // a wrong key is reported from the API's own message; no key at all is explained
    std::fs::write(&w.secrets, json!({"elevenlabs-api-key": "wrong"}).to_string()).unwrap();
    let bad = w
        .command(&["eleven", "tts", "--text", "Fresh text", "--out", "x.mp3"])
        .env("VIDEO_STUDIO_RUN_CONTEXT", ctx2.as_str())
        .current_dir(&run2)
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(1));
    assert_eq!(stderr(&bad).trim(), "ElevenLabs returned 401 (the API key was rejected): Invalid API key");
    std::fs::write(&w.secrets, "{}").unwrap();
    let none = w
        .command(&["eleven", "tts", "--text", "Fresh text", "--out", "x.mp3"])
        .env("VIDEO_STUDIO_RUN_CONTEXT", ctx2.as_str())
        .current_dir(&run2)
        .output()
        .unwrap();
    assert!(stderr(&none).contains("No ElevenLabs API key is set"));
}

#[test]
fn secret_command_stores_masks_and_tests_the_key() {
    let mut w = World::new();
    let server = fake_eleven();
    w.eleven_url = Some(server.url.clone());
    std::fs::write(&w.secrets, "{}").unwrap();
    assert!(stdout(&w.run(&["secret", "status"])).contains("No ElevenLabs key is stored."));
    let mut set = w.command(&["secret", "set"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    set.stdin.take().unwrap().write_all(b"test-key\n").unwrap();
    assert!(set.wait().unwrap().success());
    let status = stdout(&w.run(&["secret", "status", "--json"]));
    assert_eq!(serde_json::from_str::<Value>(&status).unwrap(), json!({"present": true, "masked": "••••-key"}));
    assert!(!status.contains("test-key"), "the key is only ever shown masked");
    let test: Value = serde_json::from_str(&stdout(&w.run(&["secret", "test", "--json"]))).unwrap();
    assert_eq!((test["ok"].as_bool(), test["remaining"].as_u64()), (Some(true), Some(99_000)));
    assert!(w.run(&["secret", "delete"]).status.success());
    assert_eq!(w.run(&["secret", "test"]).status.code(), Some(5));
}

#[test]
fn hook_command_applies_the_policy_file() {
    let w = World::new();
    let work = w.root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let policy = Policy {
        mode: Mode::Assistant,
        work_dir: work.clone(),
        read_paths: vec![],
        write_files: Some(vec![work.join("spec.md")]),
        cutline: None,
        helper: None,
        capture: false,
        path_dirs: vec![],
        web_access: false,
        log: Some(w.root.join("perm.ndjson")),
    };
    let file = w.root.join("policy.json");
    std::fs::write(&file, serde_json::to_string(&policy).unwrap()).unwrap();
    let call = |input: Value| -> Value {
        let mut c = w
            .command(&["hook", "--policy", file.as_str()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        serde_json::from_slice(&c.wait_with_output().unwrap().stdout).unwrap()
    };
    assert_eq!(
        call(json!({"tool_name": "Edit", "tool_input": {"file_path": "spec.md"}}))["hookSpecificOutput"]
            ["permissionDecision"],
        "allow"
    );
    let deny = call(json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}));
    assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(deny["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains("spec assistant"));
    assert_eq!(call(json!({"tool_name": "AskUserQuestion", "tool_input": {}})), json!({}));
    assert_eq!(ndjson(&w.root.join("perm.ndjson")).len(), 3);
    // a missing policy blocks everything rather than allowing it
    std::fs::remove_file(&file).unwrap();
    assert_eq!(
        call(json!({"tool_name": "Read", "tool_input": {"file_path": "spec.md"}}))["hookSpecificOutput"]
            ["permissionDecision"],
        "deny"
    );
}

#[derive(Default)]
struct Collect(Mutex<Vec<AssistantEvent>>);
impl AssistantSink for Collect {
    fn emit(&self, e: &AssistantEvent) {
        self.0.lock().unwrap().push(e.clone());
    }
}

fn fake_claude_info() -> ClaudeInfo {
    ClaudeInfo {
        path: FAKE_CLAUDE.into(),
        version: "2.1.311".into(),
        version_text: "2.1.311 (Claude Code)".into(),
        effort_levels: vec!["low".into(), "high".into()],
        login_path: None,
        supported: true,
    }
}

#[test]
fn assistant_edits_arrive_as_hunks_and_accepted_hunks_reach_the_spec() {
    let w = World::new();
    let session = w.root.join("assistant/s1");
    let spec_path = session.join("spec.md");
    let docs = w.root.join("docs-shared");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("brief.md"), "The brief.").unwrap();
    w.set_script(json!([
        {"say": "I'll tighten the hook and add a shot."},
        {"tool": "Read", "input": {"file_path": spec_path.to_string()}},
        {"tool": "AskUserQuestion", "input": {"questions": [{"question": "How long should the hook be?", "multiSelect": false, "options": [{"label": "10 s"}, {"label": "20 s"}]}]}},
        {"tool": "Edit", "input": {"file_path": spec_path.to_string(), "old_string": "- **Duration:** 20 s", "new_string": "- **Duration:** 10 s"}},
        {"tool": "Edit", "input": {"file_path": spec_path.to_string(), "old_string": "Start a free trial.", "new_string": "Start a free trial at example.com."}},
        {"tool": "Write", "input": {"file_path": session.join("notes.md").to_string(), "content": "x"}},
        {"tool": "Write", "input": {"file_path": w.project.to_string(), "content": "{}"}},
        {"tool": "Bash", "input": {"command": "rm -rf ~"}},
        {"tool": "Read", "input": {"file_path": w.root.join("footage/hero.mov").to_string()}},
        {"tool": "Read", "input": {"file_path": w.secrets.to_string()}},
        {"tool": "Read", "input": {"file_path": docs.join("brief.md").to_string()}},
        {"tool": "Write", "input": {"file_path": docs.join("scribble.md").to_string(), "content": "x"}},
        {"result": {"text": "Done.", "cost": 0.02}}
    ]));
    let mut extra_env = BTreeMap::new();
    extra_env.insert("FAKE_CLAUDE_SCRIPT".to_string(), w.script.to_string());
    extra_env.insert("FAKE_CLAUDE_TRACE".to_string(), w.trace.to_string());
    let request = AssistantRequest {
        session_dir: session.clone(),
        spec: SPEC.into(),
        prompt: "Tighten the hook.".into(),
        model: "sonnet".into(),
        effort: Some("high".into()),
        web_access: false,
        resume: Some("earlier-session".into()),
        read_paths: vec![w.root.join("footage")],
        reference_paths: vec![docs.clone()],
        project_summary: "Project: Launch teaser".into(),
        helper: Some(VIDEO_STUDIO.into()),
        extra_env,
    };
    let sink = Arc::new(Collect::default());
    let handle = assistant::start(&request, &fake_claude_info(), sink.clone()).unwrap();
    // answer the question card when it appears
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let q = sink.0.lock().unwrap().iter().find_map(|e| match e {
            AssistantEvent::Question { id, questions } => Some((id.clone(), questions[0].question.clone())),
            _ => None,
        });
        if let Some((id, question)) = q {
            let mut a = BTreeMap::new();
            a.insert(question, "10 s".to_string());
            handle.answer(&id, a);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no question arrived: {:?}", sink.0.lock().unwrap());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    handle.wait();
    let events = sink.0.lock().unwrap().clone();
    assert!(
        matches!(events.last(), Some(AssistantEvent::Finished { ok: true, stopped: false, session_id: Some(s) }) if s == "earlier-session"),
        "{events:?}"
    );
    assert!(events.iter().any(|e| matches!(e, AssistantEvent::Text { text } if text.contains("tighten the hook"))));
    assert!(events
        .iter()
        .any(|e| matches!(e, AssistantEvent::Usage { cost_usd: Some(c), .. } if (*c - 0.02).abs() < 1e-9)));
    let blocked: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            AssistantEvent::Blocked { reason } => Some(reason),
            _ => None,
        })
        .collect();
    assert!(blocked.is_empty(), "with the hook installed, denials come back as failed tool calls: {blocked:?}");
    // the session resumed, ran in its scratch folder, with the assistant's tools and no Bash
    let trace = w.trace();
    let args: Vec<&str> = trace[0]["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    assert!(
        args.windows(2).any(|p| p == ["--resume", "earlier-session"])
            && args.windows(2).any(|p| p == ["--model", "sonnet"])
            && args.windows(2).any(|p| p == ["--effort", "high"])
    );
    assert!(
        args.windows(2).any(|p| p == ["--tools", "Read,Edit,Write,Glob,Grep,AskUserQuestion"])
            && args.windows(2).any(|p| p == ["--disallowed-tools", "Bash"])
    );
    let calls = w.tool_calls();
    let verdict = |needle: &str| calls.iter().find(|c| c.3.contains(needle)).map(|c| c.1.clone()).unwrap();
    assert_eq!(verdict("notes.md"), "deny", "only spec.md can be edited");
    assert_eq!(verdict("launch.cutproj"), "deny");
    assert_eq!(verdict("rm -rf"), "deny");
    assert_eq!(verdict("hero.mov"), "allow", "media folders are readable");
    assert_eq!(verdict("secrets.json"), "deny");
    assert_eq!(verdict("brief.md"), "allow", "folders the user shared are readable");
    assert_eq!(verdict("scribble.md"), "deny", "shared folders are read-only");
    assert!(!docs.join("scribble.md").exists());
    assert!(args.windows(2).any(|p| p[0] == "--add-dir" && p[1].ends_with("docs-shared")), "{args:?}");
    let ask = trace.iter().find(|t| t["tool"] == "AskUserQuestion").unwrap();
    assert_eq!(ask["input"]["answers"]["How long should the hook be?"], "10 s", "the answer reaches Claude");
    assert!(!session.join("notes.md").exists());
    assert_ne!(std::fs::read_to_string(&w.project).unwrap(), "{}");

    // The edits arrive as the whole new spec; the shell diffs it against the editor's text.
    let new_spec = events
        .iter()
        .rev()
        .find_map(|e| match e {
            AssistantEvent::SpecChanged { spec } => Some(spec.clone()),
            _ => None,
        })
        .expect("a spec change");
    let hunks = assistant::diff_hunks(SPEC, &new_spec);
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks[0].new_lines, vec!["- **Duration:** 10 s"]);
    // Accept the first hunk, reject the second, and write the result back to the project.
    let reviewed = assistant::apply_hunks(SPEC, &hunks, &[true, false]);
    assert!(
        reviewed.contains("- **Duration:** 10 s")
            && reviewed.contains("Start a free trial.\n")
            && !reviewed.contains("example.com")
    );
    w.edit_project(|p| p.spec = reviewed.clone());
    assert_eq!(w.load_project().spec, reviewed, "accepted hunks are written to the project's spec field");
    assert_eq!(assistant::apply_hunks(SPEC, &hunks, &[false, false]), SPEC, "rejecting restores the previous text");
}

#[test]
fn assistant_can_be_stopped() {
    let w = World::new();
    w.set_script(json!([{"say": "Thinking."}, {"hang": true}]));
    let mut extra_env = BTreeMap::new();
    extra_env.insert("FAKE_CLAUDE_SCRIPT".to_string(), w.script.to_string());
    let request = AssistantRequest {
        session_dir: w.root.join("assistant/s2"),
        spec: SPEC.into(),
        prompt: "Hi".into(),
        model: "sonnet".into(),
        effort: None,
        web_access: false,
        resume: None,
        read_paths: vec![],
        reference_paths: vec![],
        project_summary: String::new(),
        helper: None,
        extra_env,
    };
    let sink = Arc::new(Collect::default());
    let handle = assistant::start(&request, &fake_claude_info(), sink.clone()).unwrap();
    while !sink.0.lock().unwrap().iter().any(|e| matches!(e, AssistantEvent::Text { .. })) {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    handle.stop();
    handle.wait();
    assert!(handle.is_finished());
    let events = sink.0.lock().unwrap().clone();
    assert!(matches!(events.last(), Some(AssistantEvent::Finished { ok: false, stopped: true, .. })), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, AssistantEvent::Error { .. })), "stopping isn't an error");
}

fn run_env(w: &World, script: &Utf8Path) -> RunEnv {
    let mut extra_env = std::collections::HashMap::new();
    extra_env.insert("FAKE_CLAUDE_SCRIPT".to_string(), script.to_string());
    extra_env.insert("FAKE_CLAUDE_TRACE".to_string(), w.trace.to_string());
    RunEnv {
        library: w.lib.clone(),
        settings: Settings::default(),
        platforms: Platforms::bundled(),
        models: ModelList::bundled(),
        claude: fake_claude_info(),
        cutline: CutlineInfo { path: FAKE_CUTLINE.into(), version: "0.1.0-fake".into() },
        helper: VIDEO_STUDIO.into(),
        cutline_home: None,
        cutline_doctor: None,
        eleven_base_url: Some("http://127.0.0.1:9".into()),
        eleven_test_key: None,
        eleven_key_present: false,
        extra_env,
        file_refs: Arc::new(NoFileRefs),
    }
}

#[test]
fn a_question_answered_in_the_app_reaches_claude_and_revise_resumes_the_session() {
    let w = World::new();
    let mut steps = vec![
        json!({"tool": "AskUserQuestion", "input": {"questions": [{"question": "Which logo opens the video?", "multiSelect": false, "options": [{"label": "Horizontal"}, {"label": "Icon"}]}]}}),
    ];
    steps.extend(
        happy_script().as_array().unwrap().iter().filter(|s| !s.to_string().contains("eleven")).skip(2).cloned(),
    );
    w.set_script(Value::Array(steps));
    let env = run_env(&w, &w.script);
    let sink = Arc::new(MemorySink::default());
    let request = RunRequest {
        project_path: w.project.clone(),
        run_config: "rc_social".into(),
        kind: RunKind::Production,
        model: Some("claude-sonnet-5-5".into()),
        output: None,
        revise: None,
        assume_answers: false,
        question_timeout_s: None,
        validation: None,
    };
    let handle = start_run(&request, &env, sink.clone()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let id = loop {
        let q = sink.events().iter().find_map(|e| match &e.kind {
            EventKind::Question { id, .. } => Some(id.clone()),
            _ => None,
        });
        if let Some(id) = q {
            break id;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(!handle.is_finished(), "the run waits for the answer");
    let mut answers = BTreeMap::new();
    answers.insert("Which logo opens the video?".to_string(), "Icon".to_string());
    handle.answer(&id, answers);
    let outcome = handle.wait();
    assert_eq!(outcome.status, RunStatus::Succeeded, "{:?}", outcome.message);
    assert_eq!(
        outcome.record.model.as_deref(),
        Some("claude-sonnet-5-5"),
        "the per-run model override is used and recorded"
    );
    assert!(sink.events().iter().any(|e| matches!(&e.kind, EventKind::QuestionAnswered { by, answers, .. } if by == "user" && answers.get("Which logo opens the video?").map(String::as_str) == Some("Icon"))));
    let ask = w.trace().into_iter().find(|t| t["tool"] == "AskUserQuestion").unwrap();
    assert_eq!(
        (ask["decision"].as_str(), ask["input"]["answers"]["Which logo opens the video?"].as_str()),
        (Some("allow"), Some("Icon"))
    );
    let first = w.load_project().runs[0].clone();

    // Revise: a new run folder that links to the previous one and resumes its Claude session.
    std::fs::remove_file(&w.trace).unwrap();
    let revise_script = w.root.join("revise.json");
    std::fs::write(&revise_script, json!([
        {"tool": "Read", "input": {"file_path": "previous-summary.md"}},
        {"tool": "Edit", "input": {"file_path": "project.json5", "old_string": "name: \"launch\"", "new_string": "name: \"launch v2\""}},
        {"tool": "Bash", "input": {"command": "cutline render project.json5"}},
        {"tool": "Write", "input": {"file_path": "summary.md", "content": "## What changed\nFaster hook.\n"}},
    ]).to_string()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let env2 = run_env(&w, &revise_script);
    let request2 = RunRequest {
        revise: Some(Revise { previous_run: first.id.clone(), feedback: "Make the hook faster.".into() }),
        model: None,
        ..request.clone()
    };
    let outcome2 = start_run(&request2, &env2, Arc::new(MemorySink::default())).unwrap().wait();
    assert_eq!(outcome2.status, RunStatus::Succeeded, "{:?}", outcome2.message);
    assert_ne!(outcome2.folder, first.folder);
    assert_eq!(outcome2.record.previous_run.as_deref(), Some(first.id.as_str()));
    let trace = w.trace();
    let args: Vec<&str> = trace[0]["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    assert!(
        args.windows(2).any(|p| p[0] == "--resume" && p[1] == first.claude_session.as_deref().unwrap()),
        "{args:?}"
    );
    assert!(trace.iter().any(|t| t["prompt"]
        .as_str()
        .map(|p| p.contains("Make the hook faster.") && p.contains(&first.folder))
        .unwrap_or(false)));
    let folder2 = Utf8Path::new(&outcome2.folder);
    assert!(
        std::fs::read_to_string(folder2.join("project.json5")).unwrap().contains("launch v2"),
        "the previous project.json5 was carried over and edited"
    );
    let run_json: Value =
        serde_json::from_str(&std::fs::read_to_string(folder2.join("logs/run.json")).unwrap()).unwrap();
    assert_eq!(run_json["previous_run"]["id"], first.id.as_str());
    assert_eq!(run_json["revise_feedback"], "Make the hook faster.");
    assert_eq!(w.load_project().runs.len(), 2);
}

#[test]
fn a_question_times_out_onto_the_assumption_and_a_turn_limit_fails_the_run() {
    let w = World::new();
    let mut steps = vec![
        json!({"tool": "AskUserQuestion", "input": {"questions": [{"question": "Music or silence?", "multiSelect": false, "options": [{"label": "Music"}, {"label": "Silence"}]}]}}),
    ];
    steps.extend(
        happy_script().as_array().unwrap().iter().filter(|s| !s.to_string().contains("eleven")).skip(2).cloned(),
    );
    w.set_script(Value::Array(steps));
    let env = run_env(&w, &w.script);
    let sink = Arc::new(MemorySink::default());
    let request = RunRequest {
        project_path: w.project.clone(),
        run_config: "rc_social".into(),
        kind: RunKind::Production,
        model: None,
        output: None,
        revise: None,
        assume_answers: false,
        question_timeout_s: Some(1),
        validation: None,
    };
    let outcome = start_run(&request, &env, sink.clone()).unwrap().wait();
    assert_eq!(outcome.status, RunStatus::SucceededWithWarnings);
    assert!(sink.events().iter().any(|e| matches!(&e.kind, EventKind::QuestionAnswered { by, .. } if by == "timeout")));
    assert!(outcome.warnings.iter().any(|w| w.contains("Music or silence?") && w.contains("no answer in time")));

    // a turn limit below what the script needs
    std::thread::sleep(std::time::Duration::from_millis(1100));
    w.edit_project(|p| p.run_configs[0].max_turns = Some(2));
    w.set_script(happy_script());
    let limited = RunRequest { question_timeout_s: None, ..request };
    let outcome = start_run(&limited, &env, Arc::new(MemorySink::default())).unwrap().wait();
    assert_eq!(outcome.status, RunStatus::Failed);
    assert!(outcome.message.unwrap().contains("limit of 2 turns"));
}

#[test]
fn the_committed_project_schema_is_current() {
    let w = World::new();
    let printed = stdout(&w.run(&["schema"]));
    let committed = include_str!("../../../schema/cutproj.schema.json");
    assert_eq!(printed.trim(), committed.trim(), "run `video-studio schema > schema/cutproj.schema.json`");
    let v: Value = serde_json::from_str(committed).unwrap();
    assert!(v["properties"]["media"].is_object() && v["properties"]["run_configs"].is_object());
}
