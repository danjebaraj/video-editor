//! Output conventions: `--json` → machine JSON on stdout, logs on stderr. Exit codes:
//! 0 ok, 1 validation/QA error, 2 usage error, 3 tool/dependency error, 4 internal error.

use serde_json::Value;

pub const EXIT_OK: i32 = 0;
pub const EXIT_VALIDATION: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_TOOL: i32 = 3;
pub const EXIT_INTERNAL: i32 = 4;

/// Result of a command: JSON payload, human rendering and exit code.
pub struct Output {
    pub json: Value,
    pub human: String,
    pub code: i32,
}

impl Output {
    pub fn ok(json: Value, human: impl Into<String>) -> Self {
        Self { json, human: human.into(), code: EXIT_OK }
    }
    pub fn with_code(mut self, code: i32) -> Self {
        self.code = code;
        self
    }
}

/// A command failure carrying its exit code.
#[derive(Debug)]
pub struct CliError {
    pub code: i32,
    pub kind: &'static str,
    pub err: anyhow::Error,
}

impl CliError {
    pub fn usage(e: impl Into<anyhow::Error>) -> Self {
        Self { code: EXIT_USAGE, kind: "usage", err: e.into() }
    }
    pub fn tool(e: impl Into<anyhow::Error>) -> Self {
        Self { code: EXIT_TOOL, kind: "tool", err: e.into() }
    }
    pub fn validation(e: impl Into<anyhow::Error>) -> Self {
        Self { code: EXIT_VALIDATION, kind: "validation", err: e.into() }
    }
    pub fn internal(e: impl Into<anyhow::Error>) -> Self {
        Self { code: EXIT_INTERNAL, kind: "internal", err: e.into() }
    }
}

impl From<anyhow::Error> for CliError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(e)
    }
}

pub type CmdResult = Result<Output, CliError>;

/// Extension to tag errors with an exit category.
#[allow(dead_code)]
pub trait Tag<T> {
    fn tool_err(self) -> Result<T, CliError>;
    fn usage_err(self) -> Result<T, CliError>;
}

impl<T, E: Into<anyhow::Error>> Tag<T> for Result<T, E> {
    fn tool_err(self) -> Result<T, CliError> {
        self.map_err(CliError::tool)
    }
    fn usage_err(self) -> Result<T, CliError> {
        self.map_err(CliError::usage)
    }
}

pub fn emit(json_mode: bool, r: CmdResult) -> i32 {
    match r {
        Ok(o) => {
            if json_mode {
                println!("{}", serde_json::to_string_pretty(&o.json).unwrap_or_default());
            } else if !o.human.is_empty() {
                println!("{}", o.human.trim_end());
            }
            o.code
        }
        Err(e) => {
            if json_mode {
                let v = serde_json::json!({"ok": false, "error": {"kind": e.kind, "message": format!("{:#}", e.err)}});
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            }
            eprintln!("error: {:#}", e.err);
            e.code
        }
    }
}
