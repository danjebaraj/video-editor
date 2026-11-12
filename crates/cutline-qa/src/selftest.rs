//! `cutline selftest`: toolchain + pipeline checks on synthetic media.

use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    Skip,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub id: String,
    pub status: Status,
    pub measured: serde_json::Value,
    pub threshold: serde_json::Value,
    pub duration_s: f64,
    pub artifacts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Outcome a check function returns.
pub struct Outcome {
    pub status: Status,
    pub measured: serde_json::Value,
    pub threshold: serde_json::Value,
    pub artifacts: Vec<PathBuf>,
    pub message: Option<String>,
}

impl Outcome {
    pub fn pass(measured: serde_json::Value, threshold: serde_json::Value) -> Self {
        Self {
            status: Status::Pass,
            measured,
            threshold,
            artifacts: vec![],
            message: None,
        }
    }
    pub fn check(ok: bool, measured: serde_json::Value, threshold: serde_json::Value) -> Self {
        Self {
            status: if ok { Status::Pass } else { Status::Fail },
            measured,
            threshold,
            artifacts: vec![],
            message: None,
        }
    }
    pub fn skip(why: impl Into<String>) -> Self {
        Self {
            status: Status::Skip,
            measured: serde_json::Value::Null,
            threshold: serde_json::Value::Null,
            artifacts: vec![],
            message: Some(why.into()),
        }
    }
    pub fn artifacts(mut self, a: Vec<PathBuf>) -> Self {
        self.artifacts = a;
        self
    }
    pub fn msg(mut self, m: impl Into<String>) -> Self {
        self.message = Some(m.into());
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub mode: String,
    pub ok: bool,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration_s: f64,
    pub workdir: String,
    pub ffmpeg_version: Option<String>,
    pub checks: Vec<CheckResult>,
}

/// Collects checks while running them.
pub struct Runner {
    pub dir: PathBuf,
    pub checks: Vec<CheckResult>,
    start: Instant,
    pub filter: Option<String>,
}

impl Runner {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            checks: vec![],
            start: Instant::now(),
            filter: std::env::var("CUTLINE_SELFTEST_ONLY").ok(),
        }
    }

    pub fn run(&mut self, id: &str, f: impl FnOnce(&Path) -> Result<Outcome>) {
        if let Some(fl) = &self.filter {
            if !id.contains(fl.as_str()) {
                return;
            }
        }
        let t = Instant::now();
        let d = self.dir.join(id.replace(['/', ':'], "_"));
        let _ = std::fs::create_dir_all(&d);
        eprintln!("selftest: {id} ...");
        let o = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&d))) {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => Outcome {
                status: Status::Fail,
                measured: serde_json::Value::Null,
                threshold: serde_json::Value::Null,
                artifacts: vec![],
                message: Some(format!("{e:#}")),
            },
            Err(_) => Outcome {
                status: Status::Fail,
                measured: serde_json::Value::Null,
                threshold: serde_json::Value::Null,
                artifacts: vec![],
                message: Some("panic".into()),
            },
        };
        let r = CheckResult {
            id: id.to_string(),
            status: o.status,
            measured: o.measured,
            threshold: o.threshold,
            duration_s: t.elapsed().as_secs_f64(),
            artifacts: o
                .artifacts
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            message: o.message,
        };
        eprintln!(
            "selftest: {id}: {:?} ({:.1}s){}",
            r.status,
            r.duration_s,
            r.message
                .as_deref()
                .map(|m| format!(" — {m}"))
                .unwrap_or_default()
        );
        self.checks.push(r);
    }

    pub fn finish(self, mode: &str) -> Report {
        let passed = self
            .checks
            .iter()
            .filter(|c| c.status == Status::Pass)
            .count();
        let failed = self
            .checks
            .iter()
            .filter(|c| c.status == Status::Fail)
            .count();
        let skipped = self
            .checks
            .iter()
            .filter(|c| c.status == Status::Skip)
            .count();
        Report {
            mode: mode.into(),
            ok: failed == 0,
            passed,
            failed,
            skipped,
            duration_s: self.start.elapsed().as_secs_f64(),
            workdir: self.dir.display().to_string(),
            ffmpeg_version: cutline_deps::toolchain().ok().map(|t| t.version.clone()),
            checks: self.checks,
        }
    }
}

pub fn summary(r: &Report) -> String {
    let mut s = format!(
        "selftest --{}: {} ({} passed, {} failed, {} skipped) in {:.1}s\n",
        r.mode,
        if r.ok { "PASS" } else { "FAIL" },
        r.passed,
        r.failed,
        r.skipped,
        r.duration_s
    );
    for c in &r.checks {
        let mark = match c.status {
            Status::Pass => "✔",
            Status::Fail => "✘",
            Status::Skip => "·",
        };
        s.push_str(&format!(
            "  {mark} {:<36} {:>6.1}s {}\n",
            c.id,
            c.duration_s,
            c.message.as_deref().unwrap_or("")
        ));
    }
    s
}
