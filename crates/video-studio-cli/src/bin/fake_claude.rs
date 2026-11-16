//! A stand-in for the Claude Code CLI (ui/spec.md §20). It speaks the same stream-json protocol
//! and replays a script, so runs can be tested end to end without a model: tool calls go through
//! the PreToolUse hook and the stdio permission prompt exactly as the real CLI's do, allowed Bash
//! commands really run, and Write and Edit really change files.
//!
//! The script is a JSON array named by `FAKE_CLAUDE_SCRIPT`. Steps:
//! - `{"say": "text"}` — assistant text
//! - `{"tool": "Bash", "input": {…}}` — a tool call (Bash, Write, Edit, Read, Glob, Grep,
//!   AskUserQuestion, WebFetch, …)
//! - `{"sleep_ms": 200}`, `{"hang": true}` (wait for an interrupt), `{"crash": 7}` (exit without
//!   a result), `{"spawn": "sleep 600"}` (a child process, to test that the tree is killed)
//! - `{"result": {"text": "…", "is_error": false, "cost": 0.12}}` — the final result (a default
//!   one is sent if the script ends without it)
//!
//! `FAKE_CLAUDE_TRACE` names a file that receives one JSON line per tool call with how it was
//! decided, plus the arguments the CLI was started with. `FAKE_CLAUDE_IGNORE_INTERRUPT=1` makes
//! it ignore interrupts, to test the kill after the grace period.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

fn emit(v: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn trace(v: Value) {
    if let Ok(path) = std::env::var("FAKE_CLAUDE_TRACE") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{v}");
        }
    }
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

struct Session {
    id: String,
    counter: u64,
    hook: Option<String>,
    responses: Receiver<Value>,
    interrupted: Arc<AtomicBool>,
    turns: u64,
}

impl Session {
    fn next(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}_{}", self.counter)
    }

    fn assistant(&mut self, content: Value) {
        self.turns += 1;
        let id = self.next("msg");
        emit(json!({"type": "assistant", "session_id": self.id, "parent_tool_use_id": null,
            "message": {"id": id, "role": "assistant", "content": [content], "usage": {"input_tokens": 12, "output_tokens": 30, "cache_read_input_tokens": 2000, "cache_creation_input_tokens": 0}}}));
    }

    fn tool_result(&self, tool_use_id: &str, content: &str, is_error: bool) {
        emit(
            json!({"type": "user", "session_id": self.id, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error}]}}),
        );
    }

    /// Runs the PreToolUse hook the way the CLI does: the tool call on stdin, a decision on stdout.
    fn run_hook(&self, tool: &str, input: &Value) -> Option<(String, String)> {
        let command = self.hook.as_ref()?;
        let mut child = shell()
            .arg("-c")
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let payload = json!({"hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": input, "cwd": std::env::current_dir().ok().map(|p| p.display().to_string())});
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.to_string().as_bytes());
        }
        let out = child.wait_with_output().ok()?;
        let v: Value = serde_json::from_slice(&out.stdout).ok()?;
        let d = v["hookSpecificOutput"]["permissionDecision"].as_str()?.to_string();
        Some((d, v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string()))
    }

    /// Asks the host over stdio (`can_use_tool`) and waits for its answer.
    fn ask_host(&mut self, tool: &str, input: &Value, tool_use_id: &str) -> (bool, Value, String) {
        let rid = self.next("req");
        emit(
            json!({"type": "control_request", "request_id": rid, "request": {"subtype": "can_use_tool", "tool_name": tool, "input": input, "tool_use_id": tool_use_id}}),
        );
        loop {
            match self.responses.recv_timeout(Duration::from_millis(50)) {
                Ok(v) if v["response"]["request_id"] == rid.as_str() => {
                    let r = &v["response"]["response"];
                    let allow = r["behavior"] == "allow";
                    return (allow, r["updatedInput"].clone(), r["message"].as_str().unwrap_or("").to_string());
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if self.interrupted.load(Ordering::SeqCst) {
                        return (false, Value::Null, "interrupted".into());
                    }
                }
                Err(_) => return (false, Value::Null, "stdin closed".into()),
            }
        }
    }

    fn tool(&mut self, tool: &str, input: Value) {
        let id = self.next("toolu");
        self.assistant(json!({"type": "tool_use", "id": id, "name": tool, "input": input}));
        let mut input = input;
        let (via, allowed, reason) = match self.run_hook(tool, &input) {
            Some((d, why)) if d == "deny" => ("hook", false, format!("PreToolUse:{tool} hook error: {why}")),
            Some((d, _)) if d == "allow" => ("hook", true, String::new()),
            _ => {
                let (ok, updated, message) = self.ask_host(tool, &input, &id);
                if ok && updated.is_object() {
                    input = updated;
                }
                ("stdio", ok, message)
            }
        };
        if !allowed {
            trace(json!({"tool": tool, "input": input, "decision": "deny", "via": via, "reason": reason}));
            self.tool_result(&id, &reason, true);
            return;
        }
        let (content, is_error) = execute(tool, &input);
        trace(
            json!({"tool": tool, "input": input, "decision": "allow", "via": via, "is_error": is_error, "result": content.chars().take(2000).collect::<String>()}),
        );
        self.tool_result(&id, &content, is_error);
    }
}

/// The shell the real CLI uses for Bash and hooks: `sh` on Unix, Git Bash on Windows (found as
/// Claude Code finds it: `CLAUDE_CODE_GIT_BASH_PATH`, then next to `git`, then the usual places).
fn shell() -> Command {
    #[cfg(windows)]
    {
        Command::new(git_bash())
    }
    #[cfg(not(windows))]
    {
        Command::new("sh")
    }
}

#[cfg(windows)]
fn git_bash() -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Some(p) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return p;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        // git.exe in <Git>\cmd or <Git>\bin: bash.exe is <Git>\bin\bash.exe
        if dir.join("git.exe").is_file() {
            if let Some(root) = dir.parent() {
                let b = root.join("bin").join("bash.exe");
                if b.is_file() {
                    return b;
                }
            }
        }
    }
    for c in [r"C:\Program Files\Git\bin\bash.exe", r"C:\Program Files (x86)\Git\bin\bash.exe"] {
        if std::path::Path::new(c).is_file() {
            return PathBuf::from(c);
        }
    }
    // A bash on PATH that isn't WSL's launcher in System32.
    for dir in std::env::split_paths(&path) {
        let b = dir.join("bash.exe");
        if b.is_file() && !dir.to_string_lossy().to_lowercase().contains("system32") {
            return b;
        }
    }
    PathBuf::from("bash.exe")
}

fn execute(tool: &str, input: &Value) -> (String, bool) {
    let path = input["file_path"].as_str().unwrap_or("");
    match tool {
        "Bash" => {
            let out = shell().arg("-c").arg(input["command"].as_str().unwrap_or("true")).stdin(Stdio::null()).output();
            match out {
                Ok(o) => {
                    let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                    text.push_str(&String::from_utf8_lossy(&o.stderr));
                    let code = o.status.code().unwrap_or(-1);
                    if code == 0 {
                        (text, false)
                    } else {
                        (format!("Exit code {code}\n{text}"), true)
                    }
                }
                Err(e) => (e.to_string(), true),
            }
        }
        "Write" => {
            if let Some(dir) = std::path::Path::new(path).parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::write(path, input["content"].as_str().unwrap_or("")) {
                Ok(()) => (format!("File created successfully at: {path}"), false),
                Err(e) => (e.to_string(), true),
            }
        }
        "Edit" => match std::fs::read_to_string(path) {
            Ok(text) => {
                let old = input["old_string"].as_str().unwrap_or("");
                if !text.contains(old) {
                    return ("String to replace not found in file.".into(), true);
                }
                let new = text.replacen(old, input["new_string"].as_str().unwrap_or(""), 1);
                match std::fs::write(path, new) {
                    Ok(()) => (format!("The file {path} has been updated."), false),
                    Err(e) => (e.to_string(), true),
                }
            }
            Err(e) => (e.to_string(), true),
        },
        "Read" => match std::fs::read_to_string(path) {
            Ok(t) => (t, false),
            Err(e) => (e.to_string(), true),
        },
        "AskUserQuestion" => (format!("User has answered your questions: {}", input["answers"]), false),
        _ => ("ok".into(), false),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") {
        println!("{}", std::env::var("FAKE_CLAUDE_VERSION").unwrap_or_else(|_| "2.1.311 (Claude Code)".into()));
        return;
    }
    if args.first().map(String::as_str) == Some("--help") {
        println!("Usage: claude [options]\n  --effort <level>   Effort level for the current session\n                     (low, medium, high)\n  --model <model>    Model for the current session");
        return;
    }
    if args.first().map(String::as_str) == Some("auth") {
        println!("{}", json!({"loggedIn": std::env::var("FAKE_CLAUDE_SIGNED_OUT").is_err()}));
        return;
    }
    let model = arg_value(&args, "--model").unwrap_or_else(|| "claude-fake-1".into());
    let session_id = arg_value(&args, "--resume").unwrap_or_else(|| format!("fake-session-{}", std::process::id()));
    let hook = arg_value(&args, "--settings")
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().map(str::to_string));
    // As the real CLI does: a prompt file that isn't there is an error before anything starts.
    let prompt_file = arg_value(&args, "--append-system-prompt-file");
    let system_prompt = match &prompt_file {
        Some(f) => match std::fs::read_to_string(f) {
            Ok(t) => Some(t),
            Err(_) => {
                eprintln!("Error: Append system prompt file not found: {f}");
                std::process::exit(1);
            }
        },
        None => arg_value(&args, "--append-system-prompt"),
    };
    trace(
        json!({"start": true, "args": args, "cwd": std::env::current_dir().ok().map(|p| p.display().to_string()), "resumed": arg_value(&args, "--resume"), "context_env": std::env::var("VIDEO_STUDIO_RUN_CONTEXT").is_ok(), "system_prompt_chars": system_prompt.map(|s| s.chars().count()), "system_prompt_file": prompt_file.is_some()}),
    );

    // stdin: the first user message starts the turn; control responses answer permission requests.
    let (resp_tx, resp_rx) = channel::<Value>();
    let (user_tx, user_rx) = channel::<String>();
    let interrupted = Arc::new(AtomicBool::new(false));
    let int_flag = interrupted.clone();
    let ignore_interrupt = std::env::var("FAKE_CLAUDE_IGNORE_INTERRUPT").is_ok();
    // A stuck CLI on Windows also ignores the Ctrl-Break the host sends after the interrupt, so
    // only ending its Job Object stops it.
    #[cfg(windows)]
    if ignore_interrupt {
        unsafe extern "system" fn swallow(_event: u32) -> windows::core::BOOL {
            windows::core::BOOL(1)
        }
        unsafe {
            let _ = windows::Win32::System::Console::SetConsoleCtrlHandler(Some(swallow), true);
        }
    }
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(|l| l.ok()) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            match v["type"].as_str() {
                Some("user") => {
                    let _ = user_tx.send(v["message"]["content"].as_str().unwrap_or("").to_string());
                }
                Some("control_response") => {
                    let _ = resp_tx.send(v);
                }
                Some("control_request") => {
                    let rid = v["request_id"].as_str().unwrap_or("").to_string();
                    if v["request"]["subtype"] == "interrupt" && !ignore_interrupt {
                        int_flag.store(true, Ordering::SeqCst);
                    }
                    emit(json!({"type": "control_response", "response": {"subtype": "success", "request_id": rid}}));
                }
                _ => {}
            }
        }
    });

    let prompt = user_rx.recv_timeout(Duration::from_secs(20)).unwrap_or_default();
    trace(json!({"prompt": prompt}));
    emit(
        json!({"type": "system", "subtype": "init", "session_id": session_id, "model": model, "tools": arg_value(&args, "--tools").map(|t| t.split(',').map(str::to_string).collect::<Vec<_>>()).unwrap_or_default(), "claude_code_version": "2.1.311", "permissionMode": "default"}),
    );

    let script: Vec<Value> = std::env::var("FAKE_CLAUDE_SCRIPT")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| vec![json!({"say": "Hello from the fake Claude."})]);
    let mut s = Session {
        id: session_id.clone(),
        counter: 0,
        hook,
        responses: resp_rx,
        interrupted: interrupted.clone(),
        turns: 0,
    };
    let mut result: Option<Value> = None;
    let finish = |result: Value, s: &Session| {
        emit(
            json!({"type": "result", "subtype": result["subtype"].as_str().unwrap_or("success"), "is_error": result["is_error"].as_bool().unwrap_or(false), "result": result["text"].as_str().unwrap_or("Done."),
            "session_id": s.id, "total_cost_usd": result["cost"].as_f64().unwrap_or(0.0421), "duration_ms": 1234, "num_turns": s.turns, "usage": {"input_tokens": 120, "output_tokens": 340, "cache_read_input_tokens": 9000, "cache_creation_input_tokens": 500}}),
        );
    };
    for step in script {
        if interrupted.load(Ordering::SeqCst) {
            finish(json!({"subtype": "error_during_execution", "is_error": true, "text": "interrupted"}), &s);
            return;
        }
        if let Some(text) = step["say"].as_str() {
            emit(
                json!({"type": "stream_event", "session_id": s.id, "parent_tool_use_id": null, "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": text}}}),
            );
            s.assistant(json!({"type": "text", "text": text}));
        } else if let Some(tool) = step["tool"].as_str() {
            s.tool(tool, step["input"].clone());
        } else if let Some(ms) = step["sleep_ms"].as_u64() {
            let until = std::time::Instant::now() + Duration::from_millis(ms);
            while std::time::Instant::now() < until && !interrupted.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
        } else if step["hang"].as_bool() == Some(true) {
            loop {
                std::thread::sleep(Duration::from_millis(20));
                if interrupted.load(Ordering::SeqCst) {
                    break;
                }
            }
        } else if let Some(code) = step["crash"].as_i64() {
            eprintln!("fake claude: simulated crash");
            std::process::exit(code as i32);
        } else if let Some(cmd) = step["spawn"].as_str() {
            if let Ok(child) =
                shell().arg("-c").arg(cmd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
            {
                trace(json!({"spawned": child.id()}));
            }
        } else if step["result"].is_object() {
            result = Some(step["result"].clone());
            break;
        }
    }
    if interrupted.load(Ordering::SeqCst) {
        finish(json!({"subtype": "error_during_execution", "is_error": true, "text": "interrupted"}), &s);
        return;
    }
    finish(result.unwrap_or_else(|| json!({})), &s);
    // Like the real CLI, stay alive until the host closes stdin.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while user_rx.recv_timeout(Duration::from_millis(20)) != Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        && std::time::Instant::now() < deadline
    {}
}
