//! One Codex turn driven like a Claude Code process (FEAT-spry-alpaca-5RQZ).
//!
//! [`CodexProcess`] starts `codex app-server`, completes `initialize` / `initialized`, checks the
//! account, starts or resumes a thread and starts one turn. What the server sends is translated
//! into the same [`StreamEvent`]s the Claude Code client produces, so the assistant and the run
//! orchestrator follow a Codex turn with the code they already have:
//!
//! - `item/agentMessage/delta` and completed agent messages → text;
//! - command executions, file changes and web searches → tool calls and their results;
//! - approval requests (commands, file changes, network access, extra permissions, MCP
//!   elicitations, dynamic tools) → permission requests that go through the broker's
//!   [`Policy`](crate::claude::broker::Policy); the host's answer becomes the protocol answer
//!   (`accept` / `decline`, an empty permission grant, a declined elicitation, a failed tool
//!   call), always with the server request's own ID;
//! - `item/tool/requestUserInput` → an AskUserQuestion-style question; the answers go back keyed
//!   by Codex's question IDs, with the request's own ID;
//! - `thread/tokenUsage/updated` → usage; `turn/completed` → the result.
//!
//! Message shapes follow `codex app-server generate-ts` / `generate-json-schema` of codex-cli
//! 0.171.0.
//!
//! The sandbox keeps Codex inside the working folder: `workspace-write` with only that folder
//! writable (no `$TMPDIR`, no `/tmp`), network access only when web access is on, approvals
//! routed to this client (`approvalsReviewer: user`) under the `untrusted` policy so every
//! command outside Codex's read-only safe list asks first, MCP servers and web search turned
//! off unless allowed.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};

use super::METHOD_NOT_FOUND;
use crate::claude::process::Incoming;
use crate::claude::stream::{StreamEvent, Usage};
use crate::platform::ProcessGroup;
use crate::{Error, Result};

/// How a session token names a Codex thread. Claude session IDs never start with it, so a token
/// from one provider is never resumed by the other.
pub const TOKEN_PREFIX: &str = "codex:";

/// The thread in a `codex:<thread>` session token; `None` for anything else (a Claude session).
pub fn thread_of(token: &str) -> Option<&str> {
    token.strip_prefix(TOKEN_PREFIX).filter(|t| !t.is_empty())
}

/// The session token for a Codex thread.
pub fn token_for(thread: &str) -> String {
    format!("{TOKEN_PREFIX}{thread}")
}

/// Known gap (FEAT-spry-alpaca-5RQZ): under the `untrusted` approval policy, Codex runs the
/// commands it classes as safe and read-only without asking, so the broker's command allowlist
/// and read limits don't apply to them. The sandbox still blocks writes outside the working
/// folder. Every Codex start records this in its trace (`codex.jsonl`).
pub const READS_UNCHECKED_WARNING: &str = "Codex runs commands it considers safe and read-only without asking, so Video Studio's command allowlist and read limits don't apply to them. It still can't write outside its working folder.";

/// What a Codex turn needs.
#[derive(Debug, Clone, Default)]
pub struct CodexOptions {
    pub executable: Utf8PathBuf,
    /// The thread's working directory: the run folder or the assistant's session folder.
    pub working_dir: Utf8PathBuf,
    /// The app server's environment.
    pub env: HashMap<String, String>,
    /// Variables the shell tool must see whatever Codex's own policy inherits (the run's PATH
    /// with the cutline proxy first, the helper's context).
    pub shell_env: BTreeMap<String, String>,
    /// `None`: Codex's default model.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The app's instructions (production or assistant), as developer instructions.
    pub developer_instructions: String,
    /// A thread to resume. When resuming fails, the client says so ([`Incoming::ContextLost`])
    /// and starts exactly one new thread.
    pub resume_thread: Option<String>,
    /// The only folders Codex may write; the working folder when empty.
    pub writable_roots: Vec<Utf8PathBuf>,
    pub web_access: bool,
    /// Raw protocol log (`codex.jsonl`).
    pub log: Option<Utf8PathBuf>,
}

/// Effort levels Codex takes; the app's `max` is Codex's `xhigh`.
pub fn codex_effort(effort: Option<&str>) -> Option<String> {
    match effort.map(str::trim) {
        Some(e @ ("minimal" | "low" | "medium" | "high" | "xhigh")) => Some(e.to_string()),
        Some("max") => Some("xhigh".into()),
        _ => None,
    }
}

impl CodexOptions {
    fn roots(&self) -> Vec<String> {
        let mut r: Vec<String> = self.writable_roots.iter().map(|p| p.to_string()).collect();
        if r.is_empty() {
            r.push(self.working_dir.to_string());
        }
        r
    }

    /// `config` overrides for the thread: no MCP servers, web search only with web access, the
    /// sandbox's writable roots, and the shell environment.
    pub fn thread_config(&self) -> Value {
        let mut shell = json!({"inherit": "all"});
        if !self.shell_env.is_empty() {
            shell["set"] = json!(self.shell_env);
        }
        json!({
            "mcp_servers": {},
            "web_search": if self.web_access { "live" } else { "disabled" },
            "sandbox_workspace_write": {
                "writable_roots": self.roots(),
                "network_access": self.web_access,
                "exclude_tmpdir_env_var": true,
                "exclude_slash_tmp": true,
            },
            "shell_environment_policy": shell,
        })
    }

    /// The sandbox policy for `turn/start`.
    pub fn sandbox_policy(&self) -> Value {
        json!({
            "type": "workspaceWrite",
            "writableRoots": self.roots(),
            "networkAccess": self.web_access,
            "excludeTmpdirEnvVar": true,
            "excludeSlashTmp": true,
        })
    }

    /// `thread/start` (or, with `thread`, `thread/resume`) parameters.
    pub fn thread_params(&self, thread: Option<&str>) -> Value {
        let mut p = json!({
            "cwd": self.working_dir,
            "approvalPolicy": "untrusted",
            "approvalsReviewer": "user",
            "sandbox": "workspace-write",
            "config": self.thread_config(),
            "developerInstructions": self.developer_instructions,
        });
        if let Some(m) = &self.model {
            p["model"] = json!(m);
        }
        if let Some(t) = thread {
            p["threadId"] = json!(t);
        }
        p
    }

    /// `turn/start` parameters.
    pub fn turn_params(&self, thread: &str, prompt: &str) -> Value {
        let mut p = json!({
            "threadId": thread,
            "input": [{"type": "text", "text": prompt, "text_elements": []}],
            "cwd": self.working_dir,
            "approvalPolicy": "untrusted",
            "approvalsReviewer": "user",
            "sandboxPolicy": self.sandbox_policy(),
        });
        if let Some(m) = &self.model {
            p["model"] = json!(m);
        }
        if let Some(e) = &self.effort {
            p["effort"] = json!(e);
        }
        p
    }
}

/// What the host told the driver to do.
enum Cmd {
    Allow(String, Value),
    Deny(String, String),
    Interrupt,
}

/// A server request waiting for the host.
enum Pending {
    Command {
        id: Value,
        v1: bool,
        item: Option<String>,
    },
    Question {
        id: Value,
        ids: HashMap<String, String>,
    },
    /// A file change checked one path at a time: answered when every path is decided.
    Files {
        id: Value,
        v1: bool,
        left: usize,
        denied: Option<String>,
        item: Option<String>,
    },
    Permissions {
        id: Value,
    },
    Elicitation {
        id: Value,
    },
    DynamicTool {
        id: Value,
    },
}

struct Shared {
    child: Mutex<Child>,
    exited: AtomicBool,
    log: Mutex<Option<std::fs::File>>,
    cmds: Mutex<Sender<Cmd>>,
}

impl Shared {
    fn log_line(&self, line: &str) {
        if let Some(f) = self.log.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// A running Codex turn. Cloning shares it.
#[derive(Clone)]
pub struct CodexProcess {
    shared: Arc<Shared>,
}

impl CodexProcess {
    /// Starts the app server and the turn. Events arrive on the receiver; the last one is always
    /// `Exit`.
    pub fn start(options: &CodexOptions, prompt: &str) -> Result<(CodexProcess, Receiver<Incoming>)> {
        let mut cmd = crate::platform::command(&options.executable);
        cmd.arg("app-server").current_dir(options.working_dir.as_str()).env_clear().envs(&options.env);
        cmd.env("PWD", options.working_dir.as_str()).env_remove("OLDPWD");
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = ProcessGroup::spawn(&mut cmd)
            .map_err(|e| Error::Environment(format!("couldn't start Codex ({}): {e}", options.executable)))?;
        let stdin = child.stdin.take().ok_or_else(|| Error::other("no stdin for Codex"))?;
        let stdout = child.stdout.take().ok_or_else(|| Error::other("no stdout for Codex"))?;
        let stderr = child.stderr.take().ok_or_else(|| Error::other("no stderr for Codex"))?;
        let log = match &options.log {
            Some(p) => {
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                Some(std::fs::OpenOptions::new().create(true).append(true).open(p)?)
            }
            None => None,
        };
        let (cmd_tx, cmd_rx) = channel();
        let shared = Arc::new(Shared {
            child: Mutex::new(child),
            exited: AtomicBool::new(false),
            log: Mutex::new(log),
            cmds: Mutex::new(cmd_tx),
        });
        shared.log_line(&json!({"type": "host_start", "ts": crate::timeutil::now_rfc3339(), "argv": [options.executable.as_str(), "app-server"], "cwd": options.working_dir}).to_string());
        shared.log_line(
            &json!({"type": "host_warning", "code": "codex_reads_unchecked", "text": READS_UNCHECKED_WARNING})
                .to_string(),
        );

        let err_text = Arc::new(Mutex::new(String::new()));
        let err_sink = err_text.clone();
        let err_shared = shared.clone();
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
        let (line_tx, line_rx) = channel::<String>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let line = String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_string();
                if !line.is_empty() && line_tx.send(line).is_err() {
                    break;
                }
            }
        });

        let (tx, rx) = channel();
        let mut driver = Driver {
            shared: shared.clone(),
            stdin: Some(stdin),
            lines: line_rx,
            cmds: cmd_rx,
            tx,
            options: options.clone(),
            next_id: 1,
            thread: None,
            turn: None,
            pending: HashMap::new(),
            file_items: HashMap::new(),
            command_items: HashMap::new(),
            declined_items: HashMap::new(),
            usage_updates: 0,
            total_usage: None,
            last_error: None,
            interrupted: false,
            disconnected: false,
        };
        let prompt = prompt.to_string();
        let drv_shared = shared.clone();
        std::thread::Builder::new().name("video-studio-codex".into()).spawn(move || {
            driver.run(&prompt);
            // Input closed: the server exits on its own; whatever is left is killed.
            drop(driver.stdin.take());
            let deadline = Instant::now() + Duration::from_secs(3);
            let code = loop {
                let status = drv_shared.child.lock().unwrap().try_wait();
                match status {
                    Ok(Some(s)) => break s.code().unwrap_or(-1),
                    Ok(None) if Instant::now() >= deadline => {
                        ProcessGroup::kill(&mut drv_shared.child.lock().unwrap());
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => break -1,
                }
            };
            // Nothing Codex started (cutline, ffmpeg) outlives the turn.
            let pid = drv_shared.child.lock().unwrap().id();
            ProcessGroup::kill_pid(pid);
            ProcessGroup::release(pid);
            let _ = err_thread.join();
            drv_shared.exited.store(true, Ordering::SeqCst);
            drv_shared.log_line(
                &json!({"type": "host_exit", "ts": crate::timeutil::now_rfc3339(), "code": code}).to_string(),
            );
            let stderr = err_text.lock().unwrap().clone();
            let _ = driver.tx.send(Incoming::Exit { code, stderr });
        })?;
        Ok((CodexProcess { shared }, rx))
    }

    fn command(&self, c: Cmd) {
        let _ = self.shared.cmds.lock().unwrap().send(c);
    }

    /// Accepts a permission request, or answers a question (`input["answers"]`: question text →
    /// labels, comma-joined).
    pub fn allow(&self, request_id: &str, input: &Value) {
        self.command(Cmd::Allow(request_id.to_string(), input.clone()));
    }

    /// Declines a permission request, or leaves a question unanswered.
    pub fn deny(&self, request_id: &str, message: &str) {
        self.command(Cmd::Deny(request_id.to_string(), message.to_string()));
    }

    /// Records a host-side note in the protocol log.
    pub fn log_note(&self, note: Value) {
        self.shared.log_line(&note.to_string());
    }

    /// One turn per process: the server is closed once the turn completes.
    pub fn finish_input(&self) {}

    /// Sends `turn/interrupt` for the active turn (or ends the session before one started).
    pub fn interrupt(&self) {
        self.command(Cmd::Interrupt);
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
            ProcessGroup::kill(&mut self.shared.child.lock().unwrap());
        }
    }

    /// Stop: `turn/interrupt`, wait up to `grace` for the turn to end, then kill the tree.
    /// Returns true when it had to be killed.
    pub fn stop(&self, grace: Duration) -> bool {
        self.log_note(
            json!({"type": "host_stop", "ts": crate::timeutil::now_rfc3339(), "grace_ms": grace.as_millis() as u64}),
        );
        self.interrupt();
        let deadline = Instant::now() + grace;
        while self.is_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        if self.is_running() {
            self.kill();
            true
        } else {
            ProcessGroup::kill_pid(self.pid());
            false
        }
    }
}

/// Why the driver stopped waiting.
enum Halt {
    Stopped,
    Failed(String),
}

struct Driver {
    shared: Arc<Shared>,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    cmds: Receiver<Cmd>,
    tx: Sender<Incoming>,
    options: CodexOptions,
    next_id: i64,
    thread: Option<String>,
    turn: Option<String>,
    pending: HashMap<String, Pending>,
    /// File-change items by ID: the paths they touch (approval requests only carry the item ID).
    file_items: HashMap<String, Vec<String>>,
    /// Command items by ID: the command, for approvals that leave it out.
    command_items: HashMap<String, String>,
    /// Items the host declined: their completion isn't reported as a failed tool call (the host
    /// already said why it was blocked).
    declined_items: HashMap<String, ()>,
    usage_updates: u64,
    total_usage: Option<Usage>,
    last_error: Option<String>,
    interrupted: bool,
    disconnected: bool,
}

/// The key a server request ID is known by on the host side.
fn key_of(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A command with a shell wrapper (`bash -lc '…'`, `zsh -c …`, `cmd /c …`) as the script it runs.
pub fn unwrap_shell(command: &str) -> String {
    let Ok(segments) = crate::claude::broker::split_commands(command) else { return command.to_string() };
    if let [argv] = segments.as_slice() {
        if argv.len() == 3 {
            let exe = Utf8Path::new(&argv[0]).file_stem().unwrap_or("").to_ascii_lowercase();
            let flag = argv[1].to_ascii_lowercase();
            let shells = ["bash", "sh", "zsh", "dash", "fish", "pwsh", "powershell", "cmd"];
            if shells.contains(&exe.as_str()) && ["-lc", "-c", "/c", "-command"].contains(&flag.as_str()) {
                return argv[2].clone();
            }
        }
    }
    command.to_string()
}

fn usage_of(u: &Value) -> Usage {
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    let cached = n("cachedInputTokens");
    Usage {
        input_tokens: n("inputTokens").saturating_sub(cached),
        output_tokens: n("outputTokens"),
        cache_read_tokens: cached,
        cache_creation_tokens: n("cacheWriteInputTokens"),
        ..Usage::default()
    }
}

impl Driver {
    fn event(&self, e: StreamEvent) {
        let _ = self.tx.send(Incoming::Event(e));
    }

    fn send(&mut self, v: &Value) {
        let line = v.to_string();
        self.shared.log_line(&line);
        if let Some(stdin) = self.stdin.as_mut() {
            if stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
                .is_err()
            {
                self.stdin = None;
            }
        }
    }

    fn respond(&mut self, id: &Value, result: Value) {
        self.send(&json!({"id": id, "result": result}));
    }

    fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"id": id, "method": method, "params": params}));
        id
    }

    /// Sends a request and waits for its answer, handling everything else meanwhile.
    fn call(&mut self, method: &str, params: Value, timeout: Duration) -> std::result::Result<Value, Halt> {
        let id = self.request(method, params);
        let deadline = Instant::now() + timeout;
        loop {
            if self.process_commands() {
                // nothing to interrupt yet: give up on the session
                self.interrupted = true;
                self.release_pending();
                return Err(Halt::Stopped);
            }
            if Instant::now() >= deadline {
                return Err(Halt::Failed(format!("Codex didn't answer `{method}` in {} s.", timeout.as_secs())));
            }
            match self.lines.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    let Some(msg) = self.parse(&line) else { continue };
                    if msg.get("method").is_none() && msg.get("id") == Some(&json!(id)) {
                        if let Some(e) = msg.get("error") {
                            return Err(Halt::Failed(e["message"].as_str().unwrap_or("no reason given").to_string()));
                        }
                        return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                    }
                    self.dispatch(msg);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.disconnected = true;
                    return Err(Halt::Failed(format!("Codex exited before answering `{method}`.")));
                }
            }
        }
    }

    fn parse(&self, line: &str) -> Option<Value> {
        self.shared.log_line(line);
        serde_json::from_str::<Value>(line).ok().filter(Value::is_object)
    }

    fn fail(&self, text: String) {
        self.event(StreamEvent::Result {
            is_error: true,
            text: Some(text),
            session_id: self.thread.as_deref().map(token_for),
            subtype: Some("error".into()),
            usage: self.total_usage.clone(),
        });
    }

    fn run(&mut self, prompt: &str) {
        let t = Duration::from_secs(60);
        let init =
            json!({"clientInfo": {"name": "video_studio", "title": crate::APP_NAME, "version": crate::CORE_VERSION}});
        match self.call("initialize", init, t) {
            Ok(_) => {}
            Err(Halt::Stopped) => return,
            Err(Halt::Failed(e)) if self.disconnected => {
                return self.shared.log_line(&json!({"type": "host_note", "text": e}).to_string())
            }
            Err(Halt::Failed(e)) => {
                return self.fail(format!(
                    "Codex didn't start its app server: {e} Update the Codex CLI (tested with 0.171.0)."
                ))
            }
        }
        self.send(&json!({"method": "initialized"}));
        let account = match self.call("account/read", json!({}), t) {
            Ok(a) => a,
            Err(Halt::Stopped) => return,
            Err(Halt::Failed(_)) if self.disconnected => return,
            Err(Halt::Failed(e)) => return self.fail(format!("Codex couldn't read its account: {e}")),
        };
        if account.get("account").map(Value::is_null).unwrap_or(true) && account["requiresOpenaiAuth"] != json!(false) {
            return self.fail(
                "Codex isn't signed in. Open Terminal, run `codex login` and sign in, then try again. The app never asks for OpenAI credentials."
                    .into(),
            );
        }
        // The thread: resume the recorded one, or start one. A failed resume is reported and
        // replaced by exactly one new thread.
        let mut started = None;
        if let Some(previous) = self.options.resume_thread.clone() {
            match self.call("thread/resume", self.options.thread_params(Some(&previous)), t) {
                Ok(r) => started = Some(r),
                Err(Halt::Stopped) => return,
                Err(Halt::Failed(_)) if self.disconnected => return,
                Err(Halt::Failed(reason)) => {
                    let _ = self.tx.send(Incoming::ContextLost { previous: token_for(&previous), reason });
                }
            }
        }
        let started = match started {
            Some(r) => r,
            None => match self.call("thread/start", self.options.thread_params(None), t) {
                Ok(r) => r,
                Err(Halt::Stopped) => return,
                Err(Halt::Failed(_)) if self.disconnected => return,
                Err(Halt::Failed(e)) => return self.fail(format!("Codex couldn't start a thread: {e}")),
            },
        };
        let Some(thread) = started["thread"]["id"].as_str().map(str::to_string) else {
            return self.fail("Codex started a thread without an ID.".into());
        };
        self.thread = Some(thread.clone());
        self.event(StreamEvent::Init {
            session_id: token_for(&thread),
            model: started["model"].as_str().map(str::to_string).or(self.options.model.clone()),
            tools: Vec::new(),
            cli_version: None,
        });
        match self.call("turn/start", self.options.turn_params(&thread, prompt), t) {
            Ok(r) => self.turn = r["turn"]["id"].as_str().map(str::to_string),
            Err(Halt::Stopped) => return,
            Err(Halt::Failed(_)) if self.disconnected => return,
            Err(Halt::Failed(e)) => return self.fail(format!("Codex couldn't start the turn: {e}")),
        }
        // Follow the turn until it completes, the host stops it, or the server goes away.
        loop {
            if self.process_commands() && !self.interrupted {
                self.interrupted = true;
                self.send_interrupt();
            }
            match self.lines.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    let Some(msg) = self.parse(&line) else { continue };
                    if msg["method"] == "turn/completed" {
                        let turn = &msg["params"]["turn"];
                        let ours = self.turn.as_deref().map(|t| turn["id"] == t).unwrap_or(true);
                        if ours {
                            self.complete(turn);
                            return;
                        }
                    }
                    self.dispatch(msg);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.disconnected = true;
                    return;
                }
            }
        }
    }

    fn send_interrupt(&mut self) {
        // Pending questions and approvals are released first: nothing waits on the host now.
        self.release_pending();
        if let (Some(thread), Some(turn)) = (self.thread.clone(), self.turn.clone()) {
            self.request("turn/interrupt", json!({"threadId": thread, "turnId": turn}));
        }
    }

    fn complete(&mut self, turn: &Value) {
        let status = turn["status"].as_str().unwrap_or("completed");
        let error = turn["error"]["message"].as_str().map(str::to_string).or(self.last_error.clone());
        let (is_error, subtype) = match status {
            "completed" => (false, "success"),
            "interrupted" => (self.interrupted, "interrupted"),
            _ => (true, "error"),
        };
        self.event(StreamEvent::Result {
            is_error: is_error || status == "failed",
            text: if status == "completed" {
                None
            } else {
                Some(error.unwrap_or_else(|| format!("The Codex turn ended ({status}).")))
            },
            session_id: self.thread.as_deref().map(token_for),
            subtype: Some(subtype.into()),
            usage: self.total_usage.clone(),
        });
    }

    /// Applies the host's answers. Returns true when the host asked to stop.
    fn process_commands(&mut self) -> bool {
        let mut stop = false;
        while let Ok(c) = self.cmds.try_recv() {
            match c {
                Cmd::Allow(key, input) => self.answer(&key, Some(input), ""),
                Cmd::Deny(key, message) => self.answer(&key, None, &message),
                Cmd::Interrupt => stop = true,
            }
        }
        stop
    }

    /// Declines everything still waiting on the host.
    fn release_pending(&mut self) {
        for key in self.pending.keys().cloned().collect::<Vec<_>>() {
            self.answer(&key, None, "The user stopped this turn.");
        }
    }

    /// Answers one server request (or one path of a file change). `allow` carries the host's
    /// input (for questions, the answers).
    fn answer(&mut self, key: &str, allow: Option<Value>, message: &str) {
        // A path of a file change: `<request>#<n>`.
        if let Some((base, _)) = key.rsplit_once('#') {
            if let Some(Pending::Files { left, denied, .. }) = self.pending.get_mut(base) {
                *left = left.saturating_sub(1);
                if allow.is_none() && denied.is_none() {
                    *denied = Some(message.to_string());
                }
                if *left > 0 {
                    return;
                }
                let base = base.to_string();
                return self.answer_now(&base);
            }
        }
        let Some(p) = self.pending.remove(key) else { return };
        let ok = allow.is_some();
        match p {
            Pending::Command { id, v1, item } => {
                if !ok {
                    self.declined(item);
                }
                let decision = match (v1, ok) {
                    (false, true) => json!("accept"),
                    (false, false) => json!("decline"),
                    (true, true) => json!("approved"),
                    (true, false) => json!({"denied": {"rejection": message}}),
                };
                self.respond(&id, json!({"decision": decision}));
            }
            Pending::Question { id, ids } => {
                let mut answers = serde_json::Map::new();
                if let Some(input) = allow {
                    if let Some(given) = input["answers"].as_object() {
                        for (question, answer) in given {
                            let qid = ids.get(question).cloned().unwrap_or_else(|| question.clone());
                            let labels: Vec<String> = answer
                                .as_str()
                                .unwrap_or("")
                                .split(", ")
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(str::to_string)
                                .collect();
                            answers.insert(qid, json!({"answers": labels}));
                        }
                    }
                }
                self.respond(&id, json!({"answers": answers}));
            }
            Pending::Files { id, v1, item, .. } => {
                // reached only when the whole request is answered at once (stop)
                self.declined(item);
                let decision = if v1 { json!({"denied": {"rejection": message}}) } else { json!("decline") };
                self.respond(&id, json!({"decision": decision}));
            }
            // Extra permissions are never granted: an empty grant for this turn.
            Pending::Permissions { id } => self.respond(&id, json!({"permissions": {}, "scope": "turn"})),
            Pending::Elicitation { id } => {
                self.respond(&id, json!({"action": "decline", "content": null, "_meta": null}))
            }
            Pending::DynamicTool { id } => {
                let text = if message.is_empty() { "This tool isn't available here." } else { message };
                self.respond(&id, json!({"contentItems": [{"type": "inputText", "text": text}], "success": false}))
            }
        }
    }

    /// A declined item's completion isn't reported as a failed tool call: the host already said
    /// why it was blocked.
    fn declined(&mut self, item: Option<String>) {
        if let Some(i) = item {
            self.declined_items.insert(i, ());
        }
    }

    fn answer_now(&mut self, key: &str) {
        let Some(Pending::Files { id, v1, denied, item, .. }) = self.pending.remove(key) else { return };
        if denied.is_some() {
            self.declined(item);
        }
        let decision = match (v1, &denied) {
            (false, None) => json!("accept"),
            (false, Some(_)) => json!("decline"),
            (true, None) => json!("approved"),
            (true, Some(m)) => json!({"denied": {"rejection": m}}),
        };
        self.respond(&id, json!({"decision": decision}));
    }

    fn permission(&self, key: &str, tool: &str, input: Value) {
        self.event(StreamEvent::PermissionRequest {
            request_id: key.to_string(),
            tool_name: tool.to_string(),
            input,
            tool_use_id: None,
        });
    }

    fn dispatch(&mut self, msg: Value) {
        let Some(method) = msg["method"].as_str().map(str::to_string) else { return };
        match msg.get("id").cloned() {
            Some(id) => self.server_request(&method, id, &msg["params"]),
            None => self.notification(&method, &msg["params"]),
        }
    }

    fn server_request(&mut self, method: &str, id: Value, params: &Value) {
        let key = key_of(&id);
        let item = params["itemId"].as_str().map(str::to_string);
        match method {
            "item/commandExecution/requestApproval" => {
                if params["networkApprovalContext"].is_object() {
                    let host = params["networkApprovalContext"]["host"].as_str().unwrap_or("the network").to_string();
                    self.pending.insert(key.clone(), Pending::Command { id, v1: false, item: item.clone() });
                    return self.permission(&key, "WebFetch", json!({"url": host}));
                }
                let command = params["command"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| params["itemId"].as_str().and_then(|i| self.command_items.get(i).cloned()));
                let Some(command) = command else {
                    return self.respond(&id, json!({"decision": "decline"}));
                };
                self.pending.insert(key.clone(), Pending::Command { id, v1: false, item: item.clone() });
                self.permission(&key, "Bash", json!({"command": unwrap_shell(&command)}));
            }
            "execCommandApproval" => {
                let argv: Vec<String> = params["command"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                if argv.is_empty() {
                    return self.respond(&id, json!({"decision": {"denied": {"rejection": "No command."}}}));
                }
                let joined = argv.iter().map(|a| crate::claude::broker::shell_quote(a)).collect::<Vec<_>>().join(" ");
                let command = if argv.len() == 3 { unwrap_shell(&joined) } else { joined };
                self.pending.insert(key.clone(), Pending::Command { id, v1: true, item: None });
                self.permission(&key, "Bash", json!({"command": command}));
            }
            "item/fileChange/requestApproval" | "applyPatchApproval" => {
                let v1 = method == "applyPatchApproval";
                let mut paths: Vec<String> = if v1 {
                    params["fileChanges"]
                        .as_object()
                        .map(|m| {
                            let mut p: Vec<String> = m.keys().cloned().collect();
                            p.extend(m.values().filter_map(|c| c["move_path"].as_str().map(str::to_string)));
                            p
                        })
                        .unwrap_or_default()
                } else {
                    params["itemId"].as_str().and_then(|i| self.file_items.get(i).cloned()).unwrap_or_default()
                };
                // asking for write access to a whole folder is checked as a write there
                if let Some(root) = params["grantRoot"].as_str() {
                    paths.push(Utf8Path::new(root).join(".codex-grant-root").to_string());
                }
                if paths.is_empty() {
                    let decision =
                        if v1 { json!({"denied": {"rejection": "No files named."}}) } else { json!("decline") };
                    return self.respond(&id, json!({"decision": decision}));
                }
                self.pending.insert(
                    key.clone(),
                    Pending::Files { id, v1, left: paths.len(), denied: None, item: item.clone() },
                );
                for (i, p) in paths.iter().enumerate() {
                    self.permission(&format!("{key}#{i}"), "Write", json!({"file_path": p}));
                }
            }
            "item/tool/requestUserInput" => {
                let mut ids = HashMap::new();
                let questions: Vec<Value> = params["questions"]
                    .as_array()
                    .map(|qs| {
                        qs.iter()
                            .filter_map(|q| {
                                let text = q["question"].as_str()?.to_string();
                                ids.insert(text.clone(), q["id"].as_str().unwrap_or(&text).to_string());
                                Some(json!({
                                    "question": text,
                                    "header": q["header"],
                                    "options": q["options"].as_array().cloned().unwrap_or_default(),
                                    "multiSelect": false,
                                }))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if questions.is_empty() {
                    return self.respond(&id, json!({"answers": {}}));
                }
                self.pending.insert(key.clone(), Pending::Question { id, ids });
                self.permission(&key, "AskUserQuestion", json!({"questions": questions}));
            }
            // Unsupported permission requests go through the broker, which refuses them, so the
            // refusal is logged and shown like any other.
            "item/permissions/requestApproval" => {
                self.pending.insert(key.clone(), Pending::Permissions { id });
                self.permission(&key, "RequestPermissions", params["permissions"].clone());
            }
            "mcpServer/elicitation/request" => {
                self.pending.insert(key.clone(), Pending::Elicitation { id });
                self.permission(&key, "McpElicitation", params.clone());
            }
            "item/tool/call" => {
                self.pending.insert(key.clone(), Pending::DynamicTool { id });
                let tool = params["tool"].as_str().unwrap_or("tool");
                self.permission(&key, &format!("DynamicTool {tool}"), params["arguments"].clone());
            }
            other => {
                let other = other.to_string();
                self.send(&json!({"id": id, "error": {"code": METHOD_NOT_FOUND, "message": format!("`{other}` isn't supported by this client")}}));
            }
        }
    }

    fn notification(&mut self, method: &str, params: &Value) {
        match method {
            "item/agentMessage/delta" => {
                if let Some(d) = params["delta"].as_str() {
                    self.event(StreamEvent::TextDelta(d.to_string()));
                }
            }
            "item/started" => self.item_started(&params["item"]),
            "item/completed" => self.item_completed(&params["item"]),
            "thread/tokenUsage/updated" => {
                let u = &params["tokenUsage"];
                self.usage_updates += 1;
                self.total_usage = Some(usage_of(&u["total"]));
                self.event(StreamEvent::MessageUsage {
                    message_id: format!("codex-{}", self.usage_updates),
                    usage: usage_of(&u["last"]),
                });
            }
            "turn/started" => {
                if self.turn.is_none() {
                    self.turn = params["turn"]["id"].as_str().map(str::to_string);
                }
            }
            "error" if params["willRetry"] != json!(true) => {
                self.last_error = params["error"]["message"].as_str().map(str::to_string);
            }
            _ => {}
        }
    }

    fn item_started(&mut self, item: &Value) {
        let id = item["id"].as_str().unwrap_or("").to_string();
        match item["type"].as_str().unwrap_or("") {
            "commandExecution" => {
                let command = unwrap_shell(item["command"].as_str().unwrap_or(""));
                self.command_items.insert(id.clone(), command.clone());
                self.event(StreamEvent::ToolUse { id, name: "Bash".into(), input: json!({"command": command}) });
            }
            "fileChange" => {
                let changes = item["changes"].as_array().cloned().unwrap_or_default();
                let mut paths = Vec::new();
                for (i, c) in changes.iter().enumerate() {
                    let Some(path) = c["path"].as_str() else { continue };
                    paths.push(path.to_string());
                    if let Some(to) = c["kind"]["move_path"].as_str() {
                        paths.push(to.to_string());
                    }
                    let name = if c["kind"]["type"] == "add" { "Write" } else { "Edit" };
                    self.event(StreamEvent::ToolUse {
                        id: format!("{id}#{i}"),
                        name: name.into(),
                        input: json!({"file_path": path}),
                    });
                }
                self.file_items.insert(id, paths);
            }
            "webSearch" => self.event(StreamEvent::ToolUse {
                id,
                name: "WebSearch".into(),
                input: json!({"query": item["query"]}),
            }),
            "mcpToolCall" => self.event(StreamEvent::ToolUse {
                id,
                name: format!("{}/{}", item["server"].as_str().unwrap_or("mcp"), item["tool"].as_str().unwrap_or("")),
                input: item["arguments"].clone(),
            }),
            _ => {}
        }
    }

    fn item_completed(&mut self, item: &Value) {
        let id = item["id"].as_str().unwrap_or("").to_string();
        let status = item["status"].as_str().unwrap_or("completed");
        let declined = status == "declined" || self.declined_items.contains_key(&id);
        match item["type"].as_str().unwrap_or("") {
            "agentMessage" => {
                if let Some(t) = item["text"].as_str() {
                    self.event(StreamEvent::AssistantText(t.to_string()));
                }
            }
            "commandExecution" if !declined => {
                let failed = status != "completed" || item["exitCode"].as_i64().map(|c| c != 0).unwrap_or(false);
                let mut content = item["aggregatedOutput"].as_str().unwrap_or("").to_string();
                if failed {
                    if let Some(c) = item["exitCode"].as_i64() {
                        content = format!("Exit code {c}\n{content}");
                    }
                }
                self.event(StreamEvent::ToolResult { tool_use_id: id, content, is_error: failed });
            }
            "fileChange" if !declined => {
                let n = item["changes"].as_array().map(|c| c.len()).unwrap_or(0);
                for i in 0..n {
                    self.event(StreamEvent::ToolResult {
                        tool_use_id: format!("{id}#{i}"),
                        content: String::new(),
                        is_error: status != "completed",
                    });
                }
            }
            "webSearch" | "mcpToolCall" => self.event(StreamEvent::ToolResult {
                tool_use_id: id,
                content: item["error"]["message"].as_str().unwrap_or("").to_string(),
                is_error: status == "failed",
            }),
            _ => {}
        }
    }
}

/// Where the assistant records a session folder's Codex thread: next to the folder, outside
/// what Codex may write.
pub fn thread_record(session_dir: &Utf8Path) -> Utf8PathBuf {
    session_dir.with_file_name(format!("{}.codex-thread.json", session_dir.file_name().unwrap_or("session")))
}

/// The recorded thread of a session folder.
pub fn recorded_thread(session_dir: &Utf8Path) -> Option<String> {
    crate::paths::read_json::<Value>(&thread_record(session_dir))
        .ok()
        .and_then(|v| v["thread_id"].as_str().map(str::to_string))
}

/// Records a session folder's thread.
pub fn record_thread(session_dir: &Utf8Path, thread: &str) {
    let _ = crate::paths::write_json_atomic(
        &thread_record(session_dir),
        &json!({"provider": "codex", "thread_id": thread, "session_dir": session_dir}),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_shell_wrappers_and_effort() {
        assert_eq!(thread_of("codex:thr_1"), Some("thr_1"));
        assert_eq!(thread_of("8f1c-claude-session"), None);
        assert_eq!(thread_of("codex:"), None);
        assert_eq!(token_for("t"), "codex:t");
        assert_eq!(unwrap_shell("/bin/zsh -lc 'cutline validate project.json5'"), "cutline validate project.json5");
        assert_eq!(unwrap_shell("bash -c \"ls -la\""), "ls -la");
        assert_eq!(unwrap_shell("cutline render project.json5"), "cutline render project.json5");
        assert_eq!(codex_effort(Some("max")).as_deref(), Some("xhigh"));
        assert_eq!(codex_effort(Some("medium")).as_deref(), Some("medium"));
        assert_eq!(codex_effort(Some("turbo")), None);
    }

    #[test]
    fn sandbox_keeps_codex_in_its_folder() {
        let o = CodexOptions { working_dir: "/runs/r1".into(), web_access: false, ..Default::default() };
        let p = o.sandbox_policy();
        assert_eq!(p["type"], "workspaceWrite");
        assert_eq!(p["writableRoots"], json!(["/runs/r1"]));
        assert_eq!((p["networkAccess"].as_bool(), p["excludeSlashTmp"].as_bool()), (Some(false), Some(true)));
        let c = o.thread_config();
        assert_eq!(c["web_search"], "disabled");
        assert_eq!(c["mcp_servers"], json!({}));
        let t = o.thread_params(Some("thr"));
        assert_eq!((t["approvalPolicy"].as_str(), t["approvalsReviewer"].as_str()), (Some("untrusted"), Some("user")));
        assert_eq!(t["threadId"], "thr");
        let web = CodexOptions { web_access: true, ..o };
        assert_eq!(web.thread_config()["web_search"], "live");
        assert_eq!(web.sandbox_policy()["networkAccess"], true);
    }
}
