//! One `claude -p` child process speaking stream-json on stdin and stdout.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};

use super::stream::{self, StreamEvent};
use crate::platform::ProcessGroup;
use crate::{Error, Result};

#[derive(Debug, Clone, Default)]
pub struct ClaudeOptions {
    pub executable: Utf8PathBuf,
    pub working_dir: Utf8PathBuf,
    pub env: HashMap<String, String>,
    /// CLI alias or full model ID; empty means the CLI's default.
    pub model: String,
    pub effort: Option<String>,
    pub tools: Vec<String>,
    pub resume_session: Option<String>,
    pub system_prompt_append: String,
    /// `--settings` JSON (the PreToolUse hook).
    pub settings: Option<Value>,
    /// Extra folders the CLI's file tools may reach without asking.
    pub add_dirs: Vec<Utf8PathBuf>,
    pub max_budget_usd: Option<f64>,
    /// Raw protocol log (`claude.jsonl`).
    pub log: Option<Utf8PathBuf>,
}

/// Windows caps a whole command line at 32,767 characters, and a run's instructions alone can be
/// longer. Past this many characters they go to Claude Code in a file instead
/// (`--append-system-prompt-file`, verified with Claude Code 2.1.319). Other systems allow far
/// longer command lines and keep passing them inline.
const INLINE_PROMPT_LIMIT: usize = 8_000;

/// Writes a system prompt for `--append-system-prompt-file` to a file of its own under the
/// temporary folder.
fn write_prompt_file(text: &str) -> Result<Utf8PathBuf> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .map_err(|p| Error::Environment(format!("the temporary folder isn't valid UTF-8: {}", p.display())))?
        .join("video-studio-prompts");
    std::fs::create_dir_all(&dir)?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let file = dir.join(format!("{}-{nanos}-{}.md", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    std::fs::write(&file, text)?;
    Ok(file)
}

impl ClaudeOptions {
    /// True when the appended system prompt goes in a file rather than on the command line.
    pub fn prompt_in_file(&self) -> bool {
        cfg!(windows) && self.system_prompt_append.len() > INLINE_PROMPT_LIMIT
    }

    /// The CLI arguments. Verified against Claude Code 2.1.311 (docs/decisions.md).
    pub fn arguments(&self) -> Vec<String> {
        self.arguments_with(None)
    }

    /// The CLI arguments, with the appended system prompt read from `prompt_file` when given.
    pub fn arguments_with(&self, prompt_file: Option<&Utf8Path>) -> Vec<String> {
        let mut a: Vec<String> = [
            "-p",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            "default",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        // Only the app's tools: no MCP servers and no slash commands or skills from the user's setup.
        a.extend(["--strict-mcp-config".to_string(), "--disable-slash-commands".to_string()]);
        a.extend(["--tools".to_string(), self.tools.join(",")]);
        if !self.tools.iter().any(|t| t == "Bash") {
            a.extend(["--disallowed-tools".to_string(), "Bash".to_string()]);
        }
        if let Some(s) = &self.settings {
            a.extend(["--settings".to_string(), s.to_string()]);
        }
        if let Some(f) = prompt_file {
            a.extend(["--append-system-prompt-file".to_string(), f.to_string()]);
        } else if !self.system_prompt_append.is_empty() {
            a.extend(["--append-system-prompt".to_string(), self.system_prompt_append.clone()]);
        }
        let model = self.model.trim();
        if !model.is_empty() && model != "default" {
            a.extend(["--model".to_string(), model.to_string()]);
        }
        if let Some(e) = self.effort.as_deref().map(str::trim).filter(|e| !e.is_empty() && *e != "default") {
            a.extend(["--effort".to_string(), e.to_string()]);
        }
        if let Some(b) = self.max_budget_usd.filter(|b| *b > 0.0) {
            a.extend(["--max-budget-usd".to_string(), format!("{b}")]);
        }
        if let Some(s) = &self.resume_session {
            a.extend(["--resume".to_string(), s.clone()]);
        }
        for d in &self.add_dirs {
            a.extend(["--add-dir".to_string(), d.to_string()]);
        }
        a
    }
}

#[derive(Debug)]
pub enum Incoming {
    Event(StreamEvent),
    /// The process ended: exit code (−1 when killed by a signal) and what it wrote to stderr.
    Exit {
        code: i32,
        stderr: String,
    },
}

struct Shared {
    stdin: Mutex<Option<ChildStdin>>,
    log: Mutex<Option<std::fs::File>>,
    child: Mutex<Child>,
    exited: AtomicBool,
    counter: AtomicU64,
}

impl Shared {
    fn log_line(&self, line: &str) {
        if let Some(f) = self.log.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// A running Claude Code process. Cloning shares the same process.
#[derive(Clone)]
pub struct ClaudeProcess {
    shared: Arc<Shared>,
}

impl ClaudeProcess {
    /// Starts the CLI, sends the initialize request and the first user message, and returns the
    /// process with the channel its events arrive on. The last message is always `Exit`.
    pub fn start(options: &ClaudeOptions, prompt: &str) -> Result<(ClaudeProcess, Receiver<Incoming>)> {
        // Removed when the process ends (below), or now if it doesn't start.
        let prompt_file =
            if options.prompt_in_file() { Some(write_prompt_file(&options.system_prompt_append)?) } else { None };
        let args = options.arguments_with(prompt_file.as_deref());
        let remove_prompt_file = move || {
            if let Some(f) = &prompt_file {
                let _ = std::fs::remove_file(f);
            }
        };
        let mut cmd = crate::platform::command(&options.executable);
        cmd.args(&args).current_dir(options.working_dir.as_str()).env_clear().envs(&options.env);
        // The CLI takes its working directory from $PWD; an app launched from Finder has PWD=/.
        cmd.env("PWD", options.working_dir.as_str()).env_remove("OLDPWD");
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match ProcessGroup::spawn(&mut cmd) {
            Ok(c) => c,
            Err(e) => {
                remove_prompt_file();
                return Err(Error::Environment(format!("couldn't start Claude Code ({}): {e}", options.executable)));
            }
        };
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let log = match &options.log {
            Some(p) => {
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                Some(std::fs::OpenOptions::new().create(true).append(true).open(p)?)
            }
            None => None,
        };
        let shared = Arc::new(Shared {
            stdin: Mutex::new(stdin),
            log: Mutex::new(log),
            child: Mutex::new(child),
            exited: AtomicBool::new(false),
            counter: AtomicU64::new(0),
        });
        shared.log_line(&json!({"type": "host_start", "ts": crate::timeutil::now_rfc3339(), "argv": args, "cwd": options.working_dir}).to_string());
        let (tx, rx): (Sender<Incoming>, Receiver<Incoming>) = channel();

        let err_text = Arc::new(Mutex::new(String::new()));
        let err_shared = shared.clone();
        let err_sink = err_text.clone();
        let err_thread = std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
                err_shared.log_line(&json!({"type": "stderr", "text": line}).to_string());
                let mut t = err_sink.lock().unwrap();
                if t.len() < 16_000 {
                    t.push_str(&line);
                    t.push('\n');
                }
            }
        });
        let out_shared = shared.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim_end_matches(['\n', '\r']);
                if line.is_empty() {
                    continue;
                }
                let events = stream::parse_line(line);
                if events.is_empty() && serde_json::from_str::<Value>(line).is_err() {
                    out_shared.log_line(&json!({"type": "raw_text", "text": line}).to_string());
                } else {
                    out_shared.log_line(line);
                }
                for e in events {
                    if tx.send(Incoming::Event(e)).is_err() {
                        break;
                    }
                }
            }
            let _ = err_thread.join();
            // Reap the child; `try_wait` in a loop so `kill` can take the lock in between.
            let code = loop {
                let status = out_shared.child.lock().unwrap().try_wait();
                match status {
                    Ok(Some(s)) => break s.code().unwrap_or(-1),
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => break -1,
                }
            };
            // On Windows this closes the tree's job: nothing Claude started outlives it.
            ProcessGroup::release(out_shared.child.lock().unwrap().id());
            out_shared.exited.store(true, Ordering::SeqCst);
            remove_prompt_file();
            let stderr = err_text.lock().unwrap().clone();
            out_shared.log_line(
                &json!({"type": "host_exit", "ts": crate::timeutil::now_rfc3339(), "code": code}).to_string(),
            );
            let _ = tx.send(Incoming::Exit { code, stderr });
        });

        let p = ClaudeProcess { shared };
        p.send(&stream::initialize_request(&p.next_id()));
        p.send(&stream::user_message(prompt));
        Ok((p, rx))
    }

    fn next_id(&self) -> String {
        format!("app-{}", self.shared.counter.fetch_add(1, Ordering::SeqCst) + 1)
    }

    /// Writes one protocol line to the CLI. Does nothing once input is closed.
    pub fn send(&self, line: &str) {
        let mut guard = self.shared.stdin.lock().unwrap();
        if let Some(stdin) = guard.as_mut() {
            self.shared.log_line(line);
            if stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
                .is_err()
            {
                *guard = None;
            }
        }
    }

    pub fn allow(&self, request_id: &str, input: &Value) {
        self.send(&stream::allow(request_id, input));
    }

    pub fn deny(&self, request_id: &str, message: &str) {
        self.send(&stream::deny(request_id, message));
    }

    /// Sends a follow-up user message in the same session.
    pub fn send_user_message(&self, text: &str) {
        self.send(&stream::user_message(text));
    }

    /// Records a host-side note in the protocol log (broker decisions, cancellation).
    pub fn log_note(&self, note: Value) {
        self.shared.log_line(&note.to_string());
    }

    /// Closes stdin so the process exits after the current turn.
    pub fn finish_input(&self) {
        *self.shared.stdin.lock().unwrap() = None;
    }

    /// Asks Claude to stop the current turn.
    pub fn interrupt(&self) {
        self.send(&stream::interrupt_request(&self.next_id()));
    }

    pub fn is_running(&self) -> bool {
        !self.shared.exited.load(Ordering::SeqCst)
    }

    pub fn pid(&self) -> u32 {
        self.shared.child.lock().unwrap().id()
    }

    /// Kills the whole process tree now.
    pub fn kill(&self) {
        if self.is_running() {
            let mut child = self.shared.child.lock().unwrap();
            ProcessGroup::kill(&mut child);
        }
    }

    /// Graceful stop (spec §12.4): interrupt, close input, wait up to `grace`, then kill the tree.
    /// Returns true when the process had to be killed.
    pub fn stop(&self, grace: Duration) -> bool {
        self.log_note(
            json!({"type": "host_stop", "ts": crate::timeutil::now_rfc3339(), "grace_ms": grace.as_millis() as u64}),
        );
        self.interrupt();
        self.finish_input();
        let started = std::time::Instant::now();
        let deadline = started + grace;
        // Windows (ui/spec.md §23.4): after the interrupt, Ctrl-Break goes to the whole process
        // group, so cutline and ffmpeg get the chance to stop cleanly before the job is ended.
        #[cfg(windows)]
        let mut broke = false;
        while self.is_running() && std::time::Instant::now() < deadline {
            #[cfg(windows)]
            if !broke && started.elapsed() >= grace / 2 {
                broke = true;
                ProcessGroup::terminate(&self.shared.child.lock().unwrap());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if self.is_running() {
            self.kill();
            true
        } else {
            // Claude exited; make sure nothing it started (cutline, ffmpeg, Chrome) outlives it.
            ProcessGroup::kill_pid(self.pid());
            false
        }
    }
}

/// Turns a CLI failure into something the user can act on.
pub fn friendly_error(text: &str) -> String {
    let lower = text.to_lowercase();
    if lower.contains("not logged in")
        || lower.contains("/login")
        || lower.contains("invalid api key")
        || lower.contains("authentication")
        || lower.contains("please run `claude`")
    {
        return format!(
            "Claude isn't signed in. Open Terminal, run `claude` and sign in, then try again.\n\n{}",
            text.trim()
        );
    }
    text.trim().to_string()
}

/// The folder a path's file tools need: the file's parent, or the folder itself.
pub fn containing_dir(p: &Utf8Path) -> Utf8PathBuf {
    if p.is_dir() {
        p.to_owned()
    } else {
        p.parent().map(|d| d.to_owned()).unwrap_or_else(|| p.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_arguments() {
        let mut o = ClaudeOptions {
            model: "claude-opus-5-5".into(),
            tools: vec!["Read".into(), "Edit".into(), "AskUserQuestion".into()],
            system_prompt_append: "ctx".into(),
            ..Default::default()
        };
        o.effort = Some("high".into());
        o.resume_session = Some("sess-1".into());
        o.max_budget_usd = Some(12.5);
        o.add_dirs = vec!["/footage".into()];
        o.settings = Some(json!({"hooks": {}}));
        let a = o.arguments();
        let has = |flag: &str, value: &str| a.windows(2).any(|w| w[0] == flag && w[1] == value);
        assert!(has("--output-format", "stream-json") && has("--input-format", "stream-json"));
        assert!(has("--permission-prompt-tool", "stdio"));
        assert!(has("--tools", "Read,Edit,AskUserQuestion"));
        assert!(has("--disallowed-tools", "Bash"), "no Bash for the assistant");
        assert!(has("--model", "claude-opus-5-5") && has("--effort", "high") && has("--resume", "sess-1"));
        assert!(has("--max-budget-usd", "12.5") && has("--add-dir", "/footage") && has("--settings", "{\"hooks\":{}}"));
        assert!(a.contains(&"--strict-mcp-config".to_string()));
        let mut p = o.clone();
        p.tools.push("Bash".into());
        p.model = "default".into();
        p.effort = Some(" ".into());
        let a = p.arguments();
        assert!(
            !a.contains(&"--disallowed-tools".to_string())
                && !a.contains(&"--model".to_string())
                && !a.contains(&"--effort".to_string())
        );
        // a long system prompt goes in a file on Windows, where a command line is capped
        assert!(!o.prompt_in_file() && has("--append-system-prompt", "ctx"));
        let long = ClaudeOptions { system_prompt_append: "x".repeat(40_000), ..Default::default() };
        assert_eq!(long.prompt_in_file(), cfg!(windows));
        let a = long.arguments_with(Some(Utf8Path::new("/tmp/p.md")));
        assert!(a.windows(2).any(|w| w[0] == "--append-system-prompt-file" && w[1] == "/tmp/p.md"));
        assert!(!a.contains(&"--append-system-prompt".to_string()) && a.iter().all(|x| x.len() < 1000));
        let f = write_prompt_file("long instructions").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "long instructions");
        std::fs::remove_file(f).unwrap();
    }

    #[test]
    fn friendly_errors() {
        assert!(friendly_error("Error: Not logged in · Please run /login").starts_with("Claude isn't signed in"));
        assert_eq!(friendly_error("  boom \n"), "boom");
    }
}
