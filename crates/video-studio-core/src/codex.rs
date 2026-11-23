//! Codex App Server (FEAT-spry-alpaca-5RQZ): an alternative to Claude Code, spoken to over
//! stdio. `codex app-server` reads and writes one JSON-RPC message per line (without the
//! `jsonrpc` member): requests carry `id` and `method`, responses `id` and `result` or `error`,
//! notifications only `method`. The server can also send requests of its own (approvals, user
//! input); a client must answer each one.
//!
//! This module holds the transport shared by the CLI, the FFI and the desktop shells: finding the
//! executable, the `initialize` / `initialized` handshake, `account/read`, `model/list`, and the
//! readiness check built on them. [`agent`] drives a whole turn (threads, items, approvals,
//! questions, Stop) for the assistant and runs. Message shapes follow codex-cli 0.171.0
//! (`codex app-server generate-ts` / `generate-json-schema`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{Error, Result};

pub mod agent;

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

/// Where the macOS app looks for Codex after PATH. An app opened from Finder gets a short PATH
/// without Homebrew, so these are searched even when PATH is unset.
pub const MACOS_FALLBACKS: [&str; 2] = ["/opt/homebrew/bin/codex", "/usr/local/bin/codex"];

/// The result of looking for Codex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexLookup {
    Found(Utf8PathBuf),
    /// The path set in Settings isn't a usable executable.
    BadPath {
        path: String,
        reason: String,
    },
    NotFound,
}

impl CodexLookup {
    pub fn found(self) -> Option<Utf8PathBuf> {
        match self {
            CodexLookup::Found(p) => Some(p),
            _ => None,
        }
    }

    /// What to tell the user when Codex can't be used.
    pub fn problem(&self) -> String {
        match self {
            CodexLookup::Found(_) => String::new(),
            CodexLookup::BadPath { path, reason } => format!(
                "The Codex path in Settings › Codex, {path}, {reason}. Correct it, or clear it so the app searches for Codex itself."
            ),
            CodexLookup::NotFound => {
                "Codex isn't installed. Install the Codex CLI, or set its path in Settings › Codex.".into()
            }
        }
    }
}

/// Why `p` can't be run, or `None` when it's an executable file. Symlinks are followed, so a
/// link to an executable counts and a broken link doesn't.
fn not_executable(p: &std::path::Path) -> Option<String> {
    match std::fs::metadata(p) {
        Err(_) if std::fs::symlink_metadata(p).is_ok() => Some("is a broken symbolic link".into()),
        Err(_) => Some("doesn't exist".into()),
        Ok(m) if !m.is_file() => Some("isn't a file".into()),
        Ok(_m) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if _m.permissions().mode() & 0o111 == 0 {
                    return Some("isn't executable".into());
                }
            }
            None
        }
    }
}

/// Looks for Codex: the configured executable when set (it must be runnable; nothing else is
/// tried), else `codex` in each PATH folder, else the platform's usual install locations
/// ([`MACOS_FALLBACKS`] on macOS). Settings, readiness, assistant turns and runs all use this.
pub fn resolve_codex(configured: &str) -> CodexLookup {
    let fallbacks: Vec<Utf8PathBuf> =
        if cfg!(target_os = "macos") { MACOS_FALLBACKS.iter().map(Utf8PathBuf::from).collect() } else { Vec::new() };
    resolve_with(configured, std::env::var_os("PATH").as_deref(), &fallbacks)
}

/// [`resolve_codex`] with the PATH value and fallbacks given (for tests).
pub fn resolve_with(configured: &str, path: Option<&std::ffi::OsStr>, fallbacks: &[Utf8PathBuf]) -> CodexLookup {
    let c = configured.trim();
    if !c.is_empty() {
        let p = Utf8PathBuf::from(c);
        return match not_executable(p.as_std_path()) {
            None => CodexLookup::Found(p),
            Some(reason) => CodexLookup::BadPath { path: c.to_string(), reason },
        };
    }
    let names: &[&str] = if cfg!(windows) { &["codex.exe", "codex.cmd", "codex"] } else { &["codex"] };
    let on_path =
        path.into_iter().flat_map(std::env::split_paths).flat_map(|dir| names.iter().map(move |n| dir.join(n)));
    on_path
        .chain(fallbacks.iter().map(|f| f.as_std_path().to_path_buf()))
        .filter(|p| p.is_absolute())
        .find(|p| not_executable(p).is_none())
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        .map(CodexLookup::Found)
        .unwrap_or(CodexLookup::NotFound)
}

/// The Codex executable to run, if there is one ([`resolve_codex`]).
pub fn find_codex(configured: &str) -> Option<Utf8PathBuf> {
    resolve_codex(configured).found()
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
    /// The path set in Settings isn't a usable executable.
    BadPath {
        path: String,
        reason: String,
    },
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
                "Install the Codex CLI (it provides `codex app-server`), or set its path in Settings › Codex, then re-check.".into(),
            )),
            CodexStatus::BadPath { path, reason } => Some((
                "codex_path_invalid",
                format!("The Codex path {path} {reason}"),
                "Correct the path in Settings › Codex, or clear it so the app searches PATH, /opt/homebrew/bin and /usr/local/bin, then re-check.".into(),
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
    let path = match resolve_codex(configured_path) {
        CodexLookup::Found(p) => p,
        CodexLookup::BadPath { path, reason } => return CodexStatus::BadPath { path, reason },
        CodexLookup::NotFound => return CodexStatus::Missing,
    };
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
        assert!(matches!(check("/definitely/not/here/codex"), CodexStatus::BadPath { .. }));
        assert_eq!(CodexStatus::Missing.finding().unwrap().0, "codex_missing");
        assert!(CodexStatus::SignInRequired.finding().unwrap().2.contains("codex login"));
    }

    /// FEAT-spry-alpaca-5RQZ: the resolver, without the real PATH or the real install folders.
    #[cfg(unix)]
    #[test]
    fn the_resolver_searches_path_then_the_install_folders() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        let dir = |n: &str| {
            let d = Utf8PathBuf::from_path_buf(root.path().join(n)).unwrap();
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let exe = |p: &Utf8Path, mode: u32| {
            std::fs::write(p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        let (on_path, brew, other, links) = (dir("path"), dir("brew"), dir("other"), dir("links"));
        let brew_codex = brew.join("codex");
        exe(&brew_codex, 0o755);
        let fallbacks = vec![other.join("codex"), brew_codex.clone()];
        let path_var = std::ffi::OsString::from(on_path.as_str());

        // PATH unset (an app opened from Finder may have none): the install folders, in order
        assert_eq!(resolve_with("", None, &fallbacks), CodexLookup::Found(brew_codex.clone()));
        // nothing on PATH: still the install folders
        assert_eq!(resolve_with("", Some(&path_var), &fallbacks), CodexLookup::Found(brew_codex.clone()));
        // a non-executable candidate is skipped
        exe(&on_path.join("codex"), 0o644);
        assert_eq!(resolve_with("", Some(&path_var), &fallbacks), CodexLookup::Found(brew_codex.clone()));
        // an executable on PATH comes before the install folders
        exe(&on_path.join("codex"), 0o755);
        assert_eq!(resolve_with("", Some(&path_var), &fallbacks), CodexLookup::Found(on_path.join("codex")));
        // a configured path comes before PATH
        let configured = other.join("my-codex");
        exe(&configured, 0o755);
        assert_eq!(
            resolve_with(configured.as_str(), Some(&path_var), &fallbacks),
            CodexLookup::Found(configured.clone())
        );

        // symlinks are followed: a link to an executable counts, a broken one doesn't
        let link_path = std::ffi::OsString::from(links.as_str());
        symlink(&configured, links.join("codex")).unwrap();
        assert_eq!(resolve_with("", Some(&link_path), &[]), CodexLookup::Found(links.join("codex")));
        std::fs::remove_file(&configured).unwrap();
        assert_eq!(resolve_with("", Some(&link_path), &fallbacks), CodexLookup::Found(brew_codex.clone()));
        assert_eq!(resolve_with("", Some(&link_path), &[]), CodexLookup::NotFound);

        // a configured path that can't run is reported with its path, and nothing else is tried
        let broken = links.join("codex");
        match resolve_with(broken.as_str(), Some(&path_var), &fallbacks) {
            CodexLookup::BadPath { path, reason } => {
                assert_eq!(path, broken.as_str());
                assert_eq!(reason, "is a broken symbolic link");
            }
            other => panic!("{other:?}"),
        }
        let plain = other.join("plain");
        exe(&plain, 0o600);
        let bad = resolve_with(plain.as_str(), Some(&path_var), &fallbacks);
        assert_eq!(bad, CodexLookup::BadPath { path: plain.to_string(), reason: "isn't executable".into() });
        assert!(bad.problem().contains(plain.as_str()) && bad.problem().contains("Settings › Codex"));
        assert_eq!(
            resolve_with(other.as_str(), None, &[]),
            CodexLookup::BadPath { path: other.to_string(), reason: "isn't a file".into() }
        );
        assert!(MACOS_FALLBACKS == ["/opt/homebrew/bin/codex", "/usr/local/bin/codex"]);
    }
}
