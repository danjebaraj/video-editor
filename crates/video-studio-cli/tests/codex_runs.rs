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
