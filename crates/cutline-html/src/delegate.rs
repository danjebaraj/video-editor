//! Rendering in another process (BUG-stellar-lily-K8CN).
//!
//! Chrome can't start inside some sandboxes: under Codex's macOS sandbox it aborts at launch,
//! because registering its Mach rendezvous service is denied. An app that runs cutline there
//! sets [`DELEGATE_ENV`] to a program that renders outside it. cutline then writes the job to a
//! JSON file and runs `<program> browser-job <file>`; that program runs
//! `cutline internal browser-job <file>` where Chrome can start, which fills the same cache
//! folder, and cutline reads the result from the cache as if it had rendered it itself.
//!
//! The job names only the page or script and the cache folder: the program that takes it decides
//! whether those are allowed. [`OFFLINE_ENV`] keeps a recording to local pages.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::RenderSpec;

/// The program that takes browser jobs.
pub const DELEGATE_ENV: &str = "CUTLINE_BROWSER_DELEGATE";
/// Set (to anything) by whoever runs the job when the run has no web access: a recording may load
/// only local pages.
pub const OFFLINE_ENV: &str = "CUTLINE_BROWSER_OFFLINE";

/// One browser job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "job", rename_all = "snake_case")]
pub enum Job {
    /// An HTML / SVG / Lottie graphic rendered into `cache_dir` (see [`crate::render_cached`]).
    Html { spec: RenderSpec, cache_dir: PathBuf },
    /// A browser recording into `cache_dir` (see [`crate::demo::record_cached`]).
    Demo { script: PathBuf, cache_dir: PathBuf, force: bool, locale: Option<String> },
}

impl Job {
    /// The folder the job writes.
    pub fn cache_dir(&self) -> &Path {
        match self {
            Job::Html { cache_dir, .. } | Job::Demo { cache_dir, .. } => cache_dir,
        }
    }
}

/// The delegate program, when one is set.
pub fn delegate() -> Option<PathBuf> {
    std::env::var_os(DELEGATE_ENV).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Whether recordings must stay on local pages.
pub fn offline() -> bool {
    std::env::var_os(OFFLINE_ENV).is_some_and(|v| !v.is_empty())
}

/// Hands `job` to `program` and waits for it. The job file sits in the cache folder (which the
/// job writes anyway) and is removed afterwards.
pub fn run(program: &Path, job: &Job) -> Result<()> {
    let dir = job.cache_dir();
    std::fs::create_dir_all(dir)?;
    let file = dir.join(format!("browser-job-{}-{}.json", std::process::id(), nanos()));
    std::fs::write(&file, serde_json::to_vec_pretty(job)?)?;
    let out = std::process::Command::new(program)
        .arg("browser-job")
        .arg(&file)
        .env_remove(DELEGATE_ENV)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("couldn't start the browser renderer {}", program.display()));
    let _ = std::fs::remove_file(&file);
    let out = out?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        bail!(
            "the browser renderer {} failed ({}){}",
            program.display(),
            out.status,
            if err.is_empty() { String::new() } else { format!(": {err}") }
        );
    }
    Ok(())
}

/// Runs a job here (`cutline internal browser-job`): renders into its cache folder.
pub fn execute(tc: &cutline_deps::Toolchain, job: &Job) -> Result<PathBuf> {
    match job {
        Job::Html { spec, cache_dir } => crate::render_cached(tc, spec, cache_dir),
        Job::Demo { script, cache_dir, force, locale } => {
            Ok(crate::demo::record_cached(tc, script, cache_dir, *force, locale.as_deref())?.video)
        }
    }
}

fn nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}
