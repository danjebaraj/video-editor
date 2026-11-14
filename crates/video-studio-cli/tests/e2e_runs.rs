//! End-to-end runs through the `video-studio` CLI with the fake Claude Code CLI, a fake cutline
//! and a fake ElevenLabs server (ui/spec.md §20): validate → run → logs, JSON events, exit
//! codes, the permission broker, the dry run's limits, and cancellation.

// The fakes run commands through `sh`, as Claude Code does on macOS and Linux.
#![cfg(unix)]

mod common;

use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::time::{Duration, Instant};

use camino::Utf8Path;
use common::*;
use serde_json::{json, Value};
use video_studio_core::project::{RunKind, RunStatus};

#[test]
fn full_run_produces_outputs_events_and_complete_logs() {
    let mut w = World::new();
    let eleven = fake_eleven();
    w.eleven_url = Some(eleven.url.clone());
    w.set_script(happy_script());
    let o = w.run(&["run", w.project.as_str(), "--config", "Social pack", "--json", "--yes"]);
    assert_eq!(o.status.code(), Some(0), "stdout:\n{}\nstderr:\n{}", stdout(&o), stderr(&o));

    // Events: the schema the spec lists, in a sensible order.
    let ev = events(&o);
    let n = names(&ev);
    for expected in
        ["run_started", "stage_started", "progress", "message", "artifact", "cost", "stage_finished", "run_finished"]
    {
        assert!(n.contains(&expected.to_string()), "no {expected} event in {n:?}");
    }
    assert_eq!(n.first().map(String::as_str), Some("run_started"));
    assert_eq!(n.last().map(String::as_str), Some("run_finished"));
    let stages: Vec<&str> =
        ev.iter().filter(|e| e["event"] == "stage_started").map(|e| e["stage"].as_str().unwrap()).collect();
    assert_eq!(
        stages,
        vec!["prepare", "authoring", "validate", "audio", "review", "render", "done", "done"],
        "stages derive from Claude's tool calls"
    );
    assert!(
        ev.iter().any(|e| e["event"] == "progress"
            && e["fraction"] == 0.5
            && e["detail"].as_str().unwrap_or("").contains("segment 1/2")),
        "cutline's progress lines become progress events"
    );
    let fin = ev.last().unwrap();
    assert_eq!(
        (fin["status"].as_str(), fin["exit_code"].as_i64(), fin["cost_usd"].as_f64(), fin["eleven_credits"].as_u64()),
        (Some("succeeded"), Some(0), Some(0.5), Some(11))
    );
    let outputs: Vec<&str> = ev
        .iter()
        .filter(|e| e["event"] == "artifact" && e["kind"] == "output")
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert_eq!(outputs.len(), 2);

    // The run folder: prepared inputs, outputs per output config, and every log the spec lists.
    let run = w.run_folder();
    assert!(run.file_name().unwrap().ends_with("-social-pack"), "{run}");
    for f in [
        "run.json",
        "spec.md",
        "media-manifest.json",
        "outputs.json",
        "project.json5",
        "summary.md",
        "audio/vo.mp3",
        "outputs/youtube-shorts/youtube-shorts.mp4",
        "outputs/linkedin/linkedin.mp4",
        "logs/run.json",
        "logs/events.ndjson",
        "logs/claude.jsonl",
        "logs/eleven.ndjson",
        "logs/validation.json",
        "logs/summary.md",
        "logs/permissions.ndjson",
        "logs/cutline/001-validate.log",
        "logs/cutline/002-review.log",
        "logs/cutline/003-render.log",
        "logs/qa/youtube-shorts.qa.json",
    ] {
        assert!(run.join(f).is_file(), "missing {f}");
    }
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("media-manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest[0]["id"], "m-hero");
    assert_eq!(
        manifest[0]["path"].as_str().unwrap(),
        w.root.join("footage/hero.mov").as_str(),
        "media is referenced in place"
    );
    assert_eq!(manifest[0]["probe"]["width"], 1920);
    let run_json: Value = serde_json::from_str(&std::fs::read_to_string(run.join("logs/run.json")).unwrap()).unwrap();
    assert_eq!(run_json["status"], "succeeded");
    assert_eq!(run_json["claude"]["version"], "2.1.311 (Claude Code)");
    assert_eq!(run_json["cutline"]["version"], "0.1.0-fake");
    assert_eq!(run_json["model_used"], "opus", "the model actually used is recorded");
    assert!(
        run_json["started"].is_string()
            && run_json["finished"].is_string()
            && run_json["app"]["core_version"].is_string()
            && run_json["os"]["family"].is_string()
    );
    assert_eq!(run_json["results"].as_array().unwrap().len(), 2);
    let render_log = std::fs::read_to_string(run.join("logs/cutline/003-render.log")).unwrap();
    assert!(
        render_log.contains("# argv:")
            && render_log.contains("\"render\"")
            && render_log.contains("[err] render: segment 2/2")
            && render_log.contains("# exit: 0")
            && render_log.contains("# duration:")
    );
    let eleven_log = ndjson(&run.join("logs/eleven.ndjson"));
    assert_eq!(eleven_log.len(), 1);
    assert_eq!(
        (
            eleven_log[0]["command"].as_str(),
            eleven_log[0]["credits"].as_u64(),
            eleven_log[0]["cache"].as_str(),
            eleven_log[0]["ok"].as_bool()
        ),
        (Some("tts"), Some(11), Some("miss"), Some(true))
    );
    assert!(
        !std::fs::read_to_string(run.join("logs/eleven.ndjson")).unwrap().contains("test-key"),
        "the key never reaches a log"
    );
    assert!(!std::fs::read_to_string(run.join("logs/claude.jsonl")).unwrap().contains("test-key"));
    assert_eq!(ndjson(&run.join("logs/events.ndjson")).len(), ev.len(), "events.ndjson is the same stream as --json");
    let summary = std::fs::read_to_string(run.join("logs/summary.md")).unwrap();
    assert!(
        summary.contains("## What I made")
            && summary.contains("Status: **Succeeded**")
            && summary.contains("ElevenLabs credits: 11 of 500")
            && summary.contains("| YouTube Shorts |")
    );
    assert_eq!(std::fs::read_to_string(run.join("audio/vo.mp3")).unwrap(), "FAKE-MP3 voice text: Hello there");
    // nothing was copied: the hero shot isn't in the run folder
    assert!(!walk(&run).iter().any(|p| p.ends_with("hero.mov")));

    // Claude was launched in the run folder with the hook and the helper context.
    let trace = w.trace();
    let start = &trace[0];
    assert_eq!(start["cwd"].as_str().unwrap(), run.as_str());
    assert_eq!(start["context_env"], true);
    let args: Vec<&str> = start["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    assert!(
        args.contains(&"--settings")
            && args.contains(&"--permission-prompt-tool")
            && args.iter().any(|a| a.contains("Read,Write,Edit,Glob,Grep,Bash,AskUserQuestion,TodoWrite"))
    );
    let prompt_arg = args[args.iter().position(|a| *a == "--append-system-prompt").unwrap() + 1];
    assert!(
        prompt_arg.contains("media-manifest.json")
            && prompt_arg.contains("outputs/linkedin/linkedin.mp4")
            && prompt_arg.contains("# cutline workflow (fake)")
            && prompt_arg.contains("at most 500 ElevenLabs credits")
    );
    assert!(w.tool_calls().iter().all(|c| c.1 == "allow" && c.2 == "hook"), "{:?}", w.tool_calls());

    // Run history in the project file.
    let p = w.load_project();
    assert_eq!(p.runs.len(), 1);
    let r = &p.runs[0];
    assert_eq!(
        (r.status, r.kind, r.cost_usd, r.eleven_credits, r.run_config.as_str()),
        (RunStatus::Succeeded, RunKind::Production, Some(0.5), Some(11), "rc_social")
    );
    assert_eq!(r.folder, run.as_str());
    assert!(r.claude_session.as_deref().unwrap().starts_with("fake-session-") && r.finished.is_some());
    assert_eq!(p.spec, SPEC, "a run changes nothing but the history");

    // A second run with the same script: the voice-over comes from the cache and costs nothing.
    std::thread::sleep(Duration::from_millis(1100));
    let o2 = w.run(&["run", w.project.as_str(), "--config", "rc_social", "--json", "--yes"]);
    assert_eq!(o2.status.code(), Some(0), "{}", stderr(&o2));
    assert_eq!(eleven.count("/v1/text-to-speech/"), 1, "the API was called once across both runs");
    let run2 = w.run_folder();
    assert_ne!(run2, run);
    let log2 = ndjson(&run2.join("logs/eleven.ndjson"));
    assert_eq!((log2[0]["cache"].as_str(), log2[0]["credits"].as_u64()), (Some("hit"), Some(0)));
    assert_eq!(events(&o2).last().unwrap()["eleven_credits"], 0);
    assert_eq!(w.load_project().runs.len(), 2);

    // runs / logs subcommands
    let runs = w.run(&["runs", w.project.as_str()]);
    assert!(stdout(&runs).contains("Succeeded") && stdout(&runs).contains(run.as_str()));
    let zip = w.root.join("logs.zip");
    let z = w.run(&["logs", run.as_str(), "--zip", zip.as_str(), "--redact"]);
    assert_eq!(z.status.code(), Some(0), "{}", stderr(&z));
    assert!(zip.is_file() && stdout(&z).contains("media paths redacted"));
    let report = w.run(&["logs", run.as_str(), "--issue-report"]);
    assert!(
        stdout(&report).contains("# cutline issue report")
            && stdout(&report).contains("No cutline command failed (3 ran).")
    );
    let listing = w.run(&["logs", run.as_str()]);
    assert!(stdout(&listing).contains("claude.jsonl") && stdout(&listing).contains("cutline/003-render.log"));
}

fn walk(dir: &Utf8Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = camino::Utf8PathBuf::from_path_buf(e.path()).unwrap();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(p);
            } else {
                out.push(p.to_string());
            }
        }
    }
    out
}

#[test]
fn broker_blocks_everything_outside_the_run_scope() {
    let w = World::new();
    std::fs::write(w.root.join("private.txt"), "secret").unwrap();
    let outside = w.root.join("private.txt");
    w.set_script(json!([
        {"tool": "Write", "input": {"file_path": "project.json5", "content": PROJECT_JSON5}},
        {"tool": "Bash", "input": {"command": "ls -la"}},
        {"tool": "Bash", "input": {"command": "cat /etc/hosts | head -2"}},
        {"tool": "Bash", "input": {"command": "cutline render project.json5 > render.log"}},
        {"tool": "Bash", "input": {"command": format!("cutline render project.json5 --out {}/stolen.mp4", w.root)}},
        {"tool": "Bash", "input": {"command": "cutline deps install --all"}},
        {"tool": "Bash", "input": {"command": "video-studio secret status"}},
        {"tool": "Bash", "input": {"command": format!("{VIDEO_STUDIO} runs {}", w.project)}},
        {"tool": "Write", "input": {"file_path": format!("{}/escape.txt", w.root), "content": "x"}},
        {"tool": "Write", "input": {"file_path": "../escape2.txt", "content": "x"}},
        {"tool": "Write", "input": {"file_path": "logs/events.ndjson", "content": "forged"}},
        {"tool": "Edit", "input": {"file_path": w.root.join("footage/hero.mov").to_string(), "old_string": "fake", "new_string": "real"}},
        {"tool": "Read", "input": {"file_path": outside.to_string()}},
        {"tool": "Read", "input": {"file_path": w.root.join("footage/hero.mov").to_string()}},
        {"tool": "WebFetch", "input": {"url": "https://example.com", "prompt": "x"}},
        {"tool": "KillShell", "input": {}},
        {"tool": "Bash", "input": {"command": "cutline render project.json5"}},
        {"tool": "Write", "input": {"file_path": "summary.md", "content": "## What I made\nBoth.\n"}},
    ]));
    let o = w.run(&["run", w.project.as_str(), "--config", "Social pack", "--json", "--yes"]);
    assert_eq!(o.status.code(), Some(0), "the run still succeeds: denials are answers, not failures\n{}", stderr(&o));
    let calls = w.tool_calls();
    let decision = |needle: &str| {
        calls
            .iter()
            .find(|c| c.3.contains(needle) || c.0 == needle)
            .map(|c| c.1.as_str())
            .unwrap_or_else(|| panic!("no call matching {needle}: {calls:?}"))
    };
    for denied in [
        "ls -la",
        "cat /etc/hosts",
        "> render.log",
        "stolen.mp4",
        "deps install",
        "secret status",
        " runs ",
        "escape.txt",
        "escape2.txt",
        "logs/events.ndjson",
        "private.txt",
        "WebFetch",
        "KillShell",
    ] {
        assert_eq!(decision(denied), "deny", "{denied} should be denied");
    }
    assert_eq!(calls.iter().find(|c| c.0 == "Edit").unwrap().1, "deny", "media is read-only");
    assert_eq!(
        calls.iter().find(|c| c.0 == "Read" && c.3.ends_with("hero.mov")).unwrap().1,
        "allow",
        "media in the manifest can be read"
    );
    assert_eq!(calls.iter().filter(|c| c.1 == "allow").count(), 4, "{calls:?}");
    assert!(
        !w.root.join("escape.txt").exists()
            && !w.root.join("escape2.txt").exists()
            && !w.root.join("stolen.mp4").exists()
    );
    assert_eq!(std::fs::read_to_string(w.root.join("footage/hero.mov")).unwrap(), "fake video bytes for the hero shot");
    let run = w.run_folder();
    assert!(!run.join("render.log").exists());
    assert_ne!(std::fs::read_to_string(run.join("logs/events.ndjson")).unwrap(), "forged");
    // Denied requests are logged with reasons.
    let perms = ndjson(&run.join("logs/permissions.ndjson"));
    let denied: Vec<&Value> = perms.iter().filter(|p| p["decision"] == "deny").collect();
    assert_eq!(denied.len(), 14);
    assert!(denied.iter().all(|d| d["reason"].as_str().map(|r| !r.is_empty()).unwrap_or(false)));
    assert!(denied.iter().any(|d| d["reason"].as_str().unwrap().contains("Only cutline")));
    let report = stdout(&w.run(&["logs", run.as_str(), "--issue-report"]));
    assert!(report.contains("## Blocked tool calls") && report.contains("`ls -la`"));
    assert!(std::fs::read_to_string(run.join("logs/summary.md")).unwrap().contains("Blocked tool calls: 14"));
}

#[test]
fn dry_run_cannot_render_finals_or_call_the_helper() {
    let mut w = World::new();
    let eleven = fake_eleven();
    w.eleven_url = Some(eleven.url.clone());
    w.set_script(json!([
        {"tool": "Write", "input": {"file_path": "project.json5", "content": PROJECT_JSON5}},
        {"tool": "Bash", "input": {"command": "cutline validate --json project.json5"}},
        {"tool": "Bash", "input": {"command": "cutline review project.json5"}},
        {"tool": "Bash", "input": {"command": "cutline render project.json5"}},
        {"tool": "Bash", "input": {"command": "cutline --json render --draft project.json5"}},
        {"tool": "Bash", "input": {"command": format!("{FAKE_CUTLINE} render project.json5")}},
        {"tool": "Bash", "input": {"command": "video-studio eleven tts --voice v1 --text 'Hello there' --out vo.mp3"}},
        {"tool": "Bash", "input": {"command": "video-studio eleven music --prompt calm --length 10 --out m.mp3"}},
        {"tool": "Bash", "input": {"command": "video-studio eleven quote tts --text 'Hello there'"}},
        {"tool": "Write", "input": {"file_path": "readiness.json", "content": json!({"ready": false, "ambiguities": [{"text": "Which logo opens the video?", "spec_line": 3}], "assumptions": ["Voice-over in English"], "missing_media": [], "risks": [{"text": "Scene 2 has no footage", "spec_line": 12}], "estimated_durations": {"youtube-shorts": 44.0}}).to_string()}},
    ]));
    let o = w.run(&["validate", w.project.as_str(), "--config", "Social pack", "--dry-run", "--json"]);
    assert_eq!(o.status.code(), Some(1), "findings are warnings\nstdout:\n{}\nstderr:\n{}", stdout(&o), stderr(&o));
    let calls = w.tool_calls();
    let by = |needle: &str| calls.iter().filter(|c| c.3.contains(needle)).map(|c| c.1.as_str()).collect::<Vec<_>>();
    assert_eq!(by("render"), vec!["deny", "deny", "deny"], "no form of render gets through: {calls:?}");
    assert_eq!(by("eleven tts"), vec!["deny"]);
    assert_eq!(by("eleven music"), vec!["deny"]);
    assert_eq!(by("eleven quote"), vec!["allow"]);
    assert_eq!(by("validate"), vec!["allow"]);
    assert_eq!(by("review"), vec!["allow"]);
    assert_eq!(eleven.count("/v1/text-to-speech"), 0, "the API was never called");
    // The dry run works in a scratch folder in the library, not in the output folder.
    assert!(!w.out.exists());
    let p = w.load_project();
    assert_eq!(p.runs.len(), 1);
    assert_eq!((p.runs[0].kind, p.runs[0].status), (RunKind::DryRun, RunStatus::SucceededWithWarnings));
    let folder = Utf8Path::new(&p.runs[0].folder);
    assert!(folder.starts_with(w.lib.join("dry-runs")), "{folder}");
    assert!(
        !folder.join("outputs/youtube-shorts/youtube-shorts.mp4").exists() && !folder.join("audio/vo.mp3").exists()
    );
    assert_eq!(p.runs[0].model.as_deref(), Some("haiku"), "the dry run uses the cheaper model");
    // Findings join the validation report as warnings linked to spec lines.
    let report: Value = serde_json::from_str(
        stdout(&o).lines().filter(|l| !l.contains("\"event\"")).collect::<Vec<_>>().join("\n").trim(),
    )
    .unwrap();
    let dry: Vec<&Value> = report["issues"].as_array().unwrap().iter().filter(|i| i["category"] == "dry_run").collect();
    assert_eq!(dry.len(), 3);
    assert!(dry.iter().all(|i| i["severity"] == "warning"));
    assert_eq!(dry.iter().find(|i| i["rule"] == "dry_run_ambiguity").unwrap()["fix"]["line"], 3);
    assert_eq!(report["dry_run"]["estimated_durations"]["youtube-shorts"], 44.0);
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("logs/validation.json")).unwrap()).unwrap();
    assert_eq!(saved["dry_run"]["ready"], false, "validation.json holds the dry run findings");
    // The proxy refuses too, had the broker let a render through.
    let trace = w.trace();
    assert!(trace.iter().any(|t| t["reason"].as_str().unwrap_or("").contains("dry run")));
}

#[test]
fn run_does_the_dry_run_first_and_stops_on_findings_without_yes() {
    let w = World::new();
    w.edit_project(|p| p.run_configs[0].dry_run_first = true);
    w.set_script(json!([
        {"tool": "Write", "input": {"file_path": "project.json5", "content": PROJECT_JSON5}},
        {"tool": "Bash", "input": {"command": "cutline validate --json project.json5"}},
        {"tool": "Write", "input": {"file_path": "readiness.json", "content": json!({"ready": false, "risks": ["The hook is longer than the scene"]}).to_string()}},
    ]));
    // Without --yes and without a terminal, findings stop the run before production.
    let o = w.run(&["run", w.project.as_str(), "--config", "Social pack"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("The dry run found 1 point(s)")
            && stderr(&o).contains("The hook is longer than the scene")
            && stderr(&o).contains("--yes")
    );
    assert!(!w.out.exists(), "production never started");
    assert_eq!(w.load_project().runs.len(), 1);
    // --no-dry-run skips it; the same script then fails as a production run (no outputs).
    let o = w.run(&["run", w.project.as_str(), "--config", "Social pack", "--no-dry-run"]);
    assert_eq!(o.status.code(), Some(3), "{}", stderr(&o));
    assert!(stderr(&o).contains("without producing these outputs: youtube-shorts, linkedin"), "{}", stderr(&o));
    let p = w.load_project();
    assert_eq!(p.runs.len(), 2);
    assert_eq!((p.runs[1].kind, p.runs[1].status), (RunKind::Production, RunStatus::Failed));
}

#[test]
fn blockers_stop_a_run_before_anything_is_launched() {
    let w = World::new();
    std::fs::remove_file(w.root.join("footage/hero.mov")).unwrap();
    w.set_script(happy_script());
    let o = w.run(&["run", w.project.as_str(), "--config", "Social pack", "--yes"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("BLOCKER  hero.mov is missing")
            && stderr(&o).contains("can't run until the blockers are fixed"),
        "{}",
        stderr(&o)
    );
    assert!(!w.trace.exists(), "Claude was never started");
    assert!(!w.out.exists() && w.load_project().runs.is_empty());
    // validate reports the same thing, as text and as JSON
    let v = w.run(&["validate", w.project.as_str(), "--config", "Social pack", "--json"]);
    assert_eq!(v.status.code(), Some(2));
    let report: Value = serde_json::from_str(&stdout(&v)).unwrap();
    assert_eq!(report["blockers"], 1);
    assert_eq!(report["issues"][0]["rule"], "media_missing");
    assert_eq!(report["issues"][0]["fix"]["target"], "m-hero");
    // an unknown run config is a clear error
    let bad = w.run(&["run", w.project.as_str(), "--config", "Nope"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(stderr(&bad).contains("no run config \"Nope\"") && stderr(&bad).contains("Social pack"));
}

#[test]
fn missing_claude_or_cutline_is_an_environment_error() {
    let w = World::new();
    w.set_script(happy_script());
    let o = w
        .command(&["run", w.project.as_str(), "--config", "Social pack", "--yes"])
        .env("VIDEO_STUDIO_NO_CLAUDE", "1")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(5), "{}", stderr(&o));
    assert!(stderr(&o).contains("Claude Code isn't installed"));
    let o = w
        .command(&["validate", w.project.as_str(), "--config", "Social pack"])
        .env("VIDEO_STUDIO_CUTLINE", "/nonexistent/cutline")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(5), "{}", stdout(&o));
    assert!(stdout(&o).contains("cutline wasn't found"));
    let o = w
        .command(&["validate", w.project.as_str(), "--config", "Social pack"])
        .env("FAKE_CLAUDE_VERSION", "1.0.3 (Claude Code)")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(5));
    assert!(
        stdout(&o).contains("Claude Code 1.0.3 is too old") && stdout(&o).contains("2.1.0 or later"),
        "{}",
        stdout(&o)
    );
    let o = w
        .command(&["validate", w.project.as_str(), "--config", "Social pack"])
        .env("FAKE_CLAUDE_SIGNED_OUT", "1")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(stdout(&o).contains("Claude Code isn't signed in"));
}

#[test]
fn questions_in_a_headless_run_proceed_on_the_assumption_and_are_logged() {
    let w = World::new();
    let mut steps = vec![
        json!({"tool": "AskUserQuestion", "input": {"questions": [{"question": "Which logo opens the video?", "header": "Logo", "multiSelect": false, "options": [{"label": "Horizontal", "description": "Recommended"}, {"label": "Icon"}]}]}}),
    ];
    steps.extend(happy_script().as_array().unwrap().iter().skip(2).cloned());
    w.set_script(Value::Array(steps));
    let o = w.run(&["run", w.project.as_str(), "--config", "Social pack", "--json", "--yes"]);
    assert_eq!(o.status.code(), Some(1), "succeeded with warnings\n{}", stderr(&o));
    let ev = events(&o);
    let q = ev.iter().find(|e| e["event"] == "question").expect("a question event");
    assert_eq!(q["questions"][0]["question"], "Which logo opens the video?");
    assert_eq!(q["questions"][0]["options"][0]["label"], "Horizontal");
    let a = ev.iter().find(|e| e["event"] == "question_answered").unwrap();
    assert_eq!(a["by"], "assumed");
    assert!(ev
        .iter()
        .any(|e| e["event"] == "warning" && e["message"].as_str().unwrap().contains("Which logo opens the video?")));
    assert_eq!(ev.last().unwrap()["status"], "succeeded_with_warnings");
    let ask = w.trace().into_iter().find(|t| t["tool"] == "AskUserQuestion").unwrap();
    assert_eq!(
        (ask["decision"].as_str(), ask["via"].as_str()),
        (Some("deny"), Some("stdio")),
        "questions go to the host, not the hook"
    );
    assert!(ask["reason"].as_str().unwrap().contains("Proceed on your stated assumption"));
    let run = w.run_folder();
    assert!(
        std::fs::read_to_string(run.join("logs/events.ndjson")).unwrap().contains("Which logo opens the video?"),
        "the question is logged"
    );
}

#[test]
fn a_crash_is_a_failed_run_with_logs_up_to_the_crash() {
    let w = World::new();
    w.set_script(json!([
        {"tool": "Write", "input": {"file_path": "project.json5", "content": PROJECT_JSON5}},
        {"tool": "Bash", "input": {"command": "cutline validate --json project.json5"}},
        {"crash": 7}
    ]));
    let o = w.run(&["run", w.project.as_str(), "--config", "Social pack", "--yes"]);
    assert_eq!(o.status.code(), Some(3), "{}", stderr(&o));
    assert!(stderr(&o).contains("Failed") && stderr(&o).contains("simulated crash"), "{}", stderr(&o));
    let run = w.run_folder();
    let run_json: Value = serde_json::from_str(&std::fs::read_to_string(run.join("logs/run.json")).unwrap()).unwrap();
    assert_eq!((run_json["status"].as_str(), run_json["exit"].as_i64()), (Some("failed"), Some(7)));
    assert!(
        run.join("logs/cutline/001-validate.log").is_file()
            && run.join("logs/claude.jsonl").is_file()
            && run.join("logs/summary.md").is_file()
    );
    assert_eq!(w.load_project().runs[0].status, RunStatus::Failed);
}

#[test]
fn failing_cutline_commands_and_qa_reach_the_issue_report() {
    let w = World::new();
    let bad = PROJECT_JSON5.replace("name: \"launch\"", "name: \"launch FAIL_RENDER\"");
    let warn = PROJECT_JSON5.replace("name: \"launch\"", "name: \"launch QA_WARN\"");
    w.set_script(json!([
        {"tool": "Write", "input": {"file_path": "project.json5", "content": bad}},
        {"tool": "Bash", "input": {"command": "cutline render project.json5"}},
        {"tool": "Write", "input": {"file_path": "project.json5", "content": warn}},
        {"tool": "Bash", "input": {"command": "cutline render project.json5"}},
        {"tool": "Write", "input": {"file_path": "summary.md", "content": "## QA results\nLoudness is low.\n"}},
    ]));
    let o = w.run(&["run", w.project.as_str(), "--config", "Social pack", "--json", "--yes"]);
    assert_eq!(o.status.code(), Some(1), "QA warnings make it succeed with warnings\n{}", stderr(&o));
    let ev = events(&o);
    assert!(
        ev.iter().any(|e| e["event"] == "stage_finished" && e["stage"] == "render" && e["ok"] == true),
        "the retry cleared the failure"
    );
    assert!(ev
        .iter()
        .any(|e| e["event"] == "warning" && e["message"].as_str().unwrap().contains("QA passed with 1 warning")));
    let run = w.run_folder();
    let report = stdout(&w.run(&["logs", run.as_str(), "--issue-report"]));
    for needle in [
        "## Failed cutline commands",
        "### `cutline render project.json5` — exit 1",
        "E_FAKE_RENDER",
        "## cutline warnings",
        "W_FAKE_SLOW",
        "## QA findings",
        "**loudness** (warn)",
        "Loudness is low.",
    ] {
        assert!(report.contains(needle), "missing {needle:?} in\n{report}");
    }
    let results: Value = serde_json::from_str(&std::fs::read_to_string(run.join("logs/run.json")).unwrap()).unwrap();
    assert_eq!(results["results"][0]["qa_verdict"], "warnings");
}

/// Starts a run as a child process and returns it once the given event has been seen on stdout.
fn spawn_until(
    w: &World,
    extra_env: &[(&str, &str)],
    wait_for: &str,
) -> (std::process::Child, Vec<Value>, BufReader<std::process::ChildStdout>) {
    let mut cmd = w.command(&["run", w.project.as_str(), "--config", "Social pack", "--json", "--yes"]);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || Instant::now() > deadline {
            panic!("the run ended before {wait_for}: {seen:?}");
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            let hit = v.to_string().contains(wait_for);
            seen.push(v);
            if hit {
                break;
            }
        }
    }
    (child, seen, reader)
}

fn alive(pid: u64) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn interrupt(child: &std::process::Child) {
    std::process::Command::new("kill").args(["-INT", &child.id().to_string()]).status().unwrap();
}

#[cfg(unix)]
#[test]
fn ctrl_c_stops_gracefully_kills_the_tree_and_leaves_complete_logs() {
    let w = World::new();
    w.set_script(json!([
        {"tool": "Write", "input": {"file_path": "project.json5", "content": PROJECT_JSON5}},
        {"tool": "Bash", "input": {"command": "cutline render project.json5"}},
        {"spawn": "sleep 300"},
        {"say": "Waiting on a long render."},
        {"hang": true}
    ]));
    let (mut child, mut seen, mut reader) = spawn_until(&w, &[], "Waiting on a long render.");
    let spawned = w.trace().iter().find_map(|t| t["spawned"].as_u64()).expect("the fake started a child process");
    assert!(alive(spawned));
    let stopped_at = Instant::now();
    interrupt(&child);
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut reader, &mut rest).unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(4), "cancelled");
    assert!(stopped_at.elapsed() < Duration::from_secs(4), "a cooperative Claude stops well inside the grace period");
    seen.extend(rest.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()));
    let fin = seen.last().unwrap();
    assert_eq!(
        (fin["event"].as_str(), fin["status"].as_str(), fin["exit_code"].as_i64()),
        (Some("run_finished"), Some("cancelled"), Some(4))
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(!alive(spawned), "the process tree is gone");
    // Logs are complete up to the cancel point, and partial outputs are kept and flagged.
    let run = w.run_folder();
    let run_json: Value = serde_json::from_str(&std::fs::read_to_string(run.join("logs/run.json")).unwrap()).unwrap();
    assert_eq!(run_json["status"], "cancelled");
    assert!(run_json["finished"].is_string());
    let results = run_json["results"].as_array().unwrap();
    assert!(results.iter().all(|r| r["exists"] == true && r["incomplete"] == true), "{results:?}");
    assert!(run.join("outputs/linkedin/linkedin.mp4").is_file(), "partial outputs stay in place");
    let claude_log = std::fs::read_to_string(run.join("logs/claude.jsonl")).unwrap();
    assert!(
        claude_log.contains("\"host_stop\"")
            && claude_log.contains("\"subtype\":\"interrupt\"")
            && claude_log.contains("\"host_exit\"")
    );
    let ev = ndjson(&run.join("logs/events.ndjson"));
    assert_eq!(ev.last().unwrap()["status"], "cancelled");
    assert!(run.join("logs/cutline/001-render.log").is_file() && run.join("logs/summary.md").is_file());
    let p = w.load_project();
    assert_eq!(p.runs[0].status, RunStatus::Cancelled);
    assert!(
        !w.lib.join("run-contexts").join(&p.runs[0].id).exists(),
        "the helper context is removed when the run ends"
    );
}

#[cfg(unix)]
#[test]
fn a_claude_that_ignores_the_interrupt_is_killed_after_the_grace_period() {
    let w = World::new();
    w.set_script(json!([{"spawn": "sleep 300"}, {"say": "Busy."}, {"hang": true}]));
    let (mut child, _seen, mut reader) = spawn_until(&w, &[("FAKE_CLAUDE_IGNORE_INTERRUPT", "1")], "Busy.");
    let spawned = w.trace().iter().find_map(|t| t["spawned"].as_u64()).unwrap();
    let at = Instant::now();
    interrupt(&child);
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut reader, &mut rest).unwrap();
    let status = child.wait().unwrap();
    let took = at.elapsed();
    assert_eq!(status.code(), Some(4));
    assert!(
        took >= Duration::from_millis(4500) && took < Duration::from_secs(12),
        "killed after the 5 s grace period, took {took:?}"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(!alive(spawned), "the whole tree was killed");
    let run = w.run_folder();
    let run_json: Value = serde_json::from_str(&std::fs::read_to_string(run.join("logs/run.json")).unwrap()).unwrap();
    assert_eq!(run_json["status"], "cancelled");
}

#[cfg(unix)]
#[test]
fn a_second_ctrl_c_kills_immediately() {
    let w = World::new();
    w.set_script(json!([{"say": "Busy."}, {"hang": true}]));
    let (mut child, _seen, mut reader) = spawn_until(&w, &[("FAKE_CLAUDE_IGNORE_INTERRUPT", "1")], "Busy.");
    let at = Instant::now();
    interrupt(&child);
    std::thread::sleep(Duration::from_millis(400));
    interrupt(&child);
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut reader, &mut rest).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(4));
    assert!(at.elapsed() < Duration::from_millis(3500), "no waiting for the grace period: {:?}", at.elapsed());
}
