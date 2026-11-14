//! Run logs (spec §15): export as a zip, the issue report, storage sizes and cleanup.
//! Logs are never pruned automatically.

use std::io::Write;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::run::collect::{self, read_qa};
use crate::{paths, timeutil, Error, Result};

/// Replaces absolute paths and file names in a media manifest with stable placeholders, so logs
/// can be shared without revealing where files live or what they are called.
pub fn redact_manifest(manifest: &Value) -> Value {
    let mut out = manifest.clone();
    if let Some(items) = out.as_array_mut() {
        for (i, item) in items.iter_mut().enumerate() {
            if let Some(o) = item.as_object_mut() {
                if let Some(p) = o.get("path").and_then(Value::as_str) {
                    let ext = Utf8Path::new(p).extension().map(|e| format!(".{e}")).unwrap_or_default();
                    o.insert("path".into(), Value::String(format!("<media-{}{ext}>", i + 1)));
                }
                o.remove("notes");
            }
        }
    }
    out
}

/// Every absolute media path and file name in the manifest, longest first, with its placeholder.
fn redactions(manifest: &Value) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    if let Some(items) = manifest.as_array() {
        for (i, item) in items.iter().enumerate() {
            if let Some(p) = item["path"].as_str() {
                let ext = Utf8Path::new(p).extension().map(|e| format!(".{e}")).unwrap_or_default();
                let placeholder = format!("<media-{}{ext}>", i + 1);
                v.push((p.to_string(), placeholder.clone()));
                if let Some(name) = Utf8Path::new(p).file_name() {
                    if name.len() >= 4 {
                        v.push((name.to_string(), placeholder));
                    }
                }
            }
        }
    }
    v.sort_by_key(|r| std::cmp::Reverse(r.0.len()));
    v
}

fn redact_text(text: &str, redactions: &[(String, String)]) -> String {
    let mut s = text.to_string();
    for (from, to) in redactions {
        if s.contains(from.as_str()) {
            s = s.replace(from.as_str(), to);
        }
        // JSON logs hold the same paths with escaped slashes or quotes
        let escaped = serde_json::to_string(from).unwrap_or_default();
        let escaped = escaped.trim_matches('"');
        if escaped != from && s.contains(escaped) {
            s = s.replace(escaped, to);
        }
    }
    s
}

/// Export logs… — a zip of the log folder plus `project.json5`, `spec.md` and the media manifest.
/// With `redact`, media paths and file names are replaced throughout the text files.
pub fn export_logs(run_folder: &Utf8Path, dest: &Utf8Path, redact: bool) -> Result<u64> {
    let logs = run_folder.join("logs");
    if !logs.is_dir() {
        return Err(Error::NotFound(format!("{run_folder} has no logs folder")));
    }
    let manifest: Value = paths::read_json(&run_folder.join("media-manifest.json")).unwrap_or(Value::Array(Vec::new()));
    let rules = if redact { redactions(&manifest) } else { Vec::new() };
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut count = 0u64;
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut add = |name: &str, bytes: Vec<u8>| -> Result<()> {
            let is_text = matches!(
                Utf8Path::new(name).extension(),
                Some("json" | "ndjson" | "jsonl" | "log" | "md" | "json5" | "txt" | "srt" | "vtt")
            );
            let bytes = if redact && is_text {
                redact_text(&String::from_utf8_lossy(&bytes), &rules).into_bytes()
            } else {
                bytes
            };
            zip.start_file(name, opts).map_err(|e| Error::other(e.to_string()))?;
            zip.write_all(&bytes)?;
            count += 1;
            Ok(())
        };
        for f in collect::list_logs(run_folder) {
            add(&format!("logs/{}", f.name), std::fs::read(&f.path)?)?;
        }
        for name in ["project.json5", "spec.md", "run.json", "outputs.json", "readiness.json"] {
            let p = run_folder.join(name);
            if p.is_file() {
                add(name, std::fs::read(&p)?)?;
            }
        }
        let m = if redact { redact_manifest(&manifest) } else { manifest.clone() };
        add("media-manifest.json", paths::to_pretty_json(&m)?.into_bytes())?;
        zip.finish().map_err(|e| Error::other(e.to_string()))?;
    }
    paths::write_atomic(dest, buf.get_ref())?;
    Ok(count)
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Copy issue report — a Markdown summary of cutline errors, warnings and failed QA checks with
/// their log excerpts, ready to paste into a cutline issue or a Claude session that fixes cutline.
pub fn issue_report(run_folder: &Utf8Path) -> String {
    let run: Value = paths::read_json(&run_folder.join("logs/run.json")).unwrap_or(Value::Null);
    let mut s = String::from("# cutline issue report\n\n");
    let get = |v: &Value| v.as_str().unwrap_or("unknown").to_string();
    s.push_str(&format!(
        "- Run: `{}` ({}, status **{}**)\n",
        get(&run["run_id"]),
        get(&run["kind"]).replace('_', " "),
        get(&run["status"]).replace('_', " ")
    ));
    s.push_str(&format!("- cutline: {} at `{}`\n", get(&run["cutline"]["version"]), get(&run["cutline"]["path"])));
    s.push_str(&format!("- Claude Code: {} · model {}\n", get(&run["claude"]["version"]), get(&run["model_used"])));
    s.push_str(&format!(
        "- OS: {} {}\n- App core: {}\n",
        get(&run["os"]["family"]),
        get(&run["os"]["arch"]),
        get(&run["app"]["core_version"])
    ));
    if let Some(m) = run["message"].as_str() {
        s.push_str(&format!("- Outcome: {m}\n"));
    }
    let outputs: Vec<String> = run["outputs"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| format!("{} ({}, {}×{})", get(&o["name"]), get(&o["target"]), o["width"], o["height"]))
                .collect()
        })
        .unwrap_or_default();
    if !outputs.is_empty() {
        s.push_str(&format!("- Outputs: {}\n", outputs.join("; ")));
    }

    // cutline invocations that failed, with the end of their logs
    let dir = run_folder.join("logs/cutline");
    let mut failed = 0;
    let mut warn_lines: Vec<String> = Vec::new();
    let mut logs: Vec<Utf8PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten().filter_map(|e| paths::utf8(e.path()).ok()).filter(|p| p.extension() == Some("log")).collect()
        })
        .unwrap_or_default();
    logs.sort();
    let total = logs.len();
    for log in &logs {
        let Ok(text) = std::fs::read_to_string(log) else { continue };
        let code = text
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix("# exit: "))
            .and_then(|c| c.trim().parse::<i32>().ok())
            .unwrap_or(0);
        for l in text.lines() {
            let body = l.trim_start_matches("[err] ").trim_start_matches("[out] ");
            let lower = body.to_ascii_lowercase();
            if (lower.contains("warning") || body.contains("W_"))
                && warn_lines.len() < 40
                && !warn_lines.iter().any(|w| w == body)
            {
                warn_lines.push(body.to_string());
            }
        }
        if code != 0 {
            failed += 1;
            if failed == 1 {
                s.push_str("\n## Failed cutline commands\n");
            }
            let argv = text.lines().find_map(|l| l.strip_prefix("# argv: ")).unwrap_or("");
            let args: Vec<String> = serde_json::from_str(argv).unwrap_or_default();
            let body: String = text.lines().filter(|l| !l.starts_with("# ")).collect::<Vec<_>>().join("\n");
            s.push_str(&format!(
                "\n### `cutline {}` — exit {code}\n\nLog: `logs/cutline/{}`\n\n```\n{}\n```\n",
                args.iter().skip(1).cloned().collect::<Vec<_>>().join(" "),
                log.file_name().unwrap_or(""),
                tail_lines(&body, 40)
            ));
        }
    }
    if failed == 0 {
        s.push_str(&format!("\nNo cutline command failed ({total} ran).\n"));
    }
    if !warn_lines.is_empty() {
        s.push_str("\n## cutline warnings\n\n```\n");
        s.push_str(&warn_lines.join("\n"));
        s.push_str("\n```\n");
    }

    // QA checks that didn't pass
    let qa_dir = run_folder.join("logs/qa");
    let mut qa_files: Vec<Utf8PathBuf> = std::fs::read_dir(&qa_dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| paths::utf8(e.path()).ok())
                .filter(|p| p.as_str().ends_with(".qa.json"))
                .collect()
        })
        .unwrap_or_default();
    qa_files.sort();
    let mut any_qa = false;
    for f in qa_files {
        let Some((verdict, errors, warnings, findings)) = read_qa(&f) else { continue };
        if findings.is_empty() {
            continue;
        }
        if !any_qa {
            s.push_str("\n## QA findings\n");
            any_qa = true;
        }
        s.push_str(&format!(
            "\n### {} — {verdict} ({errors} errors, {warnings} warnings)\n\n",
            f.file_name().unwrap_or("")
        ));
        for x in findings {
            s.push_str(&format!("- **{}** ({}): {}\n", x.check, x.severity, x.message));
        }
    }

    // What the broker refused: often a sign the workflow guide and the tool disagree
    let denied: Vec<Value> = std::fs::read_to_string(run_folder.join("logs/permissions.ndjson"))
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .filter(|v| v["decision"] == "deny")
                .collect()
        })
        .unwrap_or_default();
    if !denied.is_empty() {
        s.push_str("\n## Blocked tool calls\n\n");
        for d in denied.iter().take(20) {
            let what = d["input"]["command"].as_str().or(d["input"]["file_path"].as_str()).unwrap_or("");
            s.push_str(&format!(
                "- {} `{}` — {}\n",
                get(&d["tool"]),
                what.chars().take(160).collect::<String>(),
                get(&d["reason"])
            ));
        }
    }
    if let Ok(summary) = std::fs::read_to_string(run_folder.join("summary.md")) {
        s.push_str("\n## Claude's summary\n\n");
        s.push_str(summary.trim());
        s.push('\n');
    }
    s
}

fn dir_size(dir: &Utf8Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunStorage {
    pub folder: String,
    pub exists: bool,
    pub total_bytes: u64,
    pub logs_bytes: u64,
    pub started: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageInfo {
    pub eleven_cache_bytes: u64,
    pub dry_runs_bytes: u64,
    pub runs: Vec<RunStorage>,
}

/// Sizes for Settings › Storage. `run_folders` are the production run folders the app knows of
/// (from open and recent projects); dry runs live in the library and are found here.
pub fn storage_info(library: &Utf8Path, run_folders: &[String]) -> StorageInfo {
    let mut runs: Vec<RunStorage> = Vec::new();
    let mut add = |folder: &Utf8Path| {
        let run: Value = paths::read_json(&folder.join("logs/run.json")).unwrap_or(Value::Null);
        runs.push(RunStorage {
            folder: folder.to_string(),
            exists: folder.is_dir(),
            total_bytes: dir_size(folder),
            logs_bytes: dir_size(&folder.join("logs")),
            started: run["started"].as_str().map(str::to_string),
            kind: run["kind"].as_str().map(str::to_string),
        });
    };
    for f in run_folders {
        add(Utf8Path::new(f));
    }
    let dry = library.join("dry-runs");
    for f in dry_run_folders(library) {
        add(&f);
    }
    StorageInfo {
        eleven_cache_bytes: crate::eleven::cache_size(&crate::eleven::cache_dir(library)),
        dry_runs_bytes: dir_size(&dry),
        runs,
    }
}

fn dry_run_folders(library: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut out = Vec::new();
    for project in std::fs::read_dir(library.join("dry-runs")).into_iter().flatten().flatten() {
        for run in std::fs::read_dir(project.path()).into_iter().flatten().flatten() {
            if let Ok(p) = paths::utf8(run.path()) {
                if p.is_dir() {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn is_run_folder(folder: &Utf8Path) -> bool {
    folder.join("logs/run.json").is_file()
}

fn older_than(folder: &Utf8Path, days: u32) -> bool {
    let run: Value = paths::read_json(&folder.join("logs/run.json")).unwrap_or(Value::Null);
    let Some(started) = run["started"].as_str().and_then(timeutil::parse) else { return false };
    run["status"] != "running" && (chrono::Utc::now() - started).num_days() >= days as i64
}

/// Deletes the given run folders that are older than `days` (and all dry runs older than that).
/// Only folders that hold a `logs/run.json` are touched. Returns the bytes freed.
pub fn cleanup_runs(library: &Utf8Path, run_folders: &[String], days: u32) -> u64 {
    let mut freed = 0;
    let all: Vec<Utf8PathBuf> = run_folders.iter().map(Utf8PathBuf::from).chain(dry_run_folders(library)).collect();
    for f in all {
        if is_run_folder(&f) && older_than(&f, days) {
            let size = dir_size(&f);
            if std::fs::remove_dir_all(&f).is_ok() {
                freed += size;
            }
        }
    }
    freed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Read;

    fn run_fixture(root: &Utf8Path) -> Utf8PathBuf {
        let run = root.join("2026-11-14-1230-social-pack");
        let w = |name: &str, text: &str| {
            let p = run.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        w("logs/run.json", &json!({"run_id": "run_1", "kind": "production", "status": "failed", "started": "2020-01-01T00:00:00Z", "cutline": {"version": "0.1.0", "path": "/bin/cutline"}, "claude": {"version": "2.1.311 (Claude Code)"}, "model_used": "claude-opus-5-5", "os": {"family": "macos", "arch": "aarch64"}, "app": {"core_version": "0.1.0"}, "message": "Claude finished without producing this output: shorts", "outputs": [{"name": "YouTube Shorts", "target": "shorts", "width": 1080, "height": 1920}]}).to_string());
        w(
            "logs/events.ndjson",
            "{\"event\":\"warning\",\"message\":\"Media at /Users/ana/Footage/secret-launch.mov\"}\n",
        );
        w("logs/claude.jsonl", "{\"type\":\"assistant\",\"path\":\"/Users/ana/Footage/secret-launch.mov\"}\n");
        w("logs/cutline/001-validate.log", "# argv: [\"/bin/cutline\",\"validate\",\"--json\",\"project.json5\"]\n# cwd: /r\n[out] {\"ok\":true}\n# exit: 0\n# duration: 0.10 s\n");
        w("logs/cutline/002-render.log", "# argv: [\"/bin/cutline\",\"render\",\"project.json5\"]\n# cwd: /r\n[err] warning: W_MAX_BYTES_TIGHT the cap leaves little for video\n[err] error: E_ASSET_MISSING /Users/ana/Footage/secret-launch.mov\n# exit: 1\n# duration: 4.20 s\n");
        w("logs/qa/shorts.qa.json", &json!({"ok": false, "errors": 1, "warnings": 0, "findings": [{"check": "black_frames", "severity": "Error", "ok": false, "message": "0.8 s of black at 12.4 s"}, {"check": "loudness", "severity": "Warn", "ok": true, "message": "fine"}]}).to_string());
        w("logs/permissions.ndjson", "{\"tool\":\"Bash\",\"decision\":\"deny\",\"reason\":\"Only cutline…\",\"input\":{\"command\":\"ls -la\"}}\n{\"tool\":\"Read\",\"decision\":\"allow\",\"input\":{}}\n");
        w("project.json5", "{assets: {hero: \"/Users/ana/Footage/secret-launch.mov\"}}");
        w("summary.md", "## What I made\nNothing rendered.\n");
        w("media-manifest.json", &json!([{"id": "m-hero", "kind": "video", "path": "/Users/ana/Footage/secret-launch.mov", "status": "ok", "notes": "client name inside"}]).to_string());
        run
    }

    fn unzip(path: &Utf8Path) -> std::collections::BTreeMap<String, String> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut out = std::collections::BTreeMap::new();
        for i in 0..z.len() {
            let mut f = z.by_index(i).unwrap();
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            out.insert(f.name().to_string(), s);
        }
        out
    }

    #[test]
    fn export_zips_logs_and_can_redact_media_paths() {
        let d = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(d.path()).unwrap();
        let run = run_fixture(root);
        let plain = root.join("plain.zip");
        let n = export_logs(&run, &plain, false).unwrap();
        let files = unzip(&plain);
        assert_eq!(n as usize, files.len());
        for name in [
            "logs/run.json",
            "logs/events.ndjson",
            "logs/claude.jsonl",
            "logs/cutline/001-validate.log",
            "logs/cutline/002-render.log",
            "logs/qa/shorts.qa.json",
            "project.json5",
            "media-manifest.json",
        ] {
            assert!(files.contains_key(name), "missing {name}: {:?}", files.keys());
        }
        assert!(files["project.json5"].contains("secret-launch.mov"));
        let redacted = root.join("redacted.zip");
        export_logs(&run, &redacted, true).unwrap();
        let files = unzip(&redacted);
        for (name, text) in &files {
            assert!(
                !text.contains("secret-launch") && !text.contains("/Users/ana"),
                "{name} still names the media: {text}"
            );
        }
        assert!(files["project.json5"].contains("<media-1.mov>"));
        assert!(files["logs/cutline/002-render.log"].contains("E_ASSET_MISSING <media-1.mov>"));
        let m: Value = serde_json::from_str(&files["media-manifest.json"]).unwrap();
        assert_eq!(m[0]["path"], "<media-1.mov>");
        assert!(m[0].get("notes").is_none());
        assert!(export_logs(&root.join("nope"), &plain, false).is_err());
    }

    #[test]
    fn issue_report_has_errors_warnings_qa_and_excerpts() {
        let d = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(d.path()).unwrap();
        let run = run_fixture(root);
        let r = issue_report(&run);
        for needle in [
            "# cutline issue report",
            "status **failed**",
            "cutline: 0.1.0",
            "model claude-opus-5-5",
            "### `cutline render project.json5` — exit 1",
            "E_ASSET_MISSING",
            "logs/cutline/002-render.log",
            "## cutline warnings",
            "W_MAX_BYTES_TIGHT",
            "## QA findings",
            "**black_frames** (error): 0.8 s of black at 12.4 s",
            "## Blocked tool calls",
            "`ls -la`",
            "## Claude's summary",
            "Nothing rendered.",
        ] {
            assert!(r.contains(needle), "missing {needle:?} in\n{r}");
        }
        assert!(!r.contains("cutline validate --json project.json5` — exit"), "passing commands aren't listed");
        assert!(!r.contains("loudness"), "passing QA checks aren't listed");
    }

    #[test]
    fn storage_and_cleanup_only_touch_old_run_folders() {
        let d = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(d.path()).unwrap();
        let lib = root.join("lib");
        let old = run_fixture(root);
        // a recent dry run in the library, and a folder that isn't a run
        let fresh = lib.join("dry-runs/prj_1/2026-11-14-0900-x");
        std::fs::create_dir_all(fresh.join("logs")).unwrap();
        std::fs::write(
            fresh.join("logs/run.json"),
            json!({"kind": "dry_run", "status": "succeeded", "started": timeutil::now_rfc3339()}).to_string(),
        )
        .unwrap();
        let not_a_run = root.join("holiday-photos");
        std::fs::create_dir_all(&not_a_run).unwrap();
        std::fs::write(not_a_run.join("a.jpg"), b"x").unwrap();
        std::fs::create_dir_all(crate::eleven::cache_dir(&lib)).unwrap();
        std::fs::write(crate::eleven::cache_dir(&lib).join("k.mp3"), vec![0u8; 1234]).unwrap();
        let folders = vec![old.to_string(), not_a_run.to_string()];
        let info = storage_info(&lib, &folders);
        assert_eq!(info.eleven_cache_bytes, 1234);
        assert_eq!(info.runs.len(), 3);
        assert!(info.runs[0].total_bytes > info.runs[0].logs_bytes && info.runs[0].logs_bytes > 0);
        assert_eq!(info.runs[2].kind.as_deref(), Some("dry_run"));
        let freed = cleanup_runs(&lib, &folders, 30);
        assert!(freed > 0);
        assert!(!old.exists(), "the 2020 run is removed");
        assert!(fresh.exists(), "a recent dry run stays");
        assert!(not_a_run.join("a.jpg").exists(), "a folder that isn't a run is never deleted");
    }
}
