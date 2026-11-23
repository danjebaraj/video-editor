//! FEAT-spry-alpaca-5RQZ: production runs driven by Codex through the `video-studio` CLI, with
//! the fake app server (`vs-fake-codex`) and the fake cutline, on a machine without Claude Code:
//! the run folder, manifest and instructions reach Codex, tool progress reaches the event stream,
//! the output checks decide success, and the broker's limits hold.

mod common;

use common::*;
use serde_json::{json, Value};
use video_studio_core::settings::Settings;

const FAKE_CODEX: &str = env!("CARGO_BIN_EXE_vs-fake-codex");

fn codex_world(turn: Value) -> World {
    let mut w = World::new();
    w.eleven_url = Some(fake_eleven().url);
    let mut s = Settings::load(&w.lib);
    s.provider = video_studio_core::codex::Provider::Codex;
    s.codex_path = FAKE_CODEX.to_string();
    s.save(&w.lib).unwrap();
    std::fs::write(&w.script, json!([turn]).to_string()).unwrap();
    w
}

/// Runs with Codex and no Claude Code installed.
fn run_codex(w: &World) -> std::process::Output {
    w.command(&["run", w.project.as_str(), "--config", "Social pack", "--json", "--yes"])
        .env("VIDEO_STUDIO_NO_CLAUDE", "1")
        .env("FAKE_CODEX_SCRIPT", w.script.as_str())
        .env("FAKE_CODEX_LOG", w.root.join("codex.ndjson").as_str())
        .output()
        .unwrap()
}

#[test]
fn a_codex_run_produces_the_outputs_with_tool_progress() {
    let w = codex_world(json!([
        {"say": "Reading the spec and the media manifest."},
        {"write": {"path": "project.json5", "content": PROJECT_JSON5}},
        {"command": "cutline validate --json project.json5"},
        {"command": "video-studio eleven tts --voice v1 --text Hello --out vo.mp3"},
        {"command": "cutline render project.json5"},
        {"write": {"path": "summary.md", "content": "## What I made\nTwo outputs.\n"}},
        {"usage": [5000, 400]},
        {"say": "Both outputs are rendered."}
    ]));
    let o = run_codex(&w);
    let ev = events(&o);
    let fin = ev.last().unwrap_or_else(|| panic!("no events\nstdout:\n{}\nstderr:\n{}", stdout(&o), stderr(&o)));
    assert_eq!(fin["event"], "run_finished");
    assert!(matches!(fin["status"].as_str(), Some("succeeded" | "succeeded_with_warnings")), "{fin}\n{}", stderr(&o));
    // the ElevenLabs helper ran with the run's context and its credit accounting
    assert_eq!(fin["eleven_credits"], 5, "{fin}");
    // outputs, found by the existing output checks
    let outputs: Vec<&str> = ev
        .iter()
        .filter(|e| e["event"] == "artifact" && e["kind"] == "output")
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert_eq!(outputs.len(), 2, "{ev:#?}");
    // tool progress in the inspector: Codex's commands, cutline's own progress through the proxy
    let tools: Vec<&str> = ev
        .iter()
        .filter(|e| e["event"] == "message" && e["role"] == "tool")
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert!(tools.contains(&"Ran `cutline render project.json5`"), "{tools:?}");
    assert!(tools.contains(&"Wrote `project.json5`"), "{tools:?}");
    assert!(
        ev.iter().any(|e| e["event"] == "progress"
            && e["detail"].as_str().unwrap_or("").contains("cutline render finished")),
        "{ev:#?}"
    );
    let stages: Vec<&str> =
        ev.iter().filter(|e| e["event"] == "stage_started").map(|e| e["stage"].as_str().unwrap()).collect();
    assert!(stages.contains(&"render") && stages.contains(&"validate"), "{stages:?}");
    assert!(ev
        .iter()
        .any(|e| e["event"] == "message" && e["role"] == "assistant" && e["text"] == "Both outputs are rendered."));

    // Codex got the prepared run: its folder as the working directory and only writable root,
    // the production instructions with the media manifest
    let log = ndjson(&w.root.join("codex.ndjson"));
    let run = w.run_folder();
    let start = log.iter().find(|m| m["method"] == "thread/start").expect("thread/start");
    assert_eq!(start["params"]["cwd"], run.as_str());
    let instructions = start["params"]["developerInstructions"].as_str().unwrap();
    assert!(instructions.contains("media-manifest.json") && instructions.contains("Working in Codex"));
    assert_eq!(
        start["params"]["config"]["shell_environment_policy"]["set"]["PATH"]
            .as_str()
            .map(|p| p.starts_with(run.join(".bin").as_str())),
        Some(true)
    );
    let turn = log.iter().find(|m| m["method"] == "turn/start").unwrap();
    assert_eq!(turn["params"]["sandboxPolicy"]["writableRoots"], json!([run.as_str()]));
    assert_eq!(turn["params"]["sandboxPolicy"]["networkAccess"], false);
    // the run's logs record the provider and the protocol
    for f in ["logs/codex.jsonl", "logs/summary.md", "logs/run.json", "outputs/linkedin/linkedin.mp4"] {
        assert!(run.join(f).is_file(), "missing {f}");
    }
    let run_json: Value = serde_json::from_str(&std::fs::read_to_string(run.join("logs/run.json")).unwrap()).unwrap();
    assert_eq!(run_json["provider"], "codex");
    assert!(run_json["claude_session"].as_str().unwrap().starts_with("codex:thr-"));
    // the known read gap is traced on start, without counting as a run warning
    let trace = std::fs::read_to_string(run.join("logs/codex.jsonl")).unwrap();
    assert!(
        trace.lines().take(3).any(|l| l.contains("\"host_warning\"") && l.contains("codex_reads_unchecked")),
        "{trace}"
    );
    let events = std::fs::read_to_string(run.join("logs/events.ndjson")).unwrap();
    assert!(events.contains("considers safe and read-only without asking"), "{events}");
    let record = w.load_project().runs.pop().unwrap();
    assert!(record.claude_session.unwrap().starts_with("codex:"));
    assert_eq!(record.provider, Some(video_studio_core::codex::Provider::Codex), "history shows the provider used");
}

#[test]
fn a_codex_run_without_its_outputs_fails_and_refusals_hold() {
    let w = codex_world(json!([
        {"write": {"path": "project.json5", "content": PROJECT_JSON5}},
        {"command": "cutline validate --json project.json5"},
        {"command": "rm -rf ../footage"},
        {"write": {"path": "../outside.txt", "content": "escaped"}},
        {"write": {"path": "logs/run.json", "content": "{}"}},
        {"say": "I couldn't render."}
    ]));
    let o = run_codex(&w);
    let ev = events(&o);
    let fin = ev.last().unwrap();
    assert_eq!(fin["status"], "failed", "{fin}\n{}", stderr(&o));
    assert!(fin["message"].as_str().unwrap().contains("without producing"), "{fin}");
    assert_ne!(o.status.code(), Some(0));
    // nothing escaped the run folder
    assert!(w.root.join("footage/hero.mov").is_file(), "the refused command never ran");
    assert!(!w.root.join("out/outside.txt").exists());
    let run = w.run_folder();
    let run_json: Value = serde_json::from_str(&std::fs::read_to_string(run.join("logs/run.json")).unwrap()).unwrap();
    assert_eq!(run_json["status"], "failed", "the app's own record wasn't overwritten");
    let notes: Vec<&str> = ev
        .iter()
        .filter(|e| e["event"] == "message" && e["role"] == "note")
        .filter_map(|e| e["text"].as_str())
        .filter(|t| t.starts_with("Blocked"))
        .collect();
    assert_eq!(notes.len(), 3, "{notes:?}");
    // each refused request got a terminal answer
    let log = ndjson(&w.root.join("codex.ndjson"));
    let declined = log.iter().filter(|m| m.get("method").is_none() && m["result"]["decision"] == "decline").count();
    assert_eq!(declined, 3);
    assert_eq!(run_json["blocked_tool_calls"], 3);
}

/// BUG-fond-crane-62ZP: a Codex run never gets Claude's model. With Codex default it sends no
/// model, so Codex picks; with a Codex model on the run config, that exact ID reaches Codex.
#[test]
fn a_codex_run_uses_codex_models_not_claudes() {
    let turn = json!([
        {"write": {"path": "project.json5", "content": PROJECT_JSON5}},
        {"command": "cutline render project.json5"},
        {"write": {"path": "summary.md", "content": "## What I made\nTwo outputs.\n"}}
    ]);
    let w = codex_world(turn.clone());
    // the run config keeps Claude's Opus, as a project made before Codex did
    w.edit_project(|p| p.run_configs[0].production_model = "opus".into());
    let o = run_codex(&w);
    let fin = events(&o).last().cloned().unwrap_or_default();
    assert!(matches!(fin["status"].as_str(), Some("succeeded" | "succeeded_with_warnings")), "{fin}\n{}", stderr(&o));
    let sent = ndjson(&w.root.join("codex.ndjson"));
    for m in ["thread/start", "turn/start"] {
        let msg = sent.iter().find(|x| x["method"] == m).unwrap();
        assert!(msg["params"].get("model").is_none(), "Codex default: no model in {m}: {msg}");
    }
    let record = w.load_project().runs.pop().unwrap();
    assert_ne!(record.model.as_deref(), Some("opus"));

    // a Codex model chosen on the run config reaches Codex as is; Claude's choice is kept
    let w = codex_world(turn);
    w.edit_project(|p| {
        p.run_configs[0].production_model = "opus".into();
        p.run_configs[0].codex_production_model = "gpt-5.5-codex".into();
    });
    let o = run_codex(&w);
    let fin = events(&o).last().cloned().unwrap_or_default();
    assert!(matches!(fin["status"].as_str(), Some("succeeded" | "succeeded_with_warnings")), "{fin}\n{}", stderr(&o));
    let sent = ndjson(&w.root.join("codex.ndjson"));
    for m in ["thread/start", "turn/start"] {
        let msg = sent.iter().find(|x| x["method"] == m).unwrap();
        assert_eq!(msg["params"]["model"], "gpt-5.5-codex", "{m}");
    }
    assert_eq!(w.load_project().run_configs[0].production_model, "opus", "Claude's choice stays for Claude");
}

/// Runs an Audio Review with Codex and no Claude Code installed.
fn run_codex_audio_review(w: &World) -> std::process::Output {
    w.command(&["run", w.project.as_str(), "--config", "Social pack", "--audio-review", "--json", "--yes"])
        .env("VIDEO_STUDIO_NO_CLAUDE", "1")
        .env("FAKE_CODEX_SCRIPT", w.script.as_str())
        .env("FAKE_CODEX_LOG", w.root.join("codex.ndjson").as_str())
        .env("VIDEO_STUDIO_CODEX_PATCH_TIMEOUT_MS", "1500")
        .output()
        .unwrap()
}

const SPEC_MARKER: &str = "MARKER-rustic-olive-7Q4T";

/// BUG-rustic-olive-U4PY: a Codex Audio Review works from the inputs it was given, not from
/// files it would have to open with tools it doesn't have. The fake server has no read tool and
/// no Node REPL: it only goes on when the spec's marker, a media path and the export slug are in
/// its input. Then it validates, mixes the selected export and the WAV is the result. Usage that
/// includes cached tokens, reported twice with the same totals, is counted once and the same
/// live and final; the inspector, summary and record all name Codex and Audio Review.
#[test]
fn a_codex_audio_review_gets_its_inputs_and_mixes_the_soundtrack() {
    let w = codex_world(json!([
        {"require": SPEC_MARKER},
        {"require": "youtube-shorts"},
        {"require": "footage/hero.mov"},
        {"command": "cutline schema"},
        {"write": {"path": "project.json5", "content": PROJECT_JSON5}},
        {"command": "cutline validate project.json5"},
        {"command": "cutline audio project.json5 --export youtube-shorts --out outputs/audio-review.wav"},
        {"write": {"path": "summary.md", "content": "## What you will hear\nThe tone, once.\n"}},
        {"usage": [982031, 1503, 927104]},
        {"usage": [982031, 1503, 927104]},
        {"say": "The soundtrack is mixed."}
    ]));
    w.edit_project(|p| {
        p.spec.push_str(&format!(
            "\n\nUse the attached clip once at unity gain. No generated speech or music. {SPEC_MARKER}\n"
        ));
    });
    let o = run_codex_audio_review(&w);
    let ev = events(&o);
    let fin = ev.last().unwrap_or_else(|| panic!("no events\nstdout:\n{}\nstderr:\n{}", stdout(&o), stderr(&o)));
    assert_eq!(fin["event"], "run_finished");
    assert!(matches!(fin["status"].as_str(), Some("succeeded" | "succeeded_with_warnings")), "{fin}\n{}", stderr(&o));
    assert_eq!(fin["eleven_credits"], 0, "no paid service");

    let r = w.load_project().runs.pop().unwrap();
    assert_eq!(r.kind, video_studio_core::project::RunKind::AudioReview);
    assert_eq!(r.provider, Some(video_studio_core::codex::Provider::Codex));
    let folder = camino::Utf8PathBuf::from(&r.folder);
    assert!(folder.join("outputs/audio-review.wav").is_file());
    let results = video_studio_core::run::collect::load_results(&folder);
    assert!(results.outputs[0].exists && !results.outputs[0].incomplete, "{:?}", results.outputs);

    // the instructions describe what Codex actually has, and the inputs are in its first message
    let system = std::fs::read_to_string(folder.join("logs/prompt-system.md")).unwrap();
    assert!(!system.contains("Use the Read, Glob and Grep tools"), "Claude's tools aren't Codex's");
    assert!(system.contains("no Read, Glob or Grep tool"));
    let prompt = std::fs::read_to_string(folder.join("logs/prompt.md")).unwrap();
    assert!(prompt.contains("## Prepared inputs") && prompt.contains(SPEC_MARKER), "{prompt}");
    let run_json: Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("logs/run.json")).unwrap()).unwrap();
    assert_eq!(run_json["codex"]["inputs"]["read_tools"], false, "{run_json}");

    // a schema lookup moves nothing forward; validation and the mix are their own stages
    let stages: Vec<&str> =
        ev.iter().filter(|e| e["event"] == "stage_started").map(|e| e["stage"].as_str().unwrap()).collect();
    assert!(stages.contains(&"validate") && stages.contains(&"mix"), "{stages:?}");
    assert!(
        !ev.iter().any(|e| e["event"] == "progress"
            && e["fraction"] == 1.0
            && e["detail"].as_str().unwrap_or("").contains("cutline schema")),
        "a lookup completes nothing"
    );

    // usage: live and final agree, cached tokens counted apart, the repeat adds nothing
    let usage = &run_json["usage"];
    assert_eq!(
        (usage["input_tokens"].as_u64(), usage["cache_read_tokens"].as_u64(), usage["output_tokens"].as_u64()),
        (Some(54_927), Some(927_104), Some(1_503)),
        "{usage}"
    );
    let last_cost = ev.iter().rfind(|e| e["event"] == "cost").expect("cost");
    assert_eq!((last_cost["input_tokens"].as_u64(), last_cost["output_tokens"].as_u64()), (Some(982_031), Some(1_503)));

    // the right provider and run kind everywhere
    let summary = std::fs::read_to_string(folder.join("logs/summary.md")).unwrap();
    assert!(summary.contains("- Kind: Audio Review") && summary.contains("- Assistant: Codex"), "{summary}");
    assert!(!summary.contains("Claude"), "{summary}");
}

/// BUG-rustic-olive-U4PY: a tool Codex calls that the run doesn't provide (an MCP server's
/// Node REPL) ends the run at once with that cause, instead of waiting for Codex's own timeout.
/// The failure names Codex, the tool and the missing WAV, and the next run starts right away.
#[test]
fn a_codex_audio_review_that_calls_a_missing_tool_fails_at_once_with_the_cause() {
    let w = codex_world(json!([]));
    std::fs::write(
        &w.script,
        json!([
            [{"say": "Reading the inputs."}, {"mcp": {"server": "node_repl", "tool": "js"}}],
            [
                {"write": {"path": "project.json5", "content": PROJECT_JSON5}},
                {"command": "cutline validate project.json5"},
                {"command": "cutline audio project.json5 --export youtube-shorts --out outputs/audio-review.wav"},
                {"write": {"path": "summary.md", "content": "## What you will hear\nThe tone.\n"}}
            ]
        ])
        .to_string(),
    )
    .unwrap();
    let started = std::time::Instant::now();
    let o = run_codex_audio_review(&w);
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "no waiting on the dead tool");
    let ev = events(&o);
    let fin = ev.last().unwrap_or_else(|| panic!("no events\n{}", stderr(&o)));
    assert_eq!(fin["status"], "failed", "{fin}");
    let msg = fin["message"].as_str().unwrap();
    assert!(msg.contains("Codex") && msg.contains("node_repl/js"), "{msg}");
    assert!(msg.contains("outputs/audio-review.wav"), "names the missing WAV too: {msg}");
    assert!(!msg.contains("Claude"), "{msg}");
    let r = w.load_project().runs.pop().unwrap();
    let folder = camino::Utf8PathBuf::from(&r.folder);
    assert!(!folder.join("outputs/audio-review.wav").exists());
    let summary = std::fs::read_to_string(folder.join("logs/summary.md")).unwrap();
    assert!(summary.contains("Audio Review") && summary.contains("node_repl/js"), "{summary}");
    // the last stage is not reported as a success
    assert!(!ev.iter().any(|e| e["event"] == "stage_finished" && e["ok"] == true && e["stage"] == "done"));

    // the next run starts immediately and succeeds
    let o = run_codex_audio_review(&w);
    let fin = events(&o).pop().unwrap_or_else(|| panic!("no events\n{}", stderr(&o)));
    assert!(matches!(fin["status"].as_str(), Some("succeeded" | "succeeded_with_warnings")), "{fin}");
}

/// Four narration scripts in one Codex patch (BUG-green-cricket-P6HQ).
fn four_scripts(complete: &str) -> Value {
    json!({"patch": {"id": 2, "complete": complete, "files": (1..=4)
        .map(|i| json!({"path": format!("audio/scene{i}.txt"), "content": format!("Narration for scene {i}.")}))
        .collect::<Vec<_>>()}})
}

fn tool_texts(ev: &[Value]) -> Vec<String> {
    ev.iter()
        .filter(|e| e["event"] == "message" && e["role"] == "tool")
        .filter_map(|e| e["text"].as_str().map(str::to_string))
        .collect()
}

/// Responses the client sent to one server request.
fn responses_to(w: &World, id: Value) -> Vec<Value> {
    ndjson(&w.root.join("codex.ndjson"))
        .into_iter()
        .filter(|m| m.get("method").is_none() && m.get("id") == Some(&id))
        .collect()
}

/// BUG-green-cricket-P6HQ: a four-file patch counts as written only when Codex reports it
/// applied; then the scripts feed the speech helper and the soundtrack is mixed.
#[test]
fn a_codex_patch_counts_as_written_once_applied_and_feeds_the_helper() {
    let mut steps = vec![four_scripts("ok")];
    for i in 1..=4 {
        steps.push(json!({"command": format!("video-studio eleven tts --voice v1 --text-file audio/scene{i}.txt --out audio/scene{i}.mp3")}));
    }
    steps.extend([
        json!({"write": {"path": "project.json5", "content": PROJECT_JSON5}}),
        json!({"command": "cutline validate project.json5"}),
        json!({"command": "cutline audio project.json5 --export youtube-shorts --out outputs/audio-review.wav"}),
        json!({"write": {"path": "summary.md", "content": "## What you will hear\nFour passages.\n"}}),
    ]);
    let w = codex_world(Value::Array(steps));
    let o = run_codex_audio_review(&w);
    let ev = events(&o);
    let fin = ev.last().unwrap_or_else(|| panic!("no events\n{}", stderr(&o)));
    assert!(matches!(fin["status"].as_str(), Some("succeeded" | "succeeded_with_warnings")), "{fin}\n{}", stderr(&o));
    assert_eq!(responses_to(&w, json!(2)).len(), 1, "one answer to the patch approval");
    let tools = tool_texts(&ev);
    for i in 1..=4 {
        assert!(tools.contains(&format!("Wrote `audio/scene{i}.txt`")), "{tools:?}");
    }
    // each write is reported after it was applied, not when it was approved
    let pending = ev.iter().position(|e| e["detail"] == "Writing `audio/scene1.txt`…").expect("pending status");
    let wrote = ev.iter().position(|e| e["text"] == "Wrote `audio/scene1.txt`").unwrap();
    assert!(pending < wrote);
    let r = w.load_project().runs.pop().unwrap();
    let folder = camino::Utf8PathBuf::from(&r.folder);
    let helper = std::fs::read_to_string(folder.join("logs/eleven.ndjson")).unwrap();
    for i in 1..=4 {
        assert!(helper.contains(&format!("Narration for scene {i}.")), "the helper got scene {i}: {helper}");
    }
    assert!(folder.join("outputs/audio-review.wav").is_file());
}

/// BUG-green-cricket-P6HQ: an accepted patch Codex never reports applied stops the run after
/// the deadline, naming the files. Nothing claims they were written, the refused reads and the
/// help call aren't a write or a mix, no credits are spent and the next run starts.
#[test]
fn a_codex_patch_never_reported_applied_stops_the_run_with_the_files_named() {
    let w = codex_world(json!([]));
    std::fs::write(
        &w.script,
        json!([
            [
                four_scripts("withhold"),
                {"command": "ls audio"},
                {"command": "cat audio/scene1.txt"},
                {"command": "cutline audio --help"},
                {"hang": true}
            ],
            [
                {"write": {"path": "project.json5", "content": PROJECT_JSON5}},
                {"command": "cutline audio project.json5 --export youtube-shorts --out outputs/audio-review.wav"},
                {"write": {"path": "summary.md", "content": "## What you will hear\nThe tone.\n"}}
            ]
        ])
        .to_string(),
    )
    .unwrap();
    let o = run_codex_audio_review(&w);
    let ev = events(&o);
    let fin = ev.last().unwrap_or_else(|| panic!("no events\n{}", stderr(&o)));
    assert_eq!(fin["status"], "failed", "{fin}");
    assert_eq!(fin["eleven_credits"], 0);
    let msg = fin["message"].as_str().unwrap();
    assert!(
        msg.contains("`audio/scene1.txt`") && msg.contains("`audio/scene4.txt`") && msg.contains("accepted"),
        "{msg}"
    );
    assert!(msg.contains("outputs/audio-review.wav"), "{msg}");
    assert_eq!(responses_to(&w, json!(2)).len(), 1);
    let tools = tool_texts(&ev);
    assert!(!tools.iter().any(|t| t.starts_with("Wrote")), "{tools:?}");
    let stages: Vec<&str> =
        ev.iter().filter(|e| e["event"] == "stage_started").filter_map(|e| e["stage"].as_str()).collect();
    assert!(!stages.contains(&"mix"), "help is no mix: {stages:?}");
    assert!(!ev.iter().any(|e| e["event"] == "progress" && e["fraction"] == 1.0), "nothing completed");
    let r = w.load_project().runs.pop().unwrap();
    let folder = camino::Utf8PathBuf::from(&r.folder);
    assert!(!folder.join("outputs/audio-review.wav").exists());
    let summary = std::fs::read_to_string(folder.join("logs/summary.md")).unwrap();
    assert!(!summary.contains("permissions.ndjson") || folder.join("logs/permissions.ndjson").is_file(), "{summary}");

    let o = run_codex_audio_review(&w);
    let fin = events(&o).pop().unwrap_or_else(|| panic!("no events\n{}", stderr(&o)));
    assert!(matches!(fin["status"].as_str(), Some("succeeded" | "succeeded_with_warnings")), "{fin}");
}

/// BUG-green-cricket-P6HQ: a patch Codex reports failed is a failure of those writes, not a
/// success; a batch with one path outside the run folder is declined whole, with one answer.
#[test]
fn a_failed_or_partly_outside_codex_patch_writes_nothing_and_says_so() {
    let w = codex_world(json!([
        four_scripts("fail"),
        {"patch": {"id": 9, "complete": "ok", "files": [
            {"path": "audio/inside.txt", "content": "in"},
            {"path": "../outside.txt", "content": "out"}
        ]}}
    ]));
    let o = run_codex_audio_review(&w);
    let ev = events(&o);
    let fin = ev.last().unwrap_or_else(|| panic!("no events\n{}", stderr(&o)));
    assert_eq!(fin["status"], "failed", "{fin}");
    let tools = tool_texts(&ev);
    assert!(!tools.iter().any(|t| t.starts_with("Wrote")), "{tools:?}");
    assert!(
        ev.iter().any(|e| e["event"] == "message"
            && e["failed"] == true
            && e["text"].as_str().unwrap_or("").contains("scene1.txt")),
        "the failed write is shown as failed: {tools:?}"
    );
    let answers = responses_to(&w, json!(9));
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["result"]["decision"], "decline");
    let r = w.load_project().runs.pop().unwrap();
    let folder = camino::Utf8PathBuf::from(&r.folder);
    assert!(!folder.join("audio/inside.txt").exists() && !folder.parent().unwrap().join("outside.txt").exists());
    assert!(fin["message"].as_str().unwrap().contains("scene1.txt"), "{fin}");
}

fn sandboxed_music() -> Value {
    json!({"command": "video-studio eleven music --prompt calm-bed --length 10 --out music.mp3", "sandboxed": true})
}

/// BUG-deft-lily-344X: Codex runs the helper in its sandbox, where the key store and the
/// network are out of reach (the fake runs it with an empty key store). The helper hands the
/// call to the run's host, which has the saved key: the local service gets the authenticated
/// request and the music lands in audio/, counted once, with nothing left in the relay folder.
#[test]
fn a_sandboxed_codex_helper_makes_its_elevenlabs_call_through_the_host() {
    let server = fake_eleven();
    let mut w = codex_world(json!([
        four_scripts("ok"),
        sandboxed_music(),
        {"write": {"path": "project.json5", "content": PROJECT_JSON5}},
        {"command": "cutline audio project.json5 --export youtube-shorts --out outputs/audio-review.wav"},
        {"write": {"path": "summary.md", "content": "## What you will hear\nFour passages over a bed.\n"}}
    ]));
    w.eleven_url = Some(server.url.clone());
    let o = run_codex_audio_review(&w);
    let ev = events(&o);
    let fin = ev.last().unwrap_or_else(|| panic!("no events\n{}", stderr(&o)));
    assert!(matches!(fin["status"].as_str(), Some("succeeded" | "succeeded_with_warnings")), "{fin}\n{}", stderr(&o));
    assert_eq!(server.count("/v1/music"), 1, "the host's call reached the service with the saved key\n{:#?}", tool_texts(&ev));
    assert!(fin["eleven_credits"].as_u64().unwrap() > 0, "{fin}");
    let r = w.load_project().runs.pop().unwrap();
    let folder = camino::Utf8PathBuf::from(&r.folder);
    assert_eq!(std::fs::read_to_string(folder.join("audio/music.mp3")).unwrap(), "FAKE-MP3 music: calm-bed");
    let helper = std::fs::read_to_string(folder.join("logs/helper.ndjson")).unwrap();
    assert_eq!(helper.lines().filter(|l| l.contains("\"music\"")).count(), 1, "{helper}");
    let left: Vec<String> = std::fs::read_dir(folder.join("logs/relay"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != "closed")
        .collect();
    assert!(left.is_empty(), "{left:?}");
    assert!(folder.join("outputs/audio-review.wav").is_file());
}

/// BUG-deft-lily-344X: two accepted changes Codex never reports applied, the second accepted
/// shortly before the first one's deadline, with a failed music call between them (a saved key
/// the service refuses). Only the first is said to have timed out; the second is
/// named apart with its own age; the music failure keeps its own error in the outcome and the
/// summary; the Audio stage isn't a success.
#[test]
fn only_the_expired_codex_patch_times_out_and_the_music_failure_is_kept() {
    let w = codex_world(json!([
        four_scripts("withhold"),
        sandboxed_music(),
        {"sleep": 700},
        {"patch": {"id": 7, "complete": "withhold", "files": [
            {"path": "project.json5", "content": PROJECT_JSON5},
            {"path": "summary.md", "content": "## What you will hear\nNothing yet.\n"}
        ]}},
        {"hang": true}
    ]));
    // a saved key the service refuses: validation passes, the generation fails
    std::fs::write(&w.secrets, json!({"elevenlabs-api-key": "revoked-key"}).to_string()).unwrap();
    let o = run_codex_audio_review(&w);
    let ev = events(&o);
    let fin = ev.last().unwrap_or_else(|| panic!("no events\n{}", stderr(&o)));
    assert_eq!(fin["status"], "failed", "{fin}");
    assert_eq!(fin["eleven_credits"], 0);
    let msg = fin["message"].as_str().unwrap();
    let (timed_out, rest) = msg.split_once("A later change").unwrap_or_else(|| panic!("{msg}"));
    assert!(timed_out.contains("`audio/scene1.txt`") && timed_out.contains("`audio/scene4.txt`"), "{msg}");
    assert!(!timed_out.contains("project.json5"), "the younger change didn't time out: {msg}");
    assert!(rest.contains("`project.json5`, `summary.md`"), "{msg}");
    assert!(!msg.contains("nothing after them ran") && !msg.contains("These changes never completed"), "{msg}");
    let r = w.load_project().runs.pop().unwrap();
    let folder = camino::Utf8PathBuf::from(&r.folder);
    let calls = ndjson(&folder.join("logs/eleven.ndjson"));
    assert_eq!((calls.len(), &calls[0]["ok"], &calls[0]["credits"]), (1, &json!(false), &json!(0)), "{calls:?}");
    let error = calls[0]["error"].as_str().unwrap().trim_end_matches('.').to_string();
    assert!(msg.contains("`video-studio eleven music`") && msg.contains(&error), "{error}\n{msg}");
    assert!(msg.contains("outputs/audio-review.wav"), "{msg}");
    let audio_ok: Vec<bool> = ev
        .iter()
        .filter(|e| e["event"] == "stage_finished" && e["stage"] == "audio")
        .filter_map(|e| e["ok"].as_bool())
        .collect();
    assert_eq!(audio_ok, vec![false], "{ev:#?}");
    let summary = std::fs::read_to_string(folder.join("logs/summary.md")).unwrap();
    assert!(summary.contains(&error), "{summary}");
    assert!(!folder.join("outputs/audio-review.wav").exists());
    assert!(!ev.iter().any(|e| e["event"] == "artifact" && e["kind"] == "output"), "nothing to play");
}

/// BUG-deft-lily-344X: the older change completes just before its deadline. Its timer clears
/// and its files are written once each; the newer one keeps its own deadline and is the only
/// one said to have timed out.
#[test]
fn a_codex_patch_completed_in_time_clears_only_its_own_deadline() {
    let w = codex_world(json!([
        four_scripts("withhold"),
        {"sleep": 600},
        {"patch": {"id": 7, "complete": "withhold", "files": [
            {"path": "project.json5", "content": PROJECT_JSON5},
            {"path": "summary.md", "content": "## What you will hear\nNothing yet.\n"}
        ]}},
        {"sleep": 400},
        {"finish_patch": 2},
        {"hang": true}
    ]));
    let o = run_codex_audio_review(&w);
    let ev = events(&o);
    let fin = ev.last().unwrap_or_else(|| panic!("no events\n{}", stderr(&o)));
    assert_eq!(fin["status"], "failed", "{fin}");
    assert_eq!(ev.iter().filter(|e| e["event"] == "run_finished").count(), 1);
    let msg = fin["message"].as_str().unwrap();
    assert!(msg.contains("`project.json5`, `summary.md`") && !msg.contains("scene1.txt"), "{msg}");
    assert!(!msg.contains("A later change"), "{msg}");
    let tools = tool_texts(&ev);
    for i in 1..=4 {
        let wrote = format!("Wrote `audio/scene{i}.txt`");
        assert_eq!(tools.iter().filter(|t| **t == wrote).count(), 1, "{tools:?}");
    }
    assert!(!tools.iter().any(|t| t.contains("project.json5") && t.starts_with("Wrote")), "{tools:?}");
}
