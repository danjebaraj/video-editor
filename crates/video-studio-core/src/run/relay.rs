//! The ElevenLabs relay for Codex runs (BUG-deft-lily-344X).
//!
//! Codex runs every command in its sandbox, approved or not. On macOS that sandbox denies the
//! Keychain (`mach-lookup com.apple.SecurityServer`), which the `keyring` crate reports as "no
//! entry", and it denies outbound network. So `video-studio eleven …` run by Codex can neither
//! read the stored key nor reach ElevenLabs. Instead, the helper hands the call to the run's host,
//! the process that started Codex and isn't sandboxed. The host runs the same command with the
//! same run context and passes back its output and exit code. The key never enters the sandbox,
//! and the agent never sees it.
//!
//! The exchange is files in `logs/relay/`, the only place both sides can write:
//! `<id>.request.json` (the helper's arguments and working folder), renamed to `<id>.taken` when
//! the host picks it up, then `<id>.response.json` (exit code, stdout, stderr). `closed` marks a
//! host that stopped serving. The host only runs `eleven` subcommands, in the run folder or a
//! folder inside it; the credit cap is still enforced by the helper it runs.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::paths;

/// The relay folder inside a run's logs folder.
pub const DIR: &str = "relay";
/// Set on the command the host runs, so it doesn't relay again.
pub const RELAYED_ENV: &str = "VIDEO_STUDIO_RELAYED";
const CLOSED: &str = "closed";
const POLL: Duration = Duration::from_millis(40);
/// The longest a helper waits for the host to answer one call.
const MAX_WAIT: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Serialize, Deserialize)]
pub struct RelayRequest {
    /// The helper's arguments after the program name (`eleven music …`).
    pub args: Vec<String>,
    /// Where the helper was run.
    pub cwd: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RelayResponse {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Whether this process runs inside Codex's sandbox, which marks the commands it runs.
pub fn sandboxed() -> bool {
    let set = |k: &str| std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false);
    (set("CODEX_SANDBOX") || set("CODEX_SANDBOX_NETWORK_DISABLED")) && !set(RELAYED_ENV)
}

/// Helper side: hands `args` to the host and waits for its answer. Prints what the host's
/// command printed and returns its exit code.
pub fn forward(dir: &Utf8Path, args: &[String]) -> i32 {
    match exchange(dir, args, MAX_WAIT) {
        Ok(r) => {
            print!("{}", r.stdout);
            eprint!("{}", r.stderr);
            r.code
        }
        Err(e) => {
            eprintln!("error: {e}");
            crate::run::exit_code::ENVIRONMENT
        }
    }
}

fn exchange(dir: &Utf8Path, args: &[String], max_wait: Duration) -> Result<RelayResponse, String> {
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let id = format!("{}-{nanos}", std::process::id());
    let request = dir.join(format!("{id}.request.json"));
    let response = dir.join(format!("{id}.response.json"));
    paths::write_json_atomic(&request, &RelayRequest { args: args.to_vec(), cwd })
        .map_err(|e| format!("Couldn't hand the ElevenLabs call to Syncfusion Video Studio ({e})."))?;
    let started = Instant::now();
    loop {
        if response.is_file() {
            let r: RelayResponse = paths::read_json(&response).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&response);
            return Ok(r);
        }
        if dir.join(CLOSED).exists() && request.exists() {
            let _ = std::fs::remove_file(&request);
            return Err("The run has ended, so Syncfusion Video Studio didn't make this ElevenLabs call.".into());
        }
        if started.elapsed() > max_wait {
            let _ = std::fs::remove_file(&request);
            return Err(format!(
                "Syncfusion Video Studio didn't answer this ElevenLabs call within {} minutes.",
                max_wait.as_secs() / 60
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// Host side: serves a run's relay folder until stopped.
pub struct HostRelay {
    stop: Arc<AtomicBool>,
    dir: Utf8PathBuf,
    thread: Option<JoinHandle<()>>,
}

/// What the host needs to run a relayed call.
#[derive(Debug, Clone)]
pub struct HostSetup {
    pub dir: Utf8PathBuf,
    /// The `video-studio` binary.
    pub helper: Utf8PathBuf,
    /// The run context file (`VIDEO_STUDIO_RUN_CONTEXT`).
    pub context_file: Utf8PathBuf,
    pub run_folder: Utf8PathBuf,
}

impl HostRelay {
    pub fn start(setup: HostSetup) -> HostRelay {
        let _ = std::fs::create_dir_all(&setup.dir);
        let _ = std::fs::remove_file(setup.dir.join(CLOSED));
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let dir = setup.dir.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                serve_once(&setup);
                std::thread::sleep(POLL);
            }
        });
        HostRelay { stop, dir, thread: Some(thread) }
    }

    /// Stops taking calls. A call already taken still gets its answer.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::write(self.dir.join(CLOSED), b"");
    }
}

fn serve_once(setup: &HostSetup) {
    let Ok(rd) = std::fs::read_dir(&setup.dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".request.json") else { continue };
        if id.starts_with('.') {
            continue;
        }
        let taken = setup.dir.join(format!("{id}.taken"));
        if std::fs::rename(setup.dir.join(&name), &taken).is_err() {
            continue;
        }
        let setup = setup.clone();
        let id = id.to_string();
        std::thread::spawn(move || {
            let response = match paths::read_json::<RelayRequest>(&taken) {
                Ok(req) => run(&setup, &req),
                Err(e) => refuse(format!("The ElevenLabs call couldn't be read ({e}).")),
            };
            let _ = paths::write_json_atomic(&setup.dir.join(format!("{id}.response.json")), &response);
            let _ = std::fs::remove_file(&taken);
        });
    }
}

fn refuse(message: String) -> RelayResponse {
    RelayResponse { code: 2, stdout: String::new(), stderr: format!("error: {message}\n") }
}

/// Runs one relayed call: only `video-studio eleven …`, in the run folder or a folder inside it.
pub fn run(setup: &HostSetup, req: &RelayRequest) -> RelayResponse {
    if req.args.first().map(String::as_str) != Some("eleven") {
        return refuse("Only `video-studio eleven` calls are relayed.".into());
    }
    let cwd = paths::normalize(Utf8Path::new(&req.cwd));
    let cwd = if !req.cwd.is_empty() && cwd.is_absolute() && paths::is_inside(&cwd, &setup.run_folder) && cwd.is_dir() {
        cwd
    } else {
        setup.run_folder.clone()
    };
    let out = Command::new(&setup.helper)
        .args(&req.args)
        .current_dir(&cwd)
        .env(crate::eleven::CONTEXT_ENV, &setup.context_file)
        .env(RELAYED_ENV, "1")
        .env_remove("CODEX_SANDBOX")
        .env_remove("CODEX_SANDBOX_NETWORK_DISABLED")
        .stdin(Stdio::null())
        .output();
    match out {
        Ok(o) => RelayResponse {
            code: o.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        Err(e) => refuse(format!("Syncfusion Video Studio couldn't run the ElevenLabs helper ({e}).")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_relay_answers_at_once_and_other_commands_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(dir.path()).unwrap().to_owned();
        std::fs::write(dir.join(CLOSED), b"").unwrap();
        let err = exchange(&dir, &["eleven".into(), "music".into()], Duration::from_secs(5)).unwrap_err();
        assert!(err.contains("The run has ended"), "{err}");

        let setup = HostSetup {
            dir: dir.clone(),
            helper: Utf8PathBuf::from("/nonexistent/video-studio"),
            context_file: dir.join("context.json"),
            run_folder: dir.clone(),
        };
        let r = run(&setup, &RelayRequest { args: vec!["secret".into(), "status".into()], cwd: dir.to_string() });
        assert_eq!(r.code, 2);
        assert!(r.stderr.contains("Only `video-studio eleven`"), "{}", r.stderr);
    }
}
