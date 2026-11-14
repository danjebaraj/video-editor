//! Collecting a run's results (spec §12.2 step 6, §12.5): output files, QA reports, captions,
//! posters and the summary.

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::readiness::ReadinessReport;
use crate::paths;
use crate::presets::ResolvedOutput;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QaFinding {
    pub check: String,
    pub severity: String,
    pub message: String,
}

/// One rendered file of an output config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputFile {
    pub output_id: String,
    pub output_name: String,
    pub platform: String,
    /// cutline export name.
    pub export: String,
    pub path: String,
    pub exists: bool,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    /// True when the run was cancelled or failed before this file was verified.
    pub incomplete: bool,
    /// `pass`, `warnings`, `fail`, or `none` when there is no QA report.
    pub qa_verdict: String,
    pub qa_errors: u64,
    pub qa_warnings: u64,
    #[serde(default)]
    pub qa_findings: Vec<QaFinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qa_report: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captions_srt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captions_vtt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_report: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
}

fn sibling(file: &Utf8Path, suffix: &str) -> Option<String> {
    let p = file.with_extension("").as_str().to_string() + suffix;
    Utf8Path::new(&p).is_file().then_some(p)
}

fn find_duration(v: &Value) -> Option<f64> {
    for key in ["duration", "duration_s", "output_duration", "timeline_duration"] {
        if let Some(d) = v.get(key).and_then(Value::as_f64).filter(|d| *d > 0.0) {
            return Some(d);
        }
    }
    None
}

/// Reads a cutline `<name>.qa.json`: verdict, counts and the findings that didn't pass.
pub fn read_qa(path: &Utf8Path) -> Option<(String, u64, u64, Vec<QaFinding>)> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let errors = v["errors"].as_u64().unwrap_or(0);
    let warnings = v["warnings"].as_u64().unwrap_or(0);
    let ok = v["ok"].as_bool().unwrap_or(errors == 0);
    let findings = v["findings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|f| f["ok"].as_bool() == Some(false))
                .map(|f| QaFinding {
                    check: f["check"].as_str().unwrap_or("").to_string(),
                    severity: f["severity"].as_str().unwrap_or("").to_lowercase(),
                    message: f["message"].as_str().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let verdict = if !ok || errors > 0 {
        "fail"
    } else if warnings > 0 {
        "warnings"
    } else {
        "pass"
    };
    Some((verdict.to_string(), errors, warnings, findings))
}

/// Looks for each expected export in the run folder. With `finished_ok` false (cancelled or
/// failed), files that exist are kept and flagged incomplete.
pub fn collect_outputs(folder: &Utf8Path, outputs: &[ResolvedOutput], finished_ok: bool) -> Vec<OutputFile> {
    let mut files = Vec::new();
    for o in outputs {
        for e in &o.exports {
            let path = folder.join(&e.out);
            let exists = path.is_file();
            let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let qa_path = sibling(&path, ".qa.json");
            let qa = qa_path.as_deref().and_then(|p| read_qa(Utf8Path::new(p)));
            let render_report = sibling(&path, ".render_report.json");
            let duration = render_report
                .as_deref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| find_duration(&v));
            let (verdict, qa_errors, qa_warnings, findings) = qa.unwrap_or(("none".into(), 0, 0, Vec::new()));
            files.push(OutputFile {
                output_id: o.id.clone(),
                output_name: o.name.clone(),
                platform: o.platform.clone(),
                export: e.name.clone(),
                path: path.to_string(),
                exists,
                bytes,
                width: o.width,
                height: o.height,
                incomplete: exists && (!finished_ok || bytes == 0),
                qa_verdict: verdict,
                qa_errors,
                qa_warnings,
                qa_findings: findings,
                qa_report: qa_path,
                captions_srt: sibling(&path, ".srt"),
                captions_vtt: sibling(&path, ".vtt"),
                poster: sibling(&path, ".poster.jpg"),
                render_report,
                duration_s: duration,
            });
        }
    }
    files
}

/// Copies QA reports and the review pack's summary into `logs/qa/` so the log folder stands alone.
pub fn copy_qa_to_logs(folder: &Utf8Path, files: &[OutputFile]) {
    let dest = folder.join("logs/qa");
    for f in files {
        for src in [&f.qa_report, &f.render_report].into_iter().flatten() {
            let src = Utf8Path::new(src);
            if let Some(name) = src.file_name() {
                let _ = std::fs::create_dir_all(&dest);
                let _ = std::fs::copy(src, dest.join(name));
            }
        }
    }
    // The review pack: text and JSON only (the images stay where cutline wrote them).
    let review = folder.join("review");
    if review.is_dir() {
        for e in walkdir::WalkDir::new(&review).max_depth(3).into_iter().flatten().filter(|e| e.file_type().is_file()) {
            let Ok(p) = paths::utf8(e.path().to_path_buf()) else { continue };
            if matches!(p.extension(), Some("json" | "md" | "txt")) {
                if let Ok(rel) = p.strip_prefix(&review) {
                    let to = dest.join("review").join(rel);
                    if let Some(d) = to.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::copy(&p, &to);
                }
            }
        }
    }
}

/// Everything the Results and Logs tabs show for one run, read from its folder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunResults {
    pub folder: String,
    pub exists: bool,
    /// `logs/run.json`
    pub run: Value,
    pub outputs: Vec<OutputFile>,
    pub summary: Option<String>,
    pub readiness: Option<ReadinessReport>,
    pub project_json5: Option<String>,
    pub logs: Vec<LogFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogFile {
    /// Path relative to `logs/`.
    pub name: String,
    pub path: String,
    pub bytes: u64,
    /// `app`, `claude`, `cutline`, `eleven`, `validation`, `qa` or `summary`.
    pub source: String,
}

pub fn log_source(name: &str) -> &'static str {
    if name.starts_with("cutline/") {
        "cutline"
    } else if name.starts_with("qa/") {
        "qa"
    } else if name == "claude.jsonl" {
        "claude"
    } else if name == "eleven.ndjson" {
        "eleven"
    } else if name == "validation.json" {
        "validation"
    } else if name == "summary.md" {
        "summary"
    } else {
        "app"
    }
}

pub fn list_logs(folder: &Utf8Path) -> Vec<LogFile> {
    let logs = folder.join("logs");
    let mut out: Vec<LogFile> = walkdir::WalkDir::new(&logs)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file() && !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let p = paths::utf8(e.path().to_path_buf()).ok()?;
            let name = p.strip_prefix(&logs).ok()?.as_str().replace('\\', "/");
            Some(LogFile {
                source: log_source(&name).to_string(),
                bytes: e.metadata().map(|m| m.len()).unwrap_or(0),
                path: p.to_string(),
                name,
            })
        })
        .collect();
    // The order of the spec's table first, then the per-invocation logs.
    let rank = |n: &str| {
        [
            "run.json",
            "summary.md",
            "events.ndjson",
            "claude.jsonl",
            "eleven.ndjson",
            "validation.json",
            "permissions.ndjson",
        ]
        .iter()
        .position(|x| *x == n)
        .unwrap_or(99)
    };
    out.sort_by(|a, b| rank(&a.name).cmp(&rank(&b.name)).then(a.name.cmp(&b.name)));
    out
}

/// Loads a finished (or running) run from its folder.
pub fn load_results(folder: &Utf8Path) -> RunResults {
    let run: Value = paths::read_json(&folder.join("logs/run.json")).unwrap_or(Value::Null);
    let outputs: Vec<ResolvedOutput> = serde_json::from_value(run["outputs"].clone()).unwrap_or_default();
    let status = run["status"].as_str().unwrap_or("");
    let finished_ok = status == "succeeded" || status == "succeeded_with_warnings";
    let project = folder.join("project.json5");
    RunResults {
        folder: folder.to_string(),
        exists: folder.is_dir(),
        outputs: collect_outputs(folder, &outputs, finished_ok),
        summary: std::fs::read_to_string(folder.join("logs/summary.md"))
            .or_else(|_| std::fs::read_to_string(folder.join("summary.md")))
            .ok(),
        readiness: ReadinessReport::load(folder),
        project_json5: project.is_file().then(|| project.to_string()),
        logs: list_logs(folder),
        run,
    }
}

/// Other files worth showing as artifacts: generated audio and the review pack.
pub fn extra_artifacts(folder: &Utf8Path) -> Vec<(String, Utf8PathBuf)> {
    let mut out = Vec::new();
    for (kind, dir) in [("audio", "audio"), ("review", "review")] {
        let d = folder.join(dir);
        if !d.is_dir() {
            continue;
        }
        for e in walkdir::WalkDir::new(&d).max_depth(3).into_iter().flatten().filter(|e| e.file_type().is_file()) {
            if let Ok(p) = paths::utf8(e.path().to_path_buf()) {
                let keep = match kind {
                    "audio" => matches!(p.extension(), Some("mp3" | "wav" | "m4a" | "json")),
                    _ => {
                        matches!(p.file_name(), Some(n) if n.starts_with("contact") || n.starts_with("summary") || n.ends_with("qa.json"))
                    }
                };
                if keep {
                    out.push((kind.to_string(), p));
                }
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::{resolve_output, OutputConfig, Platforms};
    use std::collections::HashSet;

    #[test]
    fn collects_outputs_with_qa_and_sidecars() {
        let d = tempfile::tempdir().unwrap();
        let folder = Utf8Path::from_path(d.path()).unwrap();
        let p = Platforms::bundled();
        let mut taken = HashSet::new();
        let yt = resolve_output(
            &OutputConfig {
                id: "o1".into(),
                preset: "youtube_1080p".into(),
                name: None,
                overrides: Default::default(),
            },
            &p,
            &mut taken,
        )
        .unwrap();
        let lp = resolve_output(
            &OutputConfig {
                id: "o2".into(),
                preset: "website_hero_loop".into(),
                name: None,
                overrides: Default::default(),
            },
            &p,
            &mut taken,
        )
        .unwrap();
        let dir = folder.join("outputs/youtube-1080p");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("youtube-1080p.mp4"), b"mp4-bytes").unwrap();
        std::fs::write(dir.join("youtube-1080p.srt"), b"1\n").unwrap();
        std::fs::write(dir.join("youtube-1080p.poster.jpg"), b"jpg").unwrap();
        std::fs::write(dir.join("youtube-1080p.render_report.json"), r#"{"duration": 44.5}"#).unwrap();
        std::fs::write(dir.join("youtube-1080p.qa.json"), r#"{"ok": true, "errors": 0, "warnings": 1, "findings": [{"check": "loudness", "severity": "Warn", "ok": false, "message": "−17.2 LUFS, target −14"}, {"check": "black", "severity": "Error", "ok": true, "message": "none"}]}"#).unwrap();
        let files = collect_outputs(folder, &[yt.clone(), lp.clone()], true);
        assert_eq!(files.len(), 3);
        let a = &files[0];
        assert!(a.exists && !a.incomplete && a.bytes == 9);
        assert_eq!((a.qa_verdict.as_str(), a.qa_warnings, a.qa_findings.len()), ("warnings", 1, 1));
        assert_eq!(
            a.qa_findings[0],
            QaFinding {
                check: "loudness".into(), severity: "warn".into(), message: "−17.2 LUFS, target −14".into()
            }
        );
        assert!(a.captions_srt.is_some() && a.poster.is_some() && a.captions_vtt.is_none());
        assert_eq!(a.duration_s, Some(44.5));
        assert!(!files[1].exists && files[1].qa_verdict == "none");
        assert_eq!(files[2].export, "website-hero-loop-webm");
        // cancelled: what exists is kept and flagged
        let cancelled = collect_outputs(folder, &[yt], false);
        assert!(cancelled[0].exists && cancelled[0].incomplete);
        // failing QA
        std::fs::write(
            dir.join("youtube-1080p.qa.json"),
            r#"{"ok": false, "errors": 2, "warnings": 0, "findings": []}"#,
        )
        .unwrap();
        assert_eq!(collect_outputs(folder, &[lp], true)[0].qa_verdict, "none");
        assert_eq!(read_qa(&dir.join("youtube-1080p.qa.json")).unwrap().0, "fail");
        copy_qa_to_logs(folder, &files);
        assert!(folder.join("logs/qa/youtube-1080p.qa.json").exists());
    }

    #[test]
    fn lists_logs_in_a_stable_order() {
        let d = tempfile::tempdir().unwrap();
        let folder = Utf8Path::from_path(d.path()).unwrap();
        for f in [
            "logs/cutline/002-render.log",
            "logs/cutline/001-validate.log",
            "logs/claude.jsonl",
            "logs/run.json",
            "logs/events.ndjson",
            "logs/qa/a.qa.json",
            "logs/.eleven.lock",
            "logs/eleven.ndjson",
        ] {
            let p = folder.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"{}").unwrap();
        }
        let logs = list_logs(folder);
        assert_eq!(
            logs.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            vec![
                "run.json",
                "events.ndjson",
                "claude.jsonl",
                "eleven.ndjson",
                "cutline/001-validate.log",
                "cutline/002-render.log",
                "qa/a.qa.json"
            ]
        );
        assert_eq!(
            logs.iter().map(|l| l.source.as_str()).collect::<Vec<_>>(),
            vec!["app", "app", "claude", "eleven", "cutline", "cutline", "qa"]
        );
        let r = load_results(folder);
        assert!(r.exists && r.outputs.is_empty() && r.summary.is_none());
    }
}
