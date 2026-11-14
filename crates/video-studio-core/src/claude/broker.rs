//! The permission broker (spec §4.3, §12.2, §19). Every tool call Claude makes is checked against
//! a [`Policy`]; the prompt alone is never trusted.
//!
//! The policy is enforced twice: by a PreToolUse hook (`video-studio hook`), which sees every
//! tool call including the ones Claude Code would auto-approve (plain `ls`, for example), and by
//! the host when the CLI asks for permission over stdio. Both call [`Policy::decide`].

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::paths;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Spec assistant: edits `spec.md` only, no shell.
    Assistant,
    /// Dry run: may validate and review, never render finals or generate audio.
    DryRun,
    Production,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(String),
    /// AskUserQuestion: the host shows the question and answers it.
    AskUser,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Policy {
    pub mode: Mode,
    /// The run folder (or the assistant's scratch folder): Claude's working directory.
    pub work_dir: Utf8PathBuf,
    /// Extra folders and files Claude may read: media folders, brand assets, the cutline home.
    #[serde(default)]
    pub read_paths: Vec<Utf8PathBuf>,
    /// When set, the only files Claude may write (the assistant: `spec.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_files: Option<Vec<Utf8PathBuf>>,
    /// The configured cutline binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutline: Option<Utf8PathBuf>,
    /// The app's helper (`video-studio`); only its `eleven` subcommand is allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper: Option<Utf8PathBuf>,
    /// PATH folders used to resolve bare command names, in order.
    #[serde(default)]
    pub path_dirs: Vec<Utf8PathBuf>,
    #[serde(default)]
    pub web_access: bool,
    /// Where the hook appends its decisions (`logs/permissions.ndjson`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<Utf8PathBuf>,
}

/// cutline subcommands a run may use. `deps`, `cache` and `selftest` change or exercise the
/// installation and are never needed to make a video.
const CUTLINE_ALLOWED: [&str; 18] = [
    "validate", "schema", "init", "expand", "cues", "beats", "render", "preview", "templates", "review", "thumbnail", "chapters", "guide", "qa", "probe", "transcribe", "doctor", "record",
];
/// Not available in a dry run: final renders and anything that records or renders deliverables.
const CUTLINE_DRY_RUN_DENIED: [&str; 3] = ["render", "thumbnail", "record"];
/// Global cutline options that take a value.
const CUTLINE_VALUE_FLAGS: [&str; 2] = ["--variant", "--locale"];

impl Policy {
    pub fn decide(&self, tool: &str, input: &Value) -> Decision {
        match tool {
            "AskUserQuestion" => Decision::AskUser,
            "Read" | "Glob" | "Grep" | "NotebookRead" => self.decide_read(tool, input),
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => self.decide_write(input),
            "Bash" => self.decide_bash(input),
            "WebSearch" | "WebFetch" => {
                if self.web_access {
                    Decision::Allow
                } else {
                    Decision::Deny("Web access is off for this run.".into())
                }
            }
            // Planning and delegation tools touch nothing themselves; what a subagent does comes
            // back through this broker one tool call at a time.
            "TodoWrite" | "Task" | "Agent" | "TaskOutput" | "ExitPlanMode" => Decision::Allow,
            other => Decision::Deny(format!("The {other} tool isn't available here.")),
        }
    }

    fn resolve(&self, path: &str) -> Utf8PathBuf {
        paths::absolutize(&paths::expand_tilde(path), &self.work_dir)
    }

    fn may_read(&self, p: &Utf8Path) -> bool {
        paths::is_inside(p, &self.work_dir) || self.read_paths.iter().any(|r| paths::is_inside(p, r))
    }

    fn decide_read(&self, tool: &str, input: &Value) -> Decision {
        let mut targets: Vec<Utf8PathBuf> = Vec::new();
        for key in ["file_path", "notebook_path", "path"] {
            if let Some(p) = input[key].as_str().filter(|p| !p.is_empty()) {
                targets.push(self.resolve(p));
            }
        }
        // An absolute glob pattern searches outside the working folder: check its fixed prefix.
        if tool == "Glob" {
            if let Some(pat) = input["pattern"].as_str() {
                if pat.starts_with('/') || pat.starts_with('~') || pat.contains("..") {
                    let fixed: String = pat.split(['*', '?', '[', '{']).next().unwrap_or("").to_string();
                    targets.push(self.resolve(&fixed));
                }
            }
        }
        for t in &targets {
            if !self.may_read(t) {
                return Decision::Deny(format!("Reading {t} isn't allowed: it is outside this run's folders."));
            }
        }
        Decision::Allow
    }

    fn decide_write(&self, input: &Value) -> Decision {
        let Some(path) = ["file_path", "notebook_path", "path"].iter().find_map(|k| input[*k].as_str()).filter(|p| !p.is_empty()) else {
            return Decision::Deny("The edit has no file path.".into());
        };
        let target = self.resolve(path);
        // The app's own records stay out of Claude's hands. Checked on the path as written, so a
        // symlink in `.bin` is protected as well as the file it points at.
        if self.write_files.is_none() {
            let lexical = paths::normalize(&target);
            for work in [paths::normalize(&self.work_dir), paths::canonical_or_normal(&self.work_dir)] {
                if let Ok(rel) = lexical.strip_prefix(&work) {
                    let first = rel.components().next().map(|c| c.as_str()).unwrap_or("");
                    if first == "logs" || first == ".bin" || rel == "run.json" || rel == "media-manifest.json" {
                        return Decision::Deny(format!("{rel} is written by the app and is read-only."));
                    }
                }
            }
        }
        if !paths::is_inside(&target, &self.work_dir) {
            return Decision::Deny(format!("Writing {target} isn't allowed: only files in {} can be written.", self.work_dir));
        }
        if let Some(files) = &self.write_files {
            let t = paths::canonical_or_normal_deep(&target);
            if !files.iter().any(|f| paths::canonical_or_normal_deep(f) == t) {
                let names: Vec<&str> = files.iter().filter_map(|f| f.file_name()).collect();
                return Decision::Deny(format!("Only {} can be edited here.", names.join(", ")));
            }
        }
        Decision::Allow
    }

    fn decide_bash(&self, input: &Value) -> Decision {
        if self.mode == Mode::Assistant {
            return Decision::Deny("Shell commands aren't available in the spec assistant.".into());
        }
        let Some(command) = input["command"].as_str() else { return Decision::Deny("The shell command is empty.".into()) };
        if input["run_in_background"].as_bool() == Some(true) {
            return Decision::Deny("Background commands aren't allowed; run cutline in the foreground.".into());
        }
        let segments = match split_commands(command) {
            Ok(s) => s,
            Err(why) => return Decision::Deny(format!("{why} Run cutline (or `video-studio eleven`) as a plain command, one per call, with no pipes, redirections or substitutions.")),
        };
        if segments.is_empty() {
            return Decision::Deny("The shell command is empty.".into());
        }
        for argv in &segments {
            if let Err(why) = self.check_command(argv) {
                return Decision::Deny(why);
            }
        }
        Decision::Allow
    }

    /// The real path of a command's executable: a path as given, or a bare name found on PATH.
    fn resolve_executable(&self, arg0: &str) -> Option<Utf8PathBuf> {
        let candidate = if arg0.contains('/') || arg0.contains('\\') {
            Some(self.resolve(arg0))
        } else {
            self.path_dirs.iter().map(|d| d.join(arg0)).find(|c| c.exists()).or_else(|| {
                if cfg!(windows) {
                    self.path_dirs.iter().map(|d| d.join(format!("{arg0}.exe"))).find(|c| c.exists())
                } else {
                    None
                }
            })
        }?;
        candidate.exists().then(|| paths::canonical_or_normal(&candidate))
    }

    fn check_command(&self, argv: &[String]) -> Result<(), String> {
        let arg0 = &argv[0];
        let only = "Only cutline and `video-studio eleven` can be run here.";
        let Some(exe) = self.resolve_executable(arg0) else { return Err(format!("`{arg0}` isn't an allowed command. {only}")) };
        let is = |bin: &Option<Utf8PathBuf>| bin.as_ref().map(|b| paths::canonical_or_normal(b) == exe).unwrap_or(false);
        // The helper is also installed as `.bin/cutline`, a logging proxy for the real cutline;
        // the name it is called by decides which it acts as.
        let called_as = Utf8Path::new(arg0).file_stem().unwrap_or("").to_ascii_lowercase();
        if is(&self.cutline) || (is(&self.helper) && called_as == "cutline") {
            return self.check_cutline(&argv[1..]);
        }
        if is(&self.helper) {
            return self.check_helper(&argv[1..]);
        }
        Err(format!("`{arg0}` isn't an allowed command. {only}"))
    }

    fn check_cutline(&self, args: &[String]) -> Result<(), String> {
        let mut sub: Option<&str> = None;
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            if CUTLINE_VALUE_FLAGS.contains(&a) {
                i += 2;
                continue;
            }
            if !a.starts_with('-') {
                sub = Some(a);
                break;
            }
            i += 1;
        }
        let Some(sub) = sub else {
            // `cutline --help` / `--version`
            return Ok(());
        };
        if !CUTLINE_ALLOWED.contains(&sub) {
            return Err(format!("`cutline {sub}` isn't available during a run."));
        }
        if self.mode == Mode::DryRun && CUTLINE_DRY_RUN_DENIED.contains(&sub) {
            return Err(format!("`cutline {sub}` isn't available in a dry run: a dry run validates and reviews, it never renders finals. Use `cutline validate`, `cutline expand`, `cutline cues` and `cutline review`."));
        }
        // Output paths must stay in the run folder.
        let rest = &args[i + 1..];
        let mut k = 0;
        while k < rest.len() {
            let a = rest[k].as_str();
            let value = if a == "-o" || a == "--out" {
                k += 1;
                rest.get(k).map(String::as_str)
            } else {
                a.strip_prefix("--out=").or_else(|| a.strip_prefix("-o="))
            };
            if let Some(v) = value {
                let p = self.resolve(v);
                if !paths::is_inside(&p, &self.work_dir) {
                    return Err(format!("cutline may only write inside the run folder; `{v}` is outside it."));
                }
            }
            k += 1;
        }
        Ok(())
    }

    fn check_helper(&self, args: &[String]) -> Result<(), String> {
        if args.first().map(String::as_str) != Some("eleven") {
            return Err("Only the `eleven` subcommand of video-studio can be run here.".into());
        }
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        if !["tts", "music", "sfx", "scribe", "voices", "quote", "--help", "-h", "help"].contains(&sub) {
            return Err(format!("`video-studio eleven {sub}` isn't a helper command (tts, music, sfx, scribe, voices, quote)."));
        }
        if self.mode == Mode::DryRun && !["voices", "quote", "--help", "-h", "help"].contains(&sub) {
            return Err("Audio isn't generated in a dry run. Use `video-studio eleven quote …` to estimate credits instead.".into());
        }
        Ok(())
    }
}

/// Splits a shell command line into simple commands joined by `&&` or `;`, each as an argv.
/// Anything a plain command doesn't need is rejected: pipes, redirections (except `2>&1`),
/// substitutions, variables, globs outside quotes are passed through literally, background jobs,
/// subshells and environment assignments.
pub fn split_commands(command: &str) -> Result<Vec<Vec<String>>, String> {
    let mut segments: Vec<Vec<String>> = Vec::new();
    let mut argv: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut has_cur = false;
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    let reject = |what: &str| Err(format!("The command uses {what}, which isn't allowed."));
    macro_rules! push_word {
        () => {
            if has_cur {
                argv.push(std::mem::take(&mut cur));
                has_cur = false;
            }
        };
    }
    macro_rules! end_segment {
        () => {
            push_word!();
            if argv.is_empty() {
                return Err("The command has an empty part.".to_string());
            }
            segments.push(std::mem::take(&mut argv));
        };
    }
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => push_word!(),
            '\n' | '\r' => {
                push_word!();
                if !argv.is_empty() {
                    segments.push(std::mem::take(&mut argv));
                }
            }
            '\'' => {
                has_cur = true;
                i += 1;
                loop {
                    match chars.get(i) {
                        Some('\'') => break,
                        Some(ch) => cur.push(*ch),
                        None => return Err("The command has an unclosed quote.".into()),
                    }
                    i += 1;
                }
            }
            '"' => {
                has_cur = true;
                i += 1;
                loop {
                    match chars.get(i) {
                        Some('"') => break,
                        Some('\\') => {
                            i += 1;
                            match chars.get(i) {
                                Some(ch) => cur.push(*ch),
                                None => return Err("The command has an unclosed quote.".into()),
                            }
                        }
                        Some('$') => return reject("a variable or command substitution"),
                        Some('`') => return reject("a command substitution"),
                        Some(ch) => cur.push(*ch),
                        None => return Err("The command has an unclosed quote.".into()),
                    }
                    i += 1;
                }
            }
            '\\' => {
                i += 1;
                match chars.get(i) {
                    Some('\n') => {}
                    Some(ch) => {
                        cur.push(*ch);
                        has_cur = true;
                    }
                    None => return Err("The command ends with a backslash.".into()),
                }
            }
            '&' => {
                if chars.get(i + 1) == Some(&'&') {
                    end_segment!();
                    i += 1;
                } else {
                    return reject("a background job (&)");
                }
            }
            ';' => {
                end_segment!();
            }
            '|' => return reject(if chars.get(i + 1) == Some(&'|') { "`||`" } else { "a pipe" }),
            '>' | '<' => {
                // `2>&1` is harmless and Claude adds it out of habit: accept and drop it.
                let merges_stderr = c == '>' && cur == "2" && has_cur && chars.get(i + 1) == Some(&'&') && chars.get(i + 2) == Some(&'1') && chars.get(i + 3).map(|n| n.is_whitespace() || *n == ';' || *n == '&').unwrap_or(true);
                if merges_stderr {
                    cur.clear();
                    has_cur = false;
                    i += 2;
                } else {
                    return reject("a redirection");
                }
            }
            '$' => return reject("a variable or command substitution"),
            '`' => return reject("a command substitution"),
            '(' | ')' | '{' | '}' => return reject("a subshell or group"),
            '#' if !has_cur => {
                // comment to end of line
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            other => {
                cur.push(other);
                has_cur = true;
            }
        }
        i += 1;
    }
    if has_cur {
        argv.push(cur);
    }
    if !argv.is_empty() {
        segments.push(argv);
    }
    for seg in &segments {
        // `FOO=bar cutline …` would change what the command does.
        let first = &seg[0];
        if let Some(eq) = first.find('=') {
            if eq > 0 && first[..eq].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return reject("an environment assignment");
            }
        }
        // `cd` into the run folder is the one builtin Claude reaches for; it changes nothing.
    }
    Ok(segments)
}

/// Shell-quotes one word for the hook command line.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=+,@%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The `--settings` JSON that installs the PreToolUse hook for a run.
pub fn hook_settings(helper: &Utf8Path, policy_file: &Utf8Path) -> Value {
    let command = format!("{} hook --policy {}", shell_quote(helper.as_str()), shell_quote(policy_file.as_str()));
    json!({"hooks": {"PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": command, "timeout": 20}]}]}})
}

/// Handles one PreToolUse hook call: `input` is the JSON Claude Code writes to the hook's stdin.
/// Returns the JSON to print. AskUserQuestion gets no decision, so it reaches the host.
pub fn hook_response(policy: &Policy, input: &Value) -> Value {
    let tool = input["tool_name"].as_str().unwrap_or("");
    let decision = policy.decide(tool, &input["tool_input"]);
    if let Some(log) = &policy.log {
        let (verdict, reason) = match &decision {
            Decision::Allow => ("allow", None),
            Decision::Deny(r) => ("deny", Some(r.as_str())),
            Decision::AskUser => ("ask_user", None),
        };
        let entry = json!({"ts": crate::timeutil::now_rfc3339(), "source": "hook", "tool": tool, "input": input["tool_input"], "decision": verdict, "reason": reason});
        append_line(log, &entry.to_string());
    }
    let out = |decision: &str, reason: &str| json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": decision, "permissionDecisionReason": reason}});
    match decision {
        Decision::Allow => out("allow", "Allowed by Syncfusion Video Studio"),
        Decision::Deny(reason) => out("deny", &format!("{reason} This is blocked by Syncfusion Video Studio.")),
        Decision::AskUser => json!({}),
    }
}

/// Appends one line to a log file, creating it if needed. Failures are ignored: a log that can't
/// be written must not stop a run.
pub fn append_line(path: &Utf8Path, line: &str) {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        _dir: tempfile::TempDir,
        run: Utf8PathBuf,
        media: Utf8PathBuf,
        outside: Utf8PathBuf,
        policy: Policy,
    }

    fn fx(mode: Mode) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let root = paths::canonical_or_normal(Utf8Path::from_path(dir.path()).unwrap());
        let run = root.join("run");
        let media = root.join("footage");
        let outside = root.join("elsewhere");
        let tools = root.join("tools");
        for d in [&run, &media, &outside, &tools, &run.join(".bin"), &run.join("logs")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let cutline = tools.join("cutline");
        let helper = tools.join("video-studio");
        let rogue = tools.join("rm");
        for b in [&cutline, &helper, &rogue] {
            std::fs::write(b, "#!/bin/sh\n").unwrap();
        }
        // the run's .bin holds the names Claude types: `cutline` is the helper acting as a proxy
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&helper, run.join(".bin/cutline")).unwrap();
            std::os::unix::fs::symlink(&helper, run.join(".bin/video-studio")).unwrap();
        }
        std::fs::write(media.join("hero.mov"), b"x").unwrap();
        std::fs::write(outside.join("secret.txt"), b"x").unwrap();
        let policy = Policy {
            mode,
            work_dir: run.clone(),
            read_paths: vec![media.clone()],
            write_files: if mode == Mode::Assistant { Some(vec![run.join("spec.md")]) } else { None },
            cutline: Some(cutline),
            helper: Some(helper),
            path_dirs: vec![run.join(".bin"), tools],
            web_access: false,
            log: None,
        };
        Fx { _dir: dir, run, media, outside, policy }
    }

    fn bash(p: &Policy, cmd: &str) -> Decision {
        p.decide("Bash", &json!({"command": cmd}))
    }
    fn denied(d: Decision) -> String {
        match d {
            Decision::Deny(r) => r,
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    #[test]
    fn splits_plain_commands() {
        assert_eq!(split_commands("cutline validate --json project.json5").unwrap(), vec![vec!["cutline", "validate", "--json", "project.json5"]]);
        assert_eq!(split_commands("cutline validate 'my project.json5' && cutline review \"a b\" 2>&1").unwrap(), vec![vec!["cutline", "validate", "my project.json5"], vec!["cutline", "review", "a b"]]);
        assert_eq!(split_commands("cutline render p.json5; cutline qa out.mp4\n").unwrap().len(), 2);
        assert_eq!(split_commands("video-studio eleven tts --text 'it''s $5 & more'").unwrap()[0][4], "its $5 & more");
        for bad in ["cutline validate | head", "cutline render > log.txt", "cutline render $(pwd)", "cutline `id`", "cutline render &", "X=1 cutline render", "cutline a || rm b", "(cutline a)", "cutline \"$HOME\"", "cutline 'open", "cutline render 2>err.txt", "cat < f"] {
            assert!(split_commands(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[cfg(unix)]
    #[test]
    fn production_allows_only_cutline_and_the_helper() {
        let f = fx(Mode::Production);
        let p = &f.policy;
        assert_eq!(bash(p, "cutline validate --json project.json5"), Decision::Allow);
        assert_eq!(bash(p, "cutline --json --variant teaser render project.json5 --out outputs/x.mp4"), Decision::Allow);
        assert_eq!(bash(p, &format!("{} review project.json5 2>&1", p.cutline.as_ref().unwrap())), Decision::Allow, "the real binary by full path");
        assert_eq!(bash(p, "cutline validate project.json5 && cutline review project.json5"), Decision::Allow);
        assert_eq!(bash(p, "video-studio eleven tts --voice v1 --text-file vo.txt --out audio/vo.mp3"), Decision::Allow);
        assert_eq!(bash(p, "cutline --version"), Decision::Allow);
        for cmd in ["ls", "ls -la", "rm -rf outputs", "cat project.json5", "python3 -c 'print(1)'", "cd /tmp && cutline render p.json5", "ffmpeg -i a b", "./rm x", "curl https://example.com", "bash -c 'cutline render p.json5'"] {
            let why = denied(bash(p, cmd));
            assert!(why.contains("Only cutline") || why.contains("isn't allowed"), "{cmd}: {why}");
        }
        assert!(denied(bash(p, "cutline validate p.json5 | tail -5")).contains("pipe"));
        assert!(denied(bash(p, "cutline deps install --all")).contains("isn't available during a run"));
        assert!(denied(bash(p, "cutline selftest")).contains("isn't available"));
        assert!(denied(bash(p, &format!("cutline render p.json5 --out {}/x.mp4", f.outside))).contains("inside the run folder"));
        assert!(denied(bash(p, "cutline render p.json5 -o ../x.mp4")).contains("inside the run folder"));
        assert!(denied(bash(p, "video-studio run other.cutproj")).contains("eleven"));
        assert!(denied(bash(p, "video-studio secret get")).contains("eleven"));
        assert!(denied(p.decide("Bash", &json!({"command": "cutline render p.json5", "run_in_background": true}))).contains("Background"));
        // a look-alike binary elsewhere isn't cutline
        let fake = f.outside.join("cutline");
        std::fs::write(&fake, "#!/bin/sh\n").unwrap();
        assert!(denied(bash(p, &format!("{fake} render p.json5"))).contains("isn't an allowed command"));
    }

    #[cfg(unix)]
    #[test]
    fn dry_run_cannot_render_or_generate_audio() {
        let f = fx(Mode::DryRun);
        let p = &f.policy;
        for ok in ["cutline validate --json project.json5", "cutline expand project.json5", "cutline cues project.json5", "cutline review project.json5", "video-studio eleven quote tts --text-file vo.txt", "video-studio eleven voices"] {
            assert_eq!(bash(p, ok), Decision::Allow, "{ok}");
        }
        assert!(denied(bash(p, "cutline render project.json5")).contains("dry run"));
        assert!(denied(bash(p, "cutline --json render --draft project.json5")).contains("dry run"));
        assert!(denied(bash(p, "cutline validate p.json5 && cutline render p.json5")).contains("dry run"));
        assert!(denied(bash(p, "cutline thumbnail project.json5")).contains("dry run"));
        for cmd in ["video-studio eleven tts --voice v --text-file t --out a.mp3", "video-studio eleven music --prompt x --length 10 --out m.mp3", "video-studio eleven sfx --prompt x --length 2 --out s.mp3", "video-studio eleven scribe --in a.mp3 --out a.json"] {
            assert!(denied(bash(p, cmd)).contains("dry run"), "{cmd}");
        }
    }

    #[test]
    fn file_scope() {
        let f = fx(Mode::Production);
        let p = &f.policy;
        let read = |path: String| p.decide("Read", &json!({"file_path": path}));
        let write = |path: String| p.decide("Write", &json!({"file_path": path, "content": "x"}));
        assert_eq!(read("spec.md".into()), Decision::Allow);
        assert_eq!(read(f.media.join("hero.mov").to_string()), Decision::Allow);
        assert!(denied(read(f.outside.join("secret.txt").to_string())).contains("outside"));
        assert!(denied(read("../elsewhere/secret.txt".into())).contains("outside"));
        assert_eq!(p.decide("Glob", &json!({"pattern": "**/*.json5"})), Decision::Allow);
        assert_eq!(p.decide("Grep", &json!({"pattern": "TODO", "path": f.media.to_string()})), Decision::Allow);
        assert!(matches!(p.decide("Glob", &json!({"pattern": format!("{}/**/*", f.outside)})), Decision::Deny(_)));
        assert!(matches!(p.decide("Grep", &json!({"pattern": "key", "path": "/etc"})), Decision::Deny(_)));
        assert_eq!(write("project.json5".into()), Decision::Allow);
        assert_eq!(write(f.run.join("work/notes.md").to_string()), Decision::Allow);
        assert_eq!(p.decide("Edit", &json!({"file_path": "summary.md", "old_string": "a", "new_string": "b"})), Decision::Allow);
        assert!(denied(write(f.media.join("hero.mov").to_string())).contains("only files in"), "media is read-only");
        assert!(denied(write(f.outside.join("x.txt").to_string())).contains("only files in"));
        assert!(denied(write("../escape.txt".into())).contains("only files in"));
        assert!(denied(write("logs/events.ndjson".into())).contains("read-only"));
        assert!(denied(write(".bin/cutline".into())).contains("read-only"));
        assert!(denied(write("run.json".into())).contains("read-only"));
        assert!(denied(p.decide("Write", &json!({"content": "x"}))).contains("no file path"));
        assert!(denied(p.decide("WebFetch", &json!({"url": "https://example.com"}))).contains("Web access is off"));
        let mut web = p.clone();
        web.web_access = true;
        assert_eq!(web.decide("WebSearch", &json!({"query": "x"})), Decision::Allow);
        assert_eq!(p.decide("AskUserQuestion", &json!({"questions": []})), Decision::AskUser);
        assert_eq!(p.decide("TodoWrite", &json!({})), Decision::Allow);
        assert!(denied(p.decide("KillShell", &json!({}))).contains("isn't available"));
    }

    #[test]
    fn assistant_edits_only_the_spec_and_has_no_shell() {
        let f = fx(Mode::Assistant);
        let p = &f.policy;
        assert_eq!(p.decide("Edit", &json!({"file_path": "spec.md", "old_string": "a", "new_string": "b"})), Decision::Allow);
        assert_eq!(p.decide("Write", &json!({"file_path": f.run.join("spec.md").to_string(), "content": "x"})), Decision::Allow);
        assert!(denied(p.decide("Write", &json!({"file_path": "other.md", "content": "x"}))).contains("Only spec.md"));
        assert!(denied(bash(p, "cutline validate project.json5")).contains("spec assistant"));
        assert_eq!(p.decide("Read", &json!({"file_path": f.media.join("hero.mov").to_string()})), Decision::Allow);
        assert!(matches!(p.decide("Read", &json!({"file_path": f.outside.join("secret.txt").to_string()})), Decision::Deny(_)));
    }

    #[cfg(unix)]
    #[test]
    fn hook_answers_and_logs() {
        let mut f = fx(Mode::Production);
        let log = f.run.join("logs/permissions.ndjson");
        f.policy.log = Some(log.clone());
        let allow = hook_response(&f.policy, &json!({"tool_name": "Bash", "tool_input": {"command": "cutline validate project.json5"}}));
        assert_eq!(allow["hookSpecificOutput"]["permissionDecision"], "allow");
        let deny = hook_response(&f.policy, &json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}));
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(deny["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains("blocked by Syncfusion Video Studio"));
        let ask = hook_response(&f.policy, &json!({"tool_name": "AskUserQuestion", "tool_input": {}}));
        assert_eq!(ask, json!({}), "questions pass through to the host");
        let lines: Vec<Value> = std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.iter().map(|l| l["decision"].as_str().unwrap()).collect::<Vec<_>>(), vec!["allow", "deny", "ask_user"]);
        assert!(lines[1]["reason"].as_str().unwrap().contains("Only cutline"));
        let settings = hook_settings("/Apps/Video Studio/video-studio".into(), "/lib/ctx/policy.json".into());
        assert_eq!(settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "'/Apps/Video Studio/video-studio' hook --policy /lib/ctx/policy.json");
        // the policy survives the trip through its file
        let text = serde_json::to_string(&f.policy).unwrap();
        assert_eq!(serde_json::from_str::<Policy>(&text).unwrap(), f.policy);
    }
}
