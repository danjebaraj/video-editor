//! The agent behind an assistant turn or a run: Claude Code or Codex (FEAT-spry-alpaca-5RQZ).
//! Both produce the same [`Incoming`](crate::claude::process::Incoming) events and take the same
//! answers, so the assistant and the orchestrator follow either one the same way.

use std::time::Duration;

use serde_json::Value;

use crate::claude::process::ClaudeProcess;
use crate::codex::agent::CodexProcess;

#[derive(Clone)]
pub enum AgentProcess {
    Claude(ClaudeProcess),
    Codex(CodexProcess),
}

impl AgentProcess {
    /// The agent's name in messages.
    pub fn name(&self) -> &'static str {
        match self {
            AgentProcess::Claude(_) => "Claude Code",
            AgentProcess::Codex(_) => "Codex",
        }
    }

    pub fn is_codex(&self) -> bool {
        matches!(self, AgentProcess::Codex(_))
    }

    pub fn allow(&self, request_id: &str, input: &Value) {
        match self {
            AgentProcess::Claude(p) => p.allow(request_id, input),
            AgentProcess::Codex(p) => p.allow(request_id, input),
        }
    }

    pub fn deny(&self, request_id: &str, message: &str) {
        match self {
            AgentProcess::Claude(p) => p.deny(request_id, message),
            AgentProcess::Codex(p) => p.deny(request_id, message),
        }
    }

    pub fn log_note(&self, note: Value) {
        match self {
            AgentProcess::Claude(p) => p.log_note(note),
            AgentProcess::Codex(p) => p.log_note(note),
        }
    }

    pub fn finish_input(&self) {
        match self {
            AgentProcess::Claude(p) => p.finish_input(),
            AgentProcess::Codex(p) => p.finish_input(),
        }
    }

    pub fn is_running(&self) -> bool {
        match self {
            AgentProcess::Claude(p) => p.is_running(),
            AgentProcess::Codex(p) => p.is_running(),
        }
    }

    pub fn pid(&self) -> u32 {
        match self {
            AgentProcess::Claude(p) => p.pid(),
            AgentProcess::Codex(p) => p.pid(),
        }
    }

    pub fn kill(&self) {
        match self {
            AgentProcess::Claude(p) => p.kill(),
            AgentProcess::Codex(p) => p.kill(),
        }
    }

    /// Graceful stop: an interrupt (`turn/interrupt` for Codex), then the process tree is killed
    /// after `grace`. Returns true when it had to be killed.
    pub fn stop(&self, grace: Duration) -> bool {
        match self {
            AgentProcess::Claude(p) => p.stop(grace),
            AgentProcess::Codex(p) => p.stop(grace),
        }
    }
}
