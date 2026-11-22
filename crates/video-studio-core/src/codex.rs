//! Codex App Server (FEAT-spry-alpaca-5RQZ): an alternative to Claude Code, spoken to over
//! stdio. `codex app-server` reads and writes one JSON-RPC message per line (without the
//! `jsonrpc` member): requests carry `id` and `method`, responses `id` and `result` or `error`,
//! notifications only `method`. The server can also send requests of its own (approvals, user
//! input); a client must answer each one.
//!
//! This module holds the transport shared by the CLI, the FFI and the desktop shells: finding the
//! executable, the `initialize` / `initialized` handshake, `account/read`, `model/list`, and the
//! readiness check built on them. Tested against codex-cli 0.171.0's message shapes.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{Error, Result};

/// The assistant and production provider (Settings › Assistant).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// Claude Code (the default, also for settings written before the choice existed).
    #[default]
    Claude,
    /// Codex App Server.
    Codex,
}

/// JSON-RPC "method not found": the answer to a server request this client doesn't handle.
pub const METHOD_NOT_FOUND: i64 = -32601;

/// Finds Codex: the configured executable when set (it must exist), else `codex` on PATH.
pub fn find_codex(configured: &str) -> Option<Utf8PathBuf> {
    let c = configured.trim();
    if !c.is_empty() {
        let p = Utf8PathBuf::from(c);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    let names: &[&str] = if cfg!(windows) { &["codex.exe", "codex.cmd", "codex"] } else { &["codex"] };
    std::env::split_paths(&path).find_map(|dir| {
        names.iter().map(|n| dir.join(n)).find(|p| p.is_file()).and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
    })
}

/// A running `codex app-server`.
pub struct AppServer {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: i64,
    /// Notifications received while waiting for responses, in order.
    pub notifications: Vec<Value>,
    /// Methods of server requests that were answered with an error (unsupported).
    pub refused_requests: Vec<String>,
}

impl AppServer {
    /// Starts `<codex> app-server` with piped stdio.
    pub fn start(codex: &Utf8Path) -> Result<AppServer> {
        let mut child = Command::new(codex.as_str())
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::Environment(format!("Codex didn't start ({codex}): {e}")))?;
        let stdin = child.stdin.take().ok_or_else(|| Error::other("no stdin for Codex"))?;
        let stdout = child.stdout.take().ok_or_else(|| Error::other("no stdout for Codex"))?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(AppServer { child, stdin, lines: rx, next_id: 1, notifications: vec![], refused_requests: vec![] })
    }

    fn send(&mut self, v: &Value) -> Result<()> {
        writeln!(self.stdin, "{v}")
            .and_then(|_| self.stdin.flush())
            .map_err(|e| Error::other(format!("Codex closed its input: {e}")))
    }

    /// Sends a notification (no answer expected).
    pub fn notify(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let mut m = json!({"method": method});
        if let Some(p) = params {
            m["params"] = p;
        }
        self.send(&m)
    }

    /// Sends a request and waits for its response. Notifications that arrive meanwhile are kept;
    /// server requests are answered with "method not found" with their original ID.
    pub fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(l) => l,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(Error::other(format!("Codex didn't answer `{method}` in {} s", timeout.as_secs())))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::other(format!("Codex exited before answering `{method}`")))
                }
            };
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            let has_method = msg.get("method").is_some();
            match (msg.get("id"), has_method) {
                // a request from the server: refuse what this client doesn't handle
                (Some(rid), true) => {
                    let m = msg["method"].as_str().unwrap_or("").to_string();
                    self.send(&json!({"id": rid, "error": {"code": METHOD_NOT_FOUND, "message": format!("`{m}` isn't supported by this client")}}))?;
                    self.refused_requests.push(m);
                }
                (None, true) => self.notifications.push(msg),
                (Some(rid), false) if rid == &json!(id) => {
                    if let Some(e) = msg.get("error") {
                        return Err(Error::other(format!(
                            "Codex refused `{method}`: {}",
                            e["message"].as_str().unwrap_or("no reason given")
                        )));
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
                _ => {}
            }
        }
    }

    /// `initialize`, then the `initialized` notification.
    pub fn initialize(&mut self) -> Result<Value> {
        let r = self.request(
            "initialize",
            json!({"clientInfo": {"name": "video_studio", "title": crate::APP_NAME, "version": crate::CORE_VERSION}}),
            Duration::from_secs(20),
        )?;
        self.notify("initialized", None)?;
        Ok(r)
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A model Codex offers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexModel {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// What the readiness check found.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CodexStatus {
    /// No executable at the configured path, or none on PATH.
    Missing,
    /// It started but doesn't speak the app-server protocol this app uses.
    Incompatible {
        detail: String,
    },
    /// It answers, but no account is signed in.
    SignInRequired,
    Ready {
        plan: Option<String>,
        models: Vec<CodexModel>,
    },
}

impl CodexStatus {
    /// What to tell the user, and what to do about it.
    pub fn finding(&self) -> Option<(&'static str, String, String)> {
        match self {
            CodexStatus::Ready { .. } => None,
            CodexStatus::Missing => Some((
                "codex_missing",
                "Codex isn't installed".into(),
                "Install the Codex CLI (it provides `codex app-server`), or set its path in Settings › Assistant, then re-check.".into(),
            )),
            CodexStatus::Incompatible { detail } => Some((
                "codex_incompatible",
                "This Codex doesn't speak the app-server protocol the app needs".into(),
                format!("Update the Codex CLI (tested with 0.171.0), then re-check. ({detail})"),
            )),
            CodexStatus::SignInRequired => Some((
                "codex_signed_out",
                "Codex isn't signed in".into(),
                "Open Terminal, run `codex login` and sign in. The app never asks for OpenAI credentials.".into(),
            )),
        }
    }
}

/// Models from a `model/list` result (`data` or `models`, each with `id` or `model`).
pub fn parse_models(v: &Value) -> Vec<CodexModel> {
    let list = v.get("data").or_else(|| v.get("models")).and_then(Value::as_array).cloned().unwrap_or_default();
    list.iter()
        .filter_map(|m| {
            let id = m.get("id").or_else(|| m.get("model")).and_then(Value::as_str)?.to_string();
            let name = m
                .get("displayName")
                .or_else(|| m.get("display_name"))
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string();
            let is_default =
                m.get("isDefault").or_else(|| m.get("is_default")).and_then(Value::as_bool).unwrap_or(false);
            Some(CodexModel { id, name, is_default })
        })
        .collect()
}

/// Starts Codex, completes the handshake and reads the account and the models.
pub fn check(configured_path: &str) -> CodexStatus {
    let Some(path) = find_codex(configured_path) else { return CodexStatus::Missing };
    let mut s = match AppServer::start(&path) {
        Ok(s) => s,
        Err(_) => return CodexStatus::Missing,
    };
    let init = match s.initialize() {
        Ok(v) => v,
        Err(e) => return CodexStatus::Incompatible { detail: e.to_string() },
    };
    if !init.is_object() {
        return CodexStatus::Incompatible { detail: "`initialize` returned no result".into() };
    }
    let account = match s.request("account/read", json!({}), Duration::from_secs(20)) {
        Ok(v) => v,
        Err(e) => return CodexStatus::Incompatible { detail: e.to_string() },
    };
    if account.get("account").map(Value::is_null).unwrap_or(true) {
        return CodexStatus::SignInRequired;
    }
    let plan = account["account"]["planType"].as_str().map(str::to_string);
    let models =
        s.request("model/list", json!({}), Duration::from_secs(20)).map(|v| parse_models(&v)).unwrap_or_default();
    CodexStatus::Ready { plan, models }
}

/// The model a Codex session uses: the one chosen for Codex, else Codex's own default (`None`).
pub fn model_for(settings_model: &str) -> Option<String> {
    let m = settings_model.trim();
    (!m.is_empty()).then(|| m.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_parse_leniently() {
        let v = json!({"data": [{"id": "gpt-5.5-codex", "displayName": "GPT-5.5 Codex", "isDefault": true}, {"model": "o4"}, {"nope": 1}]});
        let m = parse_models(&v);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].name.as_str(), m[0].is_default), ("GPT-5.5 Codex", true));
        assert_eq!((m[1].id.as_str(), m[1].name.as_str()), ("o4", "o4"));
        assert_eq!(model_for("  "), None);
        assert_eq!(model_for("o4"), Some("o4".into()));
    }

    #[test]
    fn a_missing_executable_is_missing() {
        assert_eq!(check("/definitely/not/here/codex"), CodexStatus::Missing);
        assert_eq!(CodexStatus::Missing.finding().unwrap().0, "codex_missing");
        assert!(CodexStatus::SignInRequired.finding().unwrap().2.contains("codex login"));
    }
}
