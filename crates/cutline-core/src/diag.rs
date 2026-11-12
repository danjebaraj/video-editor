//! Machine-readable diagnostics: `{code, path (JSON pointer), message, hint, severity}`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub code: String,
    pub path: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub severity: Severity,
}

impl Diagnostic {
    pub fn error(code: &str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), path: path.into(), message: message.into(), hint: None, severity: Severity::Error }
    }
    pub fn warning(code: &str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), path: path.into(), message: message.into(), hint: None, severity: Severity::Warning }
    }
    pub fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Escape one JSON-pointer token.
pub fn ptr_token(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

pub fn has_errors(d: &[Diagnostic]) -> bool {
    d.iter().any(|x| x.is_error())
}
