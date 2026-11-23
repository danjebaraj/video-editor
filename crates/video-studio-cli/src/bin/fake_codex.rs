//! A stand-in for `codex app-server` (FEAT-spry-alpaca-5RQZ tests): one JSON-RPC message per
//! line on stdio, with codex-cli 0.171.0's shapes (from `codex app-server generate-ts`).
//!
//! Behaviour from the environment:
//! - `FAKE_CODEX_LOG`: append every message received, one per line.
//! - `FAKE_CODEX_SIGNED_OUT=1`: `account/read` reports no account.
//! - `FAKE_CODEX_OLD=1`: answers every request with "method not found" (an incompatible build).
//! - `FAKE_CODEX_REJECT_RESUME=1`: `thread/resume` fails even for a known thread.
//! - `FAKE_CODEX_SCRIPT`: a JSON array of turns, each an array of steps; the n-th `turn/start`
//!   (counted across processes) plays the n-th turn, or the last one.
//! - `FAKE_CODEX_STATE`: a folder for what outlives a process (threads, the turn count); default
//!   `<script>.state`.
//!
//! Before answering `account/read` it sends a notification and a server request of its own, so
//! clients must keep notifications and answer server requests with the request's ID.
//!
//! Steps:
//! - `{"say": "text"}`: an agent message, streamed in two deltas.
//! - `{"command": "cutline render x"}`: a command execution that asks for approval (as
//!   `bash -lc '…'`); accepted commands really run (`sh -c` in the thread's folder).
//! - `{"write": {"path": "p", "content": "c"}}`, `{"edit": {"path": "p", "edits": [["old",
//!   "new"], …]}}`: a file change that asks for approval and is applied when accepted.
//! - `{"ask": {"id": "q-1", "questions": [{"id", "header", "question", "options"}]}}`:
//!   `item/tool/requestUserInput` with that request ID; the answer is echoed as a message.
//! - `{"request": {"id": "x", "method": "…", "params": {…}}}`: any server request.
//! - `{"usage": [input, output]}`: `thread/tokenUsage/updated`.
//! - `{"hang": true}`: wait for `turn/interrupt`. `{"exit": 3}`: exit at once.
//! - `{"fail": "message"}`: the turn fails.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fake {
    lines: std::io::Lines<std::io::StdinLock<'static>>,
    log: Option<String>,
    state: Option<PathBuf>,
    cwd: PathBuf,
    thread: String,
    turn: String,
    seq: u64,
    interrupted: bool,
}

fn send(v: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn shell() -> Command {
    #[cfg(windows)]
    {
        Command::new(std::env::var("CLAUDE_CODE_GIT_BASH_PATH").unwrap_or_else(|_| "bash".into()))
    }
    #[cfg(not(windows))]
    {
        Command::new("sh")
    }
}

impl Fake {
    fn read(&mut self) -> Option<Value> {
        loop {
            let line = self.lines.next()?.ok()?;
            if let Some(l) = &self.log {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(l) {
                    let _ = writeln!(f, "{line}");
                }
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                return Some(v);
            }
        }
    }

    fn next_id(&mut self, prefix: &str) -> String {
        self.seq += 1;
        format!("{prefix}-{}", self.seq)
    }

    fn state_file(&self, name: &str) -> Option<PathBuf> {
        self.state.as_ref().map(|d| {
            let _ = std::fs::create_dir_all(d);
            d.join(name)
        })
    }

    fn known_threads(&self) -> Vec<String> {
        self.state_file("threads.txt")
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Waits for the answer to a server request. `None` when the turn was interrupted meanwhile
    /// (or input ended).
    fn wait(&mut self, id: &Value) -> Option<Value> {
        loop {
            let msg = self.read()?;
            if msg.get("method").is_none() && msg.get("id") == Some(id) {
                return Some(msg);
            }
            if msg["method"] == "turn/interrupt" {
                send(json!({"id": msg["id"], "result": {}}));
                self.interrupted = true;
                return None;
            }
        }
    }

    fn thread_json(&self) -> Value {
        json!({"id": self.thread, "sessionId": self.thread, "preview": "", "ephemeral": false, "modelProvider": "openai", "model": "gpt-fake", "createdAt": 0, "updatedAt": 0, "status": {"type": "idle"}, "path": null, "cwd": self.cwd, "cliVersion": "0.171.0", "source": "appServer", "turns": []})
    }

    fn thread_response(&self, id: &Value) {
        send(
            json!({"id": id, "result": {"thread": self.thread_json(), "model": "gpt-fake", "modelProvider": "openai", "serviceTier": null, "cwd": self.cwd, "approvalPolicy": "untrusted", "approvalsReviewer": "user", "sandbox": {"type": "workspaceWrite", "writableRoots": [self.cwd], "networkAccess": false, "excludeTmpdirEnvVar": true, "excludeSlashTmp": true}, "reasoningEffort": null}}),
        );
    }

    fn item(&self, method: &str, item: Value) {
        send(
            json!({"method": method, "params": {"item": item, "threadId": self.thread, "turnId": self.turn, "startedAtMs": 0, "completedAtMs": 0}}),
        );
    }

    fn resolve(&self, p: &str) -> PathBuf {
        let p = Path::new(p);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cwd.join(p)
        }
    }

    /// One file change: announced, approved (or not), applied.
    fn file_change(&mut self, path: &str, new_text: impl FnOnce(Option<String>) -> String) -> Option<()> {
        let full = self.resolve(path);
        let old = std::fs::read_to_string(&full).ok();
        let kind = if old.is_some() { json!({"type": "update", "move_path": null}) } else { json!({"type": "add"}) };
        let item_id = self.next_id("item");
        let changes = json!([{"path": full, "kind": kind, "diff": ""}]);
        self.item(
            "item/started",
            json!({"type": "fileChange", "id": item_id, "changes": changes, "status": "inProgress"}),
        );
        let rid = json!(self.next_id("patch"));
        send(
            json!({"id": rid, "method": "item/fileChange/requestApproval", "params": {"threadId": self.thread, "turnId": self.turn, "itemId": item_id, "startedAtMs": 0}}),
        );
        let answer = self.wait(&rid)?;
        let accepted = answer["result"]["decision"] == "accept";
        if accepted {
            if let Some(d) = full.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&full, new_text(old));
        }
        let status = if accepted { "completed" } else { "declined" };
        self.item("item/completed", json!({"type": "fileChange", "id": item_id, "changes": changes, "status": status}));
        Some(())
    }

    /// Plays one step; `None` ends the turn early (interrupted, or input closed).
    fn step(&mut self, s: &Value) -> Option<()> {
        if let Some(text) = s["say"].as_str() {
            let id = self.next_id("msg");
            self.item("item/started", json!({"type": "agentMessage", "id": id, "text": "", "phase": null}));
            let mid = text.len() / 2;
            let mid = (0..=mid).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
            for part in [&text[..mid], &text[mid..]] {
                send(
                    json!({"method": "item/agentMessage/delta", "params": {"threadId": self.thread, "turnId": self.turn, "itemId": id, "delta": part}}),
                );
            }
            self.item("item/completed", json!({"type": "agentMessage", "id": id, "text": text, "phase": null}));
        } else if let Some(cmd) = s["command"].as_str() {
            let item_id = self.next_id("item");
            let shown = format!("bash -lc '{cmd}'");
            self.item("item/started", json!({"type": "commandExecution", "id": item_id, "command": shown, "cwd": self.cwd, "status": "inProgress", "commandActions": [], "aggregatedOutput": null, "exitCode": null}));
            let rid = json!(self.next_id("appr"));
            send(
                json!({"id": rid, "method": "item/commandExecution/requestApproval", "params": {"kind": "command", "threadId": self.thread, "turnId": self.turn, "itemId": item_id, "startedAtMs": 0, "environmentId": null, "command": shown, "cwd": self.cwd}}),
            );
            let answer = self.wait(&rid)?;
            if answer["result"]["decision"] == "accept" {
                let out = shell().arg("-c").arg(cmd).current_dir(&self.cwd).output();
                let (code, text) = match out {
                    Ok(o) => (
                        o.status.code().unwrap_or(-1),
                        format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)),
                    ),
                    Err(e) => (-1, e.to_string()),
                };
                send(
                    json!({"method": "item/commandExecution/outputDelta", "params": {"threadId": self.thread, "turnId": self.turn, "itemId": item_id, "delta": text}}),
                );
                let status = if code == 0 { "completed" } else { "failed" };
                self.item("item/completed", json!({"type": "commandExecution", "id": item_id, "command": shown, "cwd": self.cwd, "status": status, "commandActions": [], "aggregatedOutput": text, "exitCode": code}));
            } else {
                self.item("item/completed", json!({"type": "commandExecution", "id": item_id, "command": shown, "cwd": self.cwd, "status": "declined", "commandActions": [], "aggregatedOutput": null, "exitCode": null}));
            }
        } else if let Some(w) = s.get("write") {
            let content = w["content"].as_str().unwrap_or("").to_string();
            self.file_change(w["path"].as_str().unwrap_or(""), move |_| content)?;
        } else if let Some(e) = s.get("edit") {
            let edits: Vec<(String, String)> = e["edits"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|p| (p[0].as_str().unwrap_or("").to_string(), p[1].as_str().unwrap_or("").to_string()))
                        .collect()
                })
                .unwrap_or_default();
            self.file_change(e["path"].as_str().unwrap_or(""), move |old| {
                let mut t = old.unwrap_or_default();
                for (a, b) in &edits {
                    t = t.replacen(a.as_str(), b, 1);
                }
                t
            })?;
        } else if let Some(a) = s.get("ask") {
            let rid = a["id"].clone();
            let item_id = self.next_id("item");
            send(
                json!({"id": rid, "method": "item/tool/requestUserInput", "params": {"threadId": self.thread, "turnId": self.turn, "itemId": item_id, "questions": a["questions"], "isBlocking": true, "autoResolutionMs": null}}),
            );
            let answer = self.wait(&rid)?;
            let text = format!("Answers: {}", answer["result"]["answers"]);
            self.step(&json!({"say": text}))?;
        } else if let Some(r) = s.get("request") {
            let rid = r["id"].clone();
            send(json!({"id": rid, "method": r["method"], "params": r["params"]}));
            self.wait(&rid)?;
        } else if let Some(u) = s["usage"].as_array() {
            let (i, o) = (u[0].as_u64().unwrap_or(0), u[1].as_u64().unwrap_or(0));
            let b = json!({"totalTokens": i + o, "inputTokens": i, "cachedInputTokens": 0, "cacheWriteInputTokens": 0, "outputTokens": o, "reasoningOutputTokens": 0});
            send(
                json!({"method": "thread/tokenUsage/updated", "params": {"threadId": self.thread, "turnId": self.turn, "tokenUsage": {"total": b, "last": b, "modelContextWindow": null}}}),
            );
        } else if s["hang"].as_bool() == Some(true) {
            let never = json!("never-answered");
            self.wait(&never)?;
        } else if let Some(code) = s["exit"].as_i64() {
            std::process::exit(code as i32);
        } else if let Some(msg) = s["fail"].as_str() {
            send(
                json!({"method": "error", "params": {"error": {"message": msg, "codexErrorInfo": null, "additionalDetails": null}, "willRetry": false, "threadId": self.thread, "turnId": self.turn}}),
            );
            send(
                json!({"method": "turn/completed", "params": {"threadId": self.thread, "turn": {"id": self.turn, "items": [], "status": "failed", "error": {"message": msg, "codexErrorInfo": null, "additionalDetails": null}}}}),
            );
            return None;
        }
        Some(())
    }

    fn turn(&mut self, steps: &[Value]) {
        for s in steps {
            if self.step(s).is_none() {
                if self.interrupted {
                    send(
                        json!({"method": "turn/completed", "params": {"threadId": self.thread, "turn": {"id": self.turn, "items": [], "status": "interrupted", "error": null}}}),
                    );
                }
                return;
            }
        }
        send(
            json!({"method": "turn/completed", "params": {"threadId": self.thread, "turn": {"id": self.turn, "items": [], "status": "completed", "error": null}}}),
        );
    }
}

fn main() {
    let signed_out = std::env::var("FAKE_CODEX_SIGNED_OUT").is_ok();
    let old = std::env::var("FAKE_CODEX_OLD").is_ok();
    let reject_resume = std::env::var("FAKE_CODEX_REJECT_RESUME").is_ok();
    let script: Option<String> = std::env::var("FAKE_CODEX_SCRIPT").ok().filter(|s| !s.is_empty());
    let turns: Vec<Vec<Value>> = script
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Vec<Vec<Value>>>(&t).ok())
        .unwrap_or_default();
    let state = std::env::var("FAKE_CODEX_STATE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| script.as_ref().map(|s| PathBuf::from(format!("{s}.state"))));
    let mut f = Fake {
        lines: std::io::stdin().lock().lines(),
        log: std::env::var("FAKE_CODEX_LOG").ok(),
        state,
        cwd: std::env::current_dir().unwrap_or_default(),
        thread: String::new(),
        turn: String::new(),
        seq: 0,
        interrupted: false,
    };
    while let Some(msg) = f.read() {
        let (Some(id), Some(method)) = (msg.get("id").cloned(), msg["method"].as_str().map(str::to_string)) else {
            continue;
        };
        if old {
            send(json!({"id": id, "error": {"code": -32601, "message": format!("unknown method {method}")}}));
            continue;
        }
        let params = &msg["params"];
        match method.as_str() {
            "initialize" => send(
                json!({"id": id, "result": {"userAgent": "fake/0.171.0", "codexHome": "/tmp/fake-codex", "platformFamily": "unix", "platformOs": "fake"}}),
            ),
            "account/read" => {
                send(json!({"method": "account/updated", "params": {"authMode": "chatgpt"}}));
                send(json!({"id": "srv-1", "method": "item/tool/requestUserInput", "params": {"questions": []}}));
                if signed_out {
                    send(json!({"id": id, "result": {"account": null, "requiresOpenaiAuth": true}}));
                } else {
                    send(
                        json!({"id": id, "result": {"account": {"type": "chatgpt", "planType": "plus"}, "requiresOpenaiAuth": true}}),
                    );
                }
            }
            "model/list" => send(
                json!({"id": id, "result": {"data": [{"id": "gpt-5.5-codex", "displayName": "GPT-5.5 Codex", "isDefault": true}, {"id": "gpt-5.5-mini", "displayName": "GPT-5.5 mini", "isDefault": false}]}}),
            ),
            "thread/start" => {
                if let Some(c) = params["cwd"].as_str() {
                    f.cwd = PathBuf::from(c);
                }
                let known = f.known_threads();
                f.thread = format!("thr-{}", known.len() + 1);
                if let Some(file) = f.state_file("threads.txt") {
                    if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
                        let _ = writeln!(h, "{}", f.thread);
                    }
                }
                f.thread_response(&id);
                send(json!({"method": "thread/started", "params": {"thread": f.thread_json()}}));
            }
            "thread/resume" => {
                let want = params["threadId"].as_str().unwrap_or("").to_string();
                if reject_resume || !f.known_threads().contains(&want) {
                    send(
                        json!({"id": id, "error": {"code": -32600, "message": format!("no rollout found for thread id {want}")}}),
                    );
                } else {
                    if let Some(c) = params["cwd"].as_str() {
                        f.cwd = PathBuf::from(c);
                    }
                    f.thread = want;
                    f.thread_response(&id);
                }
            }
            "turn/start" => {
                if let Some(c) = params["cwd"].as_str() {
                    f.cwd = PathBuf::from(c);
                }
                let n = f
                    .state_file("turns.txt")
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .and_then(|t| t.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if let Some(p) = f.state_file("turns.txt") {
                    let _ = std::fs::write(p, (n + 1).to_string());
                }
                f.turn = format!("turn-{}", n + 1);
                send(
                    json!({"id": id, "result": {"turn": {"id": f.turn, "items": [], "status": "inProgress", "error": null}}}),
                );
                send(
                    json!({"method": "turn/started", "params": {"threadId": f.thread, "turn": {"id": f.turn, "items": [], "status": "inProgress", "error": null}}}),
                );
                let steps = turns.get(n).or(turns.last()).cloned().unwrap_or_default();
                f.interrupted = false;
                f.turn(&steps);
            }
            "turn/interrupt" => send(json!({"id": id, "result": {}})),
            other => send(json!({"id": id, "error": {"code": -32601, "message": format!("unknown method {other}")}})),
        }
    }
}
