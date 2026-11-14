//! Finding the `claude` and `cutline` binaries (spec §4.1, §18).
//!
//! GUI apps don't inherit the shell PATH, so detection checks, in order: the path from Settings,
//! `command -v` in the user's login shell, then the usual install locations.

use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::platform::run_capture;
use crate::{paths, Error, Result};

/// The oldest Claude Code CLI the app runs with. Verified against 2.1.311 (docs/decisions.md):
/// `--permission-prompt-tool stdio`, `--tools`, `--settings` hooks, `--effort`, `--resume`,
/// `--max-budget-usd`, and `total_cost_usd` / `usage` in the result event.
pub const MIN_CLAUDE_VERSION: (u32, u32, u32) = (2, 1, 0);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeInfo {
    pub path: String,
    /// `2.1.311`
    pub version: String,
    /// The CLI's own version line, e.g. `2.1.311 (Claude Code)`.
    pub version_text: String,
    /// Effort levels listed by `claude --help`; empty when the CLI has no `--effort`.
    pub effort_levels: Vec<String>,
    /// PATH from the login shell, used when launching the CLI (it may need node and friends).
    pub login_path: Option<String>,
    pub supported: bool,
}

pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let token = text.split_whitespace().find(|t| t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))?;
    let mut parts =
        token.split('.').map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u32>());
    Some((
        parts.next()?.ok()?,
        parts.next().and_then(|p| p.ok()).unwrap_or(0),
        parts.next().and_then(|p| p.ok()).unwrap_or(0),
    ))
}

/// Parses `--effort <level> … (low, medium, high)` from the CLI help.
pub fn parse_effort_levels(help: &str) -> Vec<String> {
    let flat: String = help.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some(i) = flat.find("--effort <") else { return Vec::new() };
    let rest = &flat[i..];
    // stop at the next option so another option's parentheses aren't picked up
    let end = rest[9..].find(" --").map(|e| e + 9).unwrap_or(rest.len());
    let rest = &rest[..end];
    match (rest.rfind('('), rest.rfind(')')) {
        (Some(a), Some(b)) if b > a => rest[a + 1..b]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !s.contains(' '))
            .collect(),
        _ => Vec::new(),
    }
}

/// Runs a command line through the user's login shell.
pub fn run_login_shell(command: &str) -> Option<String> {
    if cfg!(windows) {
        let (code, out, _) = run_capture(Command::new("cmd").args(["/C", command]), Duration::from_secs(8)).ok()?;
        return (code == 0).then_some(out);
    }
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let (code, out, _) = run_capture(Command::new(shell).args(["-l", "-c", command]), Duration::from_secs(8)).ok()?;
    (code == 0).then_some(out)
}

fn login_path() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            if cfg!(windows) {
                return std::env::var("PATH").ok();
            }
            run_login_shell("echo $PATH")
                .map(|s| s.lines().last().unwrap_or("").trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .clone()
}

fn is_executable(p: &Utf8Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Looks a program up in a PATH string.
pub fn which_in(program: &str, path: &str) -> Option<Utf8PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &["", ".exe", ".cmd", ".bat"] } else { &[""] };
    for dir in std::env::split_paths(path) {
        let Ok(dir) = paths::utf8(dir) else { continue };
        for ext in exts {
            let c = dir.join(format!("{program}{ext}"));
            if is_executable(&c) {
                return Some(c);
            }
        }
    }
    None
}

fn candidates(program: &str, custom: &str, common: &[&str]) -> Vec<Utf8PathBuf> {
    let mut v: Vec<Utf8PathBuf> = Vec::new();
    let custom = custom.trim();
    if !custom.is_empty() {
        v.push(paths::expand_tilde(custom));
    }
    if let Some(p) = login_path().as_deref().and_then(|p| which_in(program, p)) {
        v.push(p);
    }
    if let Some(p) = std::env::var("PATH").ok().as_deref().and_then(|p| which_in(program, p)) {
        v.push(p);
    }
    v.extend(common.iter().map(|c| paths::expand_tilde(c)));
    let mut seen = std::collections::HashSet::new();
    v.retain(|p| seen.insert(p.clone()));
    v
}

/// Environment for launching the CLI: the login shell's PATH, without the variables that would
/// make a nested CLI think it runs inside another Claude Code session.
pub fn child_env(login: Option<&str>) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    match login.map(str::to_string).or_else(login_path) {
        Some(p) => {
            env.insert("PATH".into(), p);
        }
        None => {
            let p = env.get("PATH").cloned().unwrap_or_else(|| "/usr/bin:/bin".into());
            env.insert("PATH".into(), format!("{p}:/opt/homebrew/bin:/usr/local/bin"));
        }
    }
    env.remove("CLAUDECODE");
    env.remove("CLAUDE_CODE_ENTRYPOINT");
    env
}

fn run_with_env(
    exe: &Utf8Path,
    args: &[&str],
    login: Option<&str>,
    timeout: Duration,
) -> Option<(i32, String, String)> {
    let mut cmd = Command::new(exe.as_str());
    cmd.args(args).env_clear().envs(child_env(login));
    run_capture(&mut cmd, timeout).ok()
}

const CLAUDE_LOCATIONS: [&str; 4] =
    ["~/.claude/local/claude", "~/.local/bin/claude", "/opt/homebrew/bin/claude", "/usr/local/bin/claude"];

/// Finds Claude Code. `VIDEO_STUDIO_CLAUDE` overrides everything (tests use a fake CLI), and
/// `VIDEO_STUDIO_NO_CLAUDE` makes detection fail (to exercise the setup screen).
pub fn find_claude(custom_path: &str) -> Option<ClaudeInfo> {
    if std::env::var_os("VIDEO_STUDIO_NO_CLAUDE").is_some() {
        return None;
    }
    let login = login_path();
    let forced = std::env::var("VIDEO_STUDIO_CLAUDE").ok().filter(|s| !s.is_empty());
    let list = match &forced {
        Some(f) => vec![Utf8PathBuf::from(f)],
        None => candidates("claude", custom_path, &CLAUDE_LOCATIONS),
    };
    for c in list {
        if !is_executable(&c) {
            continue;
        }
        let Some((0, out, _)) = run_with_env(&c, &["--version"], login.as_deref(), Duration::from_secs(15)) else {
            continue;
        };
        let text = out.lines().next().unwrap_or("").trim().to_string();
        let Some(v) = parse_version(&text) else { continue };
        let effort_levels = run_with_env(&c, &["--help"], login.as_deref(), Duration::from_secs(15))
            .map(|(_, help, _)| parse_effort_levels(&help))
            .unwrap_or_default();
        return Some(ClaudeInfo {
            path: c.to_string(),
            version: format!("{}.{}.{}", v.0, v.1, v.2),
            version_text: text,
            effort_levels,
            login_path: login,
            supported: v >= MIN_CLAUDE_VERSION,
        });
    }
    None
}

/// Whether the CLI is signed in (`claude auth status`). `None` when it can't be told.
pub fn claude_signed_in(info: &ClaudeInfo) -> Option<bool> {
    let (_, out, _) = run_with_env(
        Utf8Path::new(&info.path),
        &["auth", "status"],
        info.login_path.as_deref(),
        Duration::from_secs(20),
    )?;
    let v: Value = serde_json::from_str(out.trim()).ok()?;
    v["loggedIn"].as_bool()
}

/// The folder where Claude Code keeps a session's files for a working directory
/// (`~/.claude/projects/<slug>`). Large tool results are saved there and read back with the
/// file tools, so a run's policy has to allow reading it.
pub fn claude_project_dir(cwd: &Utf8Path) -> Option<Utf8PathBuf> {
    let base = match std::env::var("CLAUDE_CONFIG_DIR").ok().filter(|s| !s.is_empty()) {
        Some(d) => Utf8PathBuf::from(d),
        None => paths::home_dir()?.join(".claude"),
    };
    let slug: String = cwd.as_str().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    Some(base.join("projects").join(slug))
}

pub fn min_version_text() -> String {
    format!("{}.{}.{}", MIN_CLAUDE_VERSION.0, MIN_CLAUDE_VERSION.1, MIN_CLAUDE_VERSION.2)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CutlineInfo {
    pub path: String,
    pub version: String,
}

/// Finds cutline: the path from Settings, PATH, then `bin/cutline` in the configured repo.
/// `VIDEO_STUDIO_CUTLINE` overrides everything.
pub fn find_cutline(custom_path: &str, repo: &str) -> Option<CutlineInfo> {
    let forced = std::env::var("VIDEO_STUDIO_CUTLINE").ok().filter(|s| !s.is_empty());
    let mut list = match &forced {
        Some(f) => vec![Utf8PathBuf::from(f)],
        None => candidates(
            "cutline",
            custom_path,
            &["~/.local/bin/cutline", "/opt/homebrew/bin/cutline", "/usr/local/bin/cutline"],
        ),
    };
    if forced.is_none() && !repo.trim().is_empty() {
        let repo = paths::expand_tilde(repo.trim());
        list.push(repo.join("bin/cutline"));
        list.push(repo.join("target/release/cutline"));
    }
    for c in list {
        if !is_executable(&c) {
            continue;
        }
        let Some((0, out, _)) = run_with_env(&c, &["--version"], None, Duration::from_secs(15)) else { continue };
        let version = out.split_whitespace().last().unwrap_or("").to_string();
        return Some(CutlineInfo { path: paths::canonical_or_normal(&c).to_string(), version });
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DoctorSummary {
    pub ok: bool,
    /// Required tools or capabilities that are missing.
    pub missing_required: Vec<String>,
    /// Optional ones that are missing (Chrome, whisper model, …).
    pub missing_optional: Vec<String>,
    pub raw: Value,
}

/// Summarizes `cutline doctor --json`: required capabilities that are missing make it not ok;
/// Chrome, the whisper model and the face model are optional and listed separately.
pub fn summarize_doctor(report: &Value) -> DoctorSummary {
    let mut s = DoctorSummary { ok: true, raw: report.clone(), ..Default::default() };
    for c in report["capabilities"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        if c["present"].as_bool() == Some(false) && c["required"].as_bool() == Some(true) {
            s.missing_required.push(format!(
                "{} {}",
                c["kind"].as_str().unwrap_or("tool"),
                c["name"].as_str().unwrap_or("?")
            ));
        }
    }
    if let Some(o) = report.as_object() {
        if o.get("chrome").map(Value::is_null).unwrap_or(false) {
            s.missing_optional.push("Chrome (HTML graphics)".into());
        }
        let no_whisper = o.get("whisper_cli").map(Value::is_null).unwrap_or(false)
            || o.get("whisper_models").and_then(Value::as_array).map(|a| a.is_empty()).unwrap_or(false);
        if no_whisper {
            s.missing_optional.push("whisper (captions from speech)".into());
        }
        if o.get("face_model").and_then(Value::as_bool) == Some(false) {
            s.missing_optional.push("face model (reframing)".into());
        }
    }
    if report["ok"].as_bool() == Some(false)
        || report["errors"].as_u64().unwrap_or(0) > 0
        || !s.missing_required.is_empty()
    {
        s.ok = false;
    }
    s
}

pub fn cutline_doctor(cutline: &Utf8Path) -> Result<DoctorSummary> {
    let (code, out, err) =
        run_capture(Command::new(cutline.as_str()).args(["doctor", "--json"]), Duration::from_secs(120))?;
    let v: Value = serde_json::from_str(out.trim()).map_err(|_| {
        Error::Environment(format!(
            "cutline doctor failed (exit {code}): {}",
            err.lines().last().unwrap_or("no output")
        ))
    })?;
    let mut s = summarize_doctor(&v);
    if code != 0 {
        s.ok = false;
    }
    Ok(s)
}

/// The Claude workflow guide cutline ships (`cutline guide`), given to Claude in production runs.
pub fn cutline_guide(cutline: &Utf8Path) -> Result<String> {
    let (code, out, err) = run_capture(Command::new(cutline.as_str()).arg("guide"), Duration::from_secs(30))?;
    if code != 0 || out.trim().is_empty() {
        return Err(Error::Environment(format!("cutline guide failed: {}", err.trim())));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("2.1.311 (Claude Code)"), Some((2, 1, 311)));
        assert_eq!(parse_version("claude 3.0"), Some((3, 0, 0)));
        assert_eq!(parse_version("cutline 0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("no digits"), None);
        assert!((2, 1, 311) >= MIN_CLAUDE_VERSION && (1, 9, 0) < MIN_CLAUDE_VERSION);
        let dir = claude_project_dir("/private/tmp/My Runs/2026-11-14-1753-shorts".into()).unwrap();
        assert!(dir.as_str().ends_with("projects/-private-tmp-My-Runs-2026-11-14-1753-shorts"), "{dir}");
    }

    #[test]
    fn effort_levels_from_help() {
        let help = "  --effort <level>                      Effort level for the current session\n                                        (low, medium, high, xhigh, max)\n  --environment <id>   Create (a thing)";
        assert_eq!(parse_effort_levels(help), vec!["low", "medium", "high", "xhigh", "max"]);
        assert!(parse_effort_levels("--model <model>  Model (sonnet, opus)").is_empty());
    }

    #[test]
    fn doctor_summary() {
        let report = serde_json::json!({"ok": true, "errors": 0, "chrome": null, "whisper_cli": "/bin/whisper-cli", "whisper_models": [], "face_model": true, "capabilities": [
            {"kind": "filter", "name": "xfade", "present": true, "required": true},
            {"kind": "filter", "name": "lut3d", "present": false, "required": true},
            {"kind": "encoder", "name": "h264_nvenc", "present": false, "required": false}]});
        let s = summarize_doctor(&report);
        assert!(!s.ok);
        assert_eq!(s.missing_required, vec!["filter lut3d"]);
        assert_eq!(s.missing_optional, vec!["Chrome (HTML graphics)", "whisper (captions from speech)"]);
        assert!(summarize_doctor(&serde_json::json!({"ok": true, "capabilities": [], "chrome": "/x"})).ok);
        assert!(!summarize_doctor(&serde_json::json!({"ok": false, "capabilities": []})).ok);
    }

    #[test]
    fn child_env_hides_nested_session_markers() {
        std::env::set_var("CLAUDECODE", "1");
        let env = child_env(Some("/x/bin"));
        assert_eq!(env.get("PATH").map(String::as_str), Some("/x/bin"));
        assert!(!env.contains_key("CLAUDECODE"));
    }
}
