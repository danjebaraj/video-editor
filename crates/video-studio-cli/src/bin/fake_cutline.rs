//! A stand-in for cutline (ui/spec.md §20) that answers the commands a run uses and writes
//! placeholder outputs, so runs can be tested quickly without ffmpeg.
//!
//! Markers in `project.json5` steer it: `INVALID` makes `validate` fail, `FAIL_RENDER` makes
//! `render` fail, `QA_WARN` and `QA_FAIL` change the QA report. `FAKE_CUTLINE_SLEEP_MS` slows
//! `render` down (for cancellation tests).

use std::io::Write;

use serde_json::json;

/// The values of every `out: "…"` (or `"out": "…"`) in a project file.
fn outs(text: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("out") {
        let after = &rest[i + 3..];
        let after_trim = after.trim_start_matches('"').trim_start();
        if let Some(value) = after_trim.strip_prefix(':') {
            let value = value.trim_start();
            if let Some(body) = value.strip_prefix('"') {
                if let Some(end) = body.find('"') {
                    let before_ok =
                        rest[..i].chars().next_back().map(|c| !c.is_alphanumeric() && c != '_').unwrap_or(true);
                    if before_ok {
                        v.push(body[..end].to_string());
                    }
                }
            }
        }
        rest = &rest[i + 3..];
    }
    v
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json_out = args.iter().any(|a| a == "--json");
    let positional: Vec<&String> = {
        let mut v = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a == "--variant" || a == "--locale" || a == "--export" || a == "--out" || a == "-o" || a == "--jobs" {
                i += 2;
                continue;
            }
            if !a.starts_with('-') {
                v.push(a);
            }
            i += 1;
        }
        v
    };
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("cutline 0.1.0-fake");
        return;
    }
    let Some(command) = positional.first().map(|s| s.as_str()) else {
        println!("cutline (fake)");
        return;
    };
    let file = positional.get(1).map(|s| s.as_str());
    let read = || file.and_then(|f| std::fs::read_to_string(f).ok());
    match command {
        "doctor" => println!(
            "{}",
            json!({"ok": true, "errors": 0, "warnings": 0, "home": std::env::temp_dir().join("fake-cutline-home"), "home_source": "fake", "platform": "fake", "chrome": "/fake/chrome", "whisper_cli": "/fake/whisper", "whisper_models": ["base.en"], "face_model": true, "capabilities": [{"kind": "filter", "name": "xfade", "present": true, "required": true}], "fixes": []})
        ),
        "guide" => println!("# cutline workflow (fake)\n\nValidate, review, render."),
        "probe" => {
            let Some(f) = file else { std::process::exit(2) };
            let Ok(meta) = std::fs::metadata(f) else {
                eprintln!("error: {f}: no such file");
                std::process::exit(1);
            };
            if std::fs::read(f).map(|b| b.starts_with(b"BROKEN")).unwrap_or(false) {
                eprintln!("error: {f}: Invalid data found when processing input");
                std::process::exit(1);
            }
            let audio_only = f.ends_with(".wav") || f.ends_with(".mp3");
            let image = f.ends_with(".png") || f.ends_with(".jpg");
            println!(
                "{}",
                json!({"path": f, "format": "fake", "duration": if image { 0.0 } else { 12.5 }, "size": meta.len(), "kind": if audio_only { "audio" } else if image { "image" } else { "video" },
                "video": if audio_only { serde_json::Value::Null } else { json!({"codec": "h264", "width": 1920, "height": 1080, "display_width": 1920, "display_height": 1080, "fps": "30/1", "fps_f64": 30.0, "vfr": false, "rotation": 0}) },
                "audio": if image { serde_json::Value::Null } else { json!({"codec": "aac", "sample_rate": 48000, "channels": 2}) }})
            );
        }
        "validate" => match read() {
            None => {
                eprintln!("error: can't read {}", file.unwrap_or("the project"));
                std::process::exit(1);
            }
            Some(t) if t.contains("INVALID") => {
                if json_out {
                    println!(
                        "{}",
                        json!({"ok": false, "diagnostics": [{"code": "E_FAKE_INVALID", "path": "/clips/0", "message": "the project is marked INVALID", "severity": "error"}]})
                    );
                }
                eprintln!("error: E_FAKE_INVALID at /clips/0: the project is marked INVALID");
                std::process::exit(1);
            }
            Some(_) => println!(
                "{}",
                if json_out { json!({"ok": true, "diagnostics": []}).to_string() } else { "ok".to_string() }
            ),
        },
        "expand" | "cues" | "beats" | "templates" | "schema" => println!("ok ({command})"),
        "review" => {
            let _ = std::fs::create_dir_all("review/project");
            let _ = std::fs::write("review/project/summary.md", "# Review (fake)\n\nNo findings.\n");
            let _ = std::fs::write(
                "review/project/qa.json",
                json!({"ok": true, "errors": 0, "warnings": 0, "findings": []}).to_string(),
            );
            eprintln!("review: contact sheet 1/2");
            eprintln!("review: contact sheet 2/2");
            println!("review pack: review/project");
        }
        "render" => {
            let Some(text) = read() else {
                eprintln!("error: can't read {}", file.unwrap_or("the project"));
                std::process::exit(1);
            };
            if text.contains("FAIL_RENDER") {
                eprintln!("warning: W_FAKE_SLOW this is a slow fake");
                eprintln!("error: E_FAKE_RENDER ffmpeg exited with status 1");
                std::process::exit(1);
            }
            let sleep = std::env::var("FAKE_CUTLINE_SLEEP_MS").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let list = outs(&text);
            let total = list.len().max(1);
            for (i, out) in list.iter().enumerate() {
                eprintln!("render: segment {}/{}", i + 1, total);
                let _ = std::io::stderr().flush();
                if sleep > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(sleep));
                }
                let path = std::path::Path::new(out);
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, format!("FAKE VIDEO {out}\n").repeat(64));
                let stem = path.with_extension("");
                let stem = stem.display();
                let (ok, errors, warnings) = if text.contains("QA_FAIL") {
                    (false, 1, 0)
                } else if text.contains("QA_WARN") {
                    (true, 0, 1)
                } else {
                    (true, 0, 0)
                };
                let findings = if errors + warnings > 0 {
                    json!([{"check": "loudness", "severity": if errors > 0 { "Error" } else { "Warn" }, "ok": false, "message": "−19.0 LUFS, target −14", "measured": -19.0, "expected": -14.0}])
                } else {
                    json!([])
                };
                let _ = std::fs::write(format!("{stem}.qa.json"), json!({"file": out, "target": "fake", "ok": ok, "errors": errors, "warnings": warnings, "findings": findings}).to_string());
                let _ = std::fs::write(format!("{stem}.srt"), "1\n00:00:00,000 --> 00:00:02,000\nFake caption\n");
                let _ = std::fs::write(format!("{stem}.render_report.json"), json!({"duration": 12.5}).to_string());
            }
            println!("rendered {} file(s)", list.len());
            if text.contains("QA_FAIL") && !args.iter().any(|a| a == "--allow-qa-errors") {
                eprintln!("error: QA failed");
                std::process::exit(1);
            }
        }
        "qa" | "thumbnail" | "chapters" | "preview" | "transcribe" | "init" | "record" => println!("ok ({command})"),
        other => {
            eprintln!("error: unrecognized subcommand '{other}'");
            std::process::exit(2);
        }
    }
}
