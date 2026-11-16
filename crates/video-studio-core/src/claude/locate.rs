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
    /// Windows: the Git Bash the CLI's shell tool runs in. Without it Claude Code silently starts
    /// with no Bash tool at all (verified with 2.1.319), so runs can't work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_bash: Option<String>,
}

/// Git Bash for Claude Code's shell tool on Windows, found as the CLI looks for it
/// (`CLAUDE_CODE_GIT_BASH_PATH`) and then where Git for Windows installs it: next to `git` on
/// PATH (including Scoop's shims), Program Files, and the per-user and Scoop locations. WSL's
/// `bash.exe` in System32 is not Git Bash and never chosen. `None` elsewhere.
pub fn find_git_bash() -> Option<Utf8PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    if let Some(p) = std::env::var("CLAUDE_CODE_GIT_BASH_PATH").ok().filter(|s| !s.is_empty()) {
        let p = Utf8PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let mut candidates: Vec<Utf8PathBuf> = Vec::new();
    if let Some(git) = std::env::var("PATH").ok().and_then(|p| which_in("git", &p)) {
        if let Some(root) = git.parent().and_then(|d| d.parent()) {
            candidates.push(root.join("bin").join("bash.exe"));
            // Scoop: <scoop>\shims\git.exe → <scoop>\apps\git\current\bin\bash.exe
            candidates.push(root.join("apps").join("git").join("current").join("bin").join("bash.exe"));
        }
    }
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Ok(pf) = std::env::var(var) {
            candidates.push(Utf8PathBuf::from(pf).join("Git").join("bin").join("bash.exe"));
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(Utf8PathBuf::from(local).join("Programs").join("Git").join("bin").join("bash.exe"));
    }
    if let Some(home) = paths::home_dir() {
        candidates.push(home.join("scoop").join("apps").join("git").join("current").join("bin").join("bash.exe"));
    }
    candidates.into_iter().find(|c| c.is_file() && !c.as_str().to_ascii_lowercase().contains("system32"))
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

/// Looks a program up in a PATH string. On Windows `claude` finds `claude.exe` before
/// `claude.cmd`, and an extension-less file (a Unix shell script) isn't taken for a program.
pub fn which_in(program: &str, path: &str) -> Option<Utf8PathBuf> {
    let has_ext = Utf8Path::new(program).extension().is_some();
    let exts: &[&str] = match (cfg!(windows), has_ext) {
        (true, false) => &[".exe", ".cmd", ".bat"],
        _ => &[""],
    };
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
    // Windows: tell the CLI where Git Bash is, so its shell tool exists (docs/decisions.md).
    if cfg!(windows) && !env.contains_key("CLAUDE_CODE_GIT_BASH_PATH") {
        if let Some(b) = find_git_bash() {
            env.insert("CLAUDE_CODE_GIT_BASH_PATH".into(), b.to_string());
        }
    }
    env
}

fn run_with_env(
    exe: &Utf8Path,
    args: &[&str],
    login: Option<&str>,
    timeout: Duration,
) -> Option<(i32, String, String)> {
    let mut cmd = crate::platform::command(exe);
    cmd.args(args).env_clear().envs(child_env(login));
    run_capture(&mut cmd, timeout).ok()
}

#[cfg(not(windows))]
const CLAUDE_LOCATIONS: [&str; 4] =
    ["~/.claude/local/claude", "~/.local/bin/claude", "/opt/homebrew/bin/claude", "/usr/local/bin/claude"];

/// Windows (ui/spec.md §23.5): after the configured path and `PATH`, the native installer's
/// location, then npm's global shim.
#[cfg(windows)]
fn claude_locations() -> Vec<String> {
    let mut v = vec!["~\\.local\\bin\\claude.exe".to_string()];
    if let Ok(appdata) = std::env::var("APPDATA") {
        v.push(format!("{appdata}\\npm\\claude.cmd"));
    }
    v
}

#[cfg(not(windows))]
fn claude_locations() -> Vec<String> {
    CLAUDE_LOCATIONS.iter().map(|s| s.to_string()).collect()
}

/// Finds Claude Code. `VIDEO_STUDIO_CLAUDE` overrides everything (tests use a fake CLI), and
/// `VIDEO_STUDIO_NO_CLAUDE` makes detection fail (to exercise the setup screen).
pub fn find_claude(custom_path: &str) -> Option<ClaudeInfo> {
    if std::env::var_os("VIDEO_STUDIO_NO_CLAUDE").is_some() {
        return None;
    }
    let login = login_path();
    let forced = std::env::var("VIDEO_STUDIO_CLAUDE").ok().filter(|s| !s.is_empty());
    let common = claude_locations();
    let common: Vec<&str> = common.iter().map(String::as_str).collect();
    let list = match &forced {
        Some(f) => vec![Utf8PathBuf::from(f)],
        None => candidates("claude", custom_path, &common),
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
            git_bash: find_git_bash().map(|b| b.to_string()),
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

/// Where Claude Code writes the output of a command it moved to the background, which happens
/// to any command that outlasts its timeout (a long `cutline render`): a folder for the working
/// directory under `/tmp/claude-<user id>`. Claude reads that file back to see how the command
/// ended, so a run's policy has to allow reading it.
pub fn claude_task_dirs(cwd: &Utf8Path) -> Vec<Utf8PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Some(uid) = paths::home_dir().and_then(|h| std::fs::metadata(h).ok()).map(|m| m.uid()) else {
            return Vec::new();
        };
        let slug: String = cwd.as_str().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let mut roots = vec![Utf8PathBuf::from("/tmp"), Utf8PathBuf::from("/private/tmp")];
        if let Some(t) = std::env::var("CLAUDE_CODE_TMPDIR").ok().filter(|s| !s.is_empty()) {
            roots.push(Utf8PathBuf::from(t));
        }
        roots.into_iter().map(|r| r.join(format!("claude-{uid}")).join(&slug)).collect()
    }
    #[cfg(not(unix))]
    {
        // On Windows the output goes to `%TEMP%\claude\<hash>\<session>\tasks\<id>.output`
        // (seen with Claude Code on Windows 11). The hash stands for the working directory but
        // isn't a documented function of it, so the whole `claude` folder under the temporary
        // folder is allowed: it holds only Claude Code's own scratch files for this user.
        let _ = cwd;
        let mut roots: Vec<Utf8PathBuf> = Utf8PathBuf::from_path_buf(std::env::temp_dir()).into_iter().collect();
        if let Some(t) = std::env::var("CLAUDE_CODE_TMPDIR").ok().filter(|s| !s.is_empty()) {
            roots.push(Utf8PathBuf::from(t));
        }
        roots.into_iter().map(|r| r.join("claude")).collect()
    }
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
    let common: &[&str] = if cfg!(windows) {
        &[]
    } else {
        &["~/.local/bin/cutline", "/opt/homebrew/bin/cutline", "/usr/local/bin/cutline"]
    };
    let mut list = match &forced {
        Some(f) => vec![Utf8PathBuf::from(f)],
        None => candidates("cutline", custom_path, common),
    };
    if forced.is_none() && !repo.trim().is_empty() {
        let repo = paths::expand_tilde(repo.trim());
        let exe = if cfg!(windows) { ".exe" } else { "" };
        list.push(repo.join(format!("bin/cutline{exe}")));
        list.push(repo.join(format!("target/release/cutline{exe}")));
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
        assert!(dir.ends_with("projects/-private-tmp-My-Runs-2026-11-14-1753-shorts"), "{dir}");
        // Claude Code's slug for a Windows folder: `C:\GitHub\video-editor` → `C--GitHub-video-editor`
        let win = claude_project_dir(r"C:\Runs\2026-11-14-1753-shorts".into()).unwrap();
        assert!(win.ends_with("projects/C--Runs-2026-11-14-1753-shorts"), "{win}");
    }

    #[cfg(windows)]
    #[test]
    fn background_command_output_is_under_the_temporary_folder() {
        let dirs = claude_task_dirs(r"C:\Runs\r1".into());
        let temp = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap();
        assert_eq!(dirs.first(), Some(&temp.join("claude")), "{dirs:?}");
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
