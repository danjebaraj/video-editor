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
    /// The app's helper (`video-studio`); only its `eleven` and `capture` subcommands are allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper: Option<Utf8PathBuf>,
    /// Whether the run may drive and record apps with `video-studio capture …`.
    #[serde(default)]
    pub capture: bool,
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
const CUTLINE_ALLOWED: [&str; 19] = [
    "validate",
    "schema",
    "init",
    "expand",
    "cues",
    "beats",
    "render",
    "preview",
    "templates",
    "review",
    "thumbnail",
    "chapters",
    "guide",
    "qa",
    "probe",
    "transcribe",
    "doctor",
    "record",
    "still",
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

    /// An absolute path for a path Claude gave: relative to the working folder, `~` expanded, and
    /// on Windows a Git Bash form (`/c/Runs/x`) turned into its Windows form.
    fn resolve(&self, path: &str) -> Utf8PathBuf {
        paths::absolutize(&paths::expand_tilde(&paths::from_shell_path(path)), &self.work_dir)
    }

    /// Whether a path or glob prefix points outside the working folder by itself: absolute, from
    /// home, a drive or share on Windows, or climbing out with `..`.
    fn escapes(&self, pat: &str) -> bool {
        pat.starts_with('/')
            || pat.starts_with('~')
            || pat.contains("..")
            || (cfg!(windows)
                && (pat.starts_with('\\') || pat.as_bytes().get(1) == Some(&b':') || Utf8Path::new(pat).has_root()))
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
                if self.escapes(pat) {
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
        let Some(path) =
            ["file_path", "notebook_path", "path"].iter().find_map(|k| input[*k].as_str()).filter(|p| !p.is_empty())
        else {
            return Decision::Deny("The edit has no file path.".into());
        };
        let target = self.resolve(path);
        // The app's own records stay out of Claude's hands. Checked on the path as written, so a
        // symlink in `.bin` is protected as well as the file it points at.
        if self.write_files.is_none() {
            let lexical = paths::normalize(&target);
            for work in [paths::normalize(&self.work_dir), paths::canonical_or_normal(&self.work_dir)] {
                if let Some(rel) = paths::strip_path_prefix(&lexical, &work) {
                    // Windows file names are case-insensitive: `LOGS\x` is `logs/x`.
                    let fold = |s: &str| if cfg!(windows) { s.to_lowercase() } else { s.to_string() };
                    let first = fold(rel.components().next().map(|c| c.as_str()).unwrap_or(""));
                    let whole = fold(rel.as_str());
                    if first == "logs"
                        || first == ".bin"
                        || first == crate::viewable::PREVIEWS_DIR
                        || whole == "run.json"
                        || whole == "media-manifest.json"
                    {
                        return Decision::Deny(format!("{rel} is written by the app and is read-only."));
                    }
                }
            }
        }
        if !paths::is_inside(&target, &self.work_dir) {
            return Decision::Deny(format!(
                "Writing {target} isn't allowed: only files in {} can be written.",
                self.work_dir
            ));
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
        if self.mode == Mode::Assistant && self.cutline.is_none() {
            return Decision::Deny("Shell commands aren't available in the spec assistant.".into());
        }
        let Some(command) = input["command"].as_str() else {
            return Decision::Deny("The shell command is empty.".into());
        };
        if input["run_in_background"].as_bool() == Some(true) {
            return Decision::Deny("Background commands aren't allowed; run cutline in the foreground.".into());
        }
        let segments = match split_commands(command) {
            Ok(s) => s,
            Err(why) if self.mode == Mode::Assistant => {
                return Decision::Deny(format!("{why} {}", self.still_usage()))
            }
            Err(why) => return Decision::Deny(format!("{why} Run cutline (or `video-studio eleven` / `video-studio capture`) as a plain command, one per call, with no pipes, redirections or substitutions.")),
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
    /// On Windows a name or path without an extension also finds the `.exe` (as Git Bash does),
    /// and a `.exe` is matched case-insensitively.
    fn resolve_executable(&self, arg0: &str) -> Option<Utf8PathBuf> {
        let with_exe = |p: Utf8PathBuf| -> Option<Utf8PathBuf> {
            if p.is_file() {
                return Some(p);
            }
            if cfg!(windows) && p.extension().is_none() {
                let e = Utf8PathBuf::from(format!("{p}.exe"));
                return e.is_file().then_some(e);
            }
            None
        };
        let candidate = if arg0.contains('/') || arg0.contains('\\') {
            with_exe(self.resolve(arg0))
        } else {
            self.path_dirs.iter().find_map(|d| with_exe(d.join(arg0)))
        }?;
        Some(paths::canonical_or_normal(&candidate))
    }

    /// Whether `exe` is the configured binary `bin`. On Unix `.bin/cutline` is a symlink and
    /// canonicalizes to the helper; on Windows it is a hard link (or a copy), so the file's
    /// identity decides, and a copy in the run's own `.bin` (which Claude can't write) counts.
    fn is_binary(&self, exe: &Utf8Path, bin: &Option<Utf8PathBuf>, bin_names: &[&str]) -> bool {
        let Some(b) = bin else { return false };
        if paths::same_path(&paths::canonical_or_normal(b), exe) {
            return true;
        }
        #[cfg(windows)]
        {
            if same_file::is_same_file(b, exe).unwrap_or(false) {
                return true;
            }
            let in_bin = exe.parent().map(|d| paths::same_path(d, &self.work_dir.join(".bin"))).unwrap_or(false);
            let stem = exe.file_stem().unwrap_or("").to_ascii_lowercase();
            if in_bin && bin_names.contains(&stem.as_str()) {
                return true;
            }
        }
        let _ = bin_names;
        false
    }

    fn check_command(&self, argv: &[String]) -> Result<(), String> {
        let arg0 = &argv[0];
        let only = if self.mode == Mode::Assistant {
            format!(
                "The spec assistant can only run `cutline still`, to make a JPEG copy of a HEIC/HEIF photo. {}",
                self.still_usage()
            )
        } else {
            "Only cutline, `video-studio eleven` and `video-studio capture` can be run here.".to_string()
        };
        let Some(exe) = self.resolve_executable(arg0) else {
            return Err(format!("`{arg0}` isn't an allowed command. {only}"));
        };
        let is_cutline = self.is_binary(&exe, &self.cutline, &[]);
        let is_helper = self.is_binary(&exe, &self.helper, &["cutline", "video-studio"]);
        // The helper is also installed as `.bin/cutline`, a logging proxy for the real cutline;
        // the name it is called by decides which it acts as.
        let called_as = Utf8Path::new(arg0).file_stem().unwrap_or("").to_ascii_lowercase();
        if is_cutline || (is_helper && called_as == "cutline") {
            return self.check_cutline(&argv[1..]);
        }
        if is_helper && self.mode != Mode::Assistant {
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
        if self.mode == Mode::Assistant && sub != "still" {
            return Err(format!("`cutline {sub}` isn't available in the spec assistant; only `cutline still` is, to make a JPEG copy of a HEIC/HEIF photo."));
        }
        if !CUTLINE_ALLOWED.contains(&sub) {
            return Err(format!("`cutline {sub}` isn't available during a run."));
        }
        if self.mode == Mode::DryRun && CUTLINE_DRY_RUN_DENIED.contains(&sub) {
            return Err(format!("`cutline {sub}` isn't available in a dry run: a dry run validates and reviews, it never renders finals. Use `cutline validate`, `cutline expand`, `cutline cues` and `cutline review`."));
        }
        let rest = &args[i + 1..];
        // The assistant's own, stricter check first: its message names the exact command.
        if self.mode == Mode::Assistant {
            self.check_still(rest)?;
        }
        // Output paths must stay in the run folder.
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
        if sub == "still" {
            self.check_still(rest)?;
        }
        Ok(())
    }

    /// The one command the spec assistant may run, written out the way it must be typed, for
    /// denial messages: Claude copies it instead of guessing again (BUG-0002).
    fn still_usage(&self) -> String {
        let cutline = self.cutline.as_ref().map(|c| shell_quote(c.as_str())).unwrap_or_else(|| "cutline".into());
        let copies = self.work_dir.join(crate::viewable::CLAUDE_COPIES_DIR);
        format!(
            "Run exactly this, one photo per command, with the photo's full path and a plain file name filled in and no variables, `$(…)`, `~`, pipes, `&&` or other folders: {cutline} still '<photo>' --out {}",
            shell_quote(copies.join("<name>.jpg").as_str())
        )
    }

    /// `cutline still <photo> --out <file>`: the photo must be one Claude may read; the spec
    /// assistant writes its copies only to `photo-copies/` in its folder.
    fn check_still(&self, rest: &[String]) -> Result<(), String> {
        const VALUE_FLAGS: [&str; 5] = ["-o", "--out", "--max-side", "--quality", "--format"];
        let mut input: Option<&str> = None;
        let mut out: Option<&str> = None;
        let mut k = 0;
        while k < rest.len() {
            let a = rest[k].as_str();
            if VALUE_FLAGS.contains(&a) {
                if a == "-o" || a == "--out" {
                    out = rest.get(k + 1).map(String::as_str);
                }
                k += 2;
                continue;
            }
            if let Some(v) = a.strip_prefix("--out=").or_else(|| a.strip_prefix("-o=")) {
                out = Some(v);
            } else if !a.starts_with('-') && input.is_none() {
                input = Some(a);
            }
            k += 1;
        }
        if let Some(i) = input {
            let p = self.resolve(i);
            if !self.may_read(&p) {
                return Err(format!("Reading {p} isn't allowed: it is outside this run's folders."));
            }
        }
        if self.mode == Mode::Assistant {
            let copies = self.work_dir.join(crate::viewable::CLAUDE_COPIES_DIR);
            let ok = out.map(|o| self.resolve(o)).is_some_and(|o| paths::is_inside(&o, &copies) && o != copies);
            if !ok {
                return Err(format!(
                    "`cutline still` must write its copy into {}/ in your working folder, with --out. {}",
                    crate::viewable::CLAUDE_COPIES_DIR,
                    self.still_usage()
                ));
            }
        }
        Ok(())
    }

    fn check_helper(&self, args: &[String]) -> Result<(), String> {
        if args.first().map(String::as_str) == Some("capture") {
            return self.check_capture(&args[1..]);
        }
        if args.first().map(String::as_str) == Some("standin") {
            let sub = args.get(1).map(String::as_str).unwrap_or("");
            if !["video", "audio", "--help", "-h", "help"].contains(&sub) {
                return Err(format!("`video-studio standin {sub}` isn't a command (video, audio)."));
            }
            if self.mode != Mode::DryRun {
                return Err("Stand-ins are only for dry runs. In a production run, record the footage with `video-studio capture …` and generate the audio with `video-studio eleven …`.".into());
            }
            return Ok(());
        }
        if args.first().map(String::as_str) != Some("eleven") {
            return Err(
                "Only the `eleven`, `capture` and `standin` subcommands of video-studio can be run here.".into()
            );
        }
        let sub = args.get(1).map(String::as_str).unwrap_or("");
        if !["tts", "music", "sfx", "scribe", "voices", "quote", "--help", "-h", "help"].contains(&sub) {
            return Err(format!(
                "`video-studio eleven {sub}` isn't a helper command (tts, music, sfx, scribe, voices, quote)."
            ));
        }
        if self.mode == Mode::DryRun && !["voices", "quote", "--help", "-h", "help"].contains(&sub) {
            return Err(
                "Audio isn't generated in a dry run. Use `video-studio eleven quote …` to estimate credits, and `video-studio standin audio --out audio/<name>.mp3 --seconds <n>` to hold the file's place in the edit."
                    .into(),
            );
        }
        Ok(())
    }
}

impl Policy {
    /// `video-studio capture …`: driving and recording apps. The helper itself decides which
    /// apps may be touched and where files go; here only the mode and the project's switch count.
    fn check_capture(&self, args: &[String]) -> Result<(), String> {
        let sub = args.first().map(String::as_str).unwrap_or("");
        const SUBS: [&str; 13] = [
            "check",
            "open",
            "window",
            "screenshot",
            "click",
            "move",
            "drag",
            "scroll",
            "type",
            "key",
            "menu",
            "wait",
            "record",
        ];
        if !SUBS.contains(&sub) && !["--help", "-h", "help"].contains(&sub) {
            return Err(format!("`video-studio capture {sub}` isn't a command ({}).", SUBS.join(", ")));
        }
        if !self.capture {
            return Err("App recording is turned off for this project, so apps can't be driven or recorded in this run. Use only the attached media, and say in summary.md which footage is missing. The user can turn it on under Project › App recording.".into());
        }
        if self.mode == Mode::DryRun && !["check", "--help", "-h", "help"].contains(&sub) {
            return Err("Apps aren't driven or recorded in a dry run. Plan the recordings (which app, which steps, which file under captures/), hold each one's place with `video-studio standin video --out captures/<name>.mov --seconds <n>`, and list them under `planned` in the readiness report; `video-studio capture check` shows whether recording is allowed.".into());
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
                            // As in bash: inside double quotes a backslash only escapes `"`, `\`,
                            // `$`, a backtick or a newline, and stays otherwise ("C:\Runs\x").
                            i += 1;
                            match chars.get(i) {
                                Some(ch @ ('"' | '\\' | '$' | '`')) => cur.push(*ch),
                                Some('\n') => {}
                                Some(ch) => {
                                    cur.push('\\');
                                    cur.push(*ch);
                                }
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
                let merges_stderr = c == '>'
                    && cur == "2"
                    && has_cur
                    && chars.get(i + 1) == Some(&'&')
                    && chars.get(i + 2) == Some(&'1')
                    && chars.get(i + 3).map(|n| n.is_whitespace() || *n == ';' || *n == '&').unwrap_or(true);
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
    // Plain paths: Git Bash, which runs hooks on Windows, mangles `\\?\` verbatim paths.
    let (helper, policy_file) = (paths::plain(helper), paths::plain(policy_file));
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
        let exe = if cfg!(windows) { ".exe" } else { "" };
        let cutline = tools.join(format!("cutline{exe}"));
        let helper = tools.join(format!("video-studio{exe}"));
        let rogue = tools.join(format!("rm{exe}"));
        for b in [&cutline, &helper, &rogue] {
            std::fs::write(b, "#!/bin/sh\n").unwrap();
        }
        // the run's .bin holds the names Claude types: `cutline` is the helper acting as a proxy
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&helper, run.join(".bin/cutline")).unwrap();
            std::os::unix::fs::symlink(&helper, run.join(".bin/video-studio")).unwrap();
        }
        // Windows has no symlinks without Developer Mode: the run folder gets hard links
        #[cfg(windows)]
        {
            std::fs::hard_link(&helper, run.join(".bin/cutline.exe")).unwrap();
            std::fs::hard_link(&helper, run.join(".bin/video-studio.exe")).unwrap();
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
            capture: true,
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
        assert_eq!(
            split_commands("cutline validate --json project.json5").unwrap(),
            vec![vec!["cutline", "validate", "--json", "project.json5"]]
        );
        assert_eq!(
            split_commands("cutline validate 'my project.json5' && cutline review \"a b\" 2>&1").unwrap(),
            vec![vec!["cutline", "validate", "my project.json5"], vec!["cutline", "review", "a b"]]
        );
        assert_eq!(split_commands("cutline render p.json5; cutline qa out.mp4\n").unwrap().len(), 2);
        assert_eq!(split_commands("video-studio eleven tts --text 'it''s $5 & more'").unwrap()[0][4], "its $5 & more");
        for bad in [
            "cutline validate | head",
            "cutline render > log.txt",
            "cutline render $(pwd)",
            "cutline `id`",
            "cutline render &",
            "X=1 cutline render",
            "cutline a || rm b",
            "(cutline a)",
            "cutline \"$HOME\"",
            "cutline 'open",
            "cutline render 2>err.txt",
            "cat < f",
        ] {
            assert!(split_commands(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn production_allows_only_cutline_and_the_helper() {
        let f = fx(Mode::Production);
        let p = &f.policy;
        assert_eq!(bash(p, "cutline validate --json project.json5"), Decision::Allow);
        assert_eq!(
            bash(p, "cutline --json --variant teaser render project.json5 --out outputs/x.mp4"),
            Decision::Allow
        );
        assert_eq!(
            // (forward slashes: unquoted backslashes are escapes to Git Bash, as to any shell)
            bash(p, &format!("{} review project.json5 2>&1", p.cutline.as_ref().unwrap().as_str().replace('\\', "/"))),
            Decision::Allow,
            "the real binary by full path"
        );
        assert_eq!(bash(p, "cutline validate project.json5 && cutline review project.json5"), Decision::Allow);
        assert_eq!(
            bash(p, "video-studio eleven tts --voice v1 --text-file vo.txt --out audio/vo.mp3"),
            Decision::Allow
        );
        assert_eq!(bash(p, "cutline --version"), Decision::Allow);
        for cmd in [
            "ls",
            "ls -la",
            "rm -rf outputs",
            "cat project.json5",
            "python3 -c 'print(1)'",
            "cd /tmp && cutline render p.json5",
            "ffmpeg -i a b",
            "./rm x",
            "curl https://example.com",
            "bash -c 'cutline render p.json5'",
        ] {
            let why = denied(bash(p, cmd));
            assert!(why.contains("Only cutline") || why.contains("isn't allowed"), "{cmd}: {why}");
        }
        assert!(denied(bash(p, "cutline validate p.json5 | tail -5")).contains("pipe"));
        assert!(denied(bash(p, "cutline deps install --all")).contains("isn't available during a run"));
        assert!(denied(bash(p, "cutline selftest")).contains("isn't available"));
        assert!(denied(bash(p, &format!("cutline render p.json5 --out {}/x.mp4", f.outside)))
            .contains("inside the run folder"));
        assert!(denied(bash(p, "cutline render p.json5 -o ../x.mp4")).contains("inside the run folder"));
        assert!(denied(bash(p, "video-studio run other.cutproj")).contains("eleven"));
        assert!(denied(bash(p, "video-studio secret get")).contains("eleven"));
        assert!(denied(p.decide("Bash", &json!({"command": "cutline render p.json5", "run_in_background": true})))
            .contains("Background"));
        // a look-alike binary elsewhere isn't cutline
        let fake = f.outside.join(if cfg!(windows) { "cutline.exe" } else { "cutline" });
        std::fs::write(&fake, "#!/bin/sh\n").unwrap();
        let fake_cmd = fake.as_str().replace('\\', "/");
        assert!(denied(bash(p, &format!("{fake_cmd} render p.json5"))).contains("isn't an allowed command"));
    }

    /// Claude Code runs Bash through Git Bash on Windows (docs/decisions.md, Windows port): the
    /// same command can name the binaries in POSIX, mixed or Windows form, with or without
    /// `.exe`, and `cmd`, PowerShell and `start` stay closed like any other program.
    #[cfg(windows)]
    #[test]
    fn git_bash_command_forms_on_windows() {
        let f = fx(Mode::Production);
        let p = &f.policy;
        let posix = |path: &Utf8Path| {
            let s = path.as_str().replace('\\', "/");
            format!("/{}{}", s[..1].to_ascii_lowercase(), &s[2..])
        };
        let real = p.cutline.clone().unwrap();
        let helper = p.helper.clone().unwrap();
        let mixed = real.as_str().replace('\\', "/");
        for ok in [
            "cutline validate project.json5".to_string(),
            "cutline.exe validate project.json5".to_string(),
            "CUTLINE.EXE validate project.json5".to_string(),
            "video-studio.exe eleven voices".to_string(),
            format!("{} validate project.json5", posix(&real)),
            format!("{} validate project.json5", posix(&real).trim_end_matches(".exe")),
            format!("{mixed} validate project.json5"),
            format!("\"{}\" validate project.json5", real),
            format!("'{}' eleven voices", helper),
            format!("{} validate p.json5", posix(&f.run.join(".bin/cutline.exe"))),
            "./.bin/cutline validate p.json5".to_string(),
            format!("cutline render p.json5 --out {}/outputs/x.mp4", posix(&f.run)),
            format!("cutline render p.json5 --out \"{}\"", f.run.join("outputs").join("y.mp4")),
            "cutline validate --json project.json5 2>&1".to_string(),
        ] {
            assert_eq!(bash(p, &ok), Decision::Allow, "{ok}");
        }
        for cmd in [
            "cmd /c dir",
            "cmd.exe /C cutline render p.json5",
            "powershell -Command Get-ChildItem",
            "pwsh -c ls",
            "start notepad",
            "explorer .",
            "rm.exe -rf outputs",
            "where cutline",
        ] {
            let why = denied(bash(p, cmd));
            assert!(why.contains("isn't an allowed command"), "{cmd}: {why}");
        }
        // an output path outside the run folder, in either form, is refused
        assert!(denied(bash(p, &format!("cutline render p.json5 --out {}/x.mp4", posix(&f.outside))))
            .contains("inside the run folder"));
        assert!(denied(bash(p, &format!("cutline render p.json5 --out \"{}\"", f.outside.join("x.mp4"))))
            .contains("inside the run folder"));
        assert!(denied(bash(p, "cutline render p.json5 -o C:/Windows/x.mp4")).contains("inside the run folder"));
        // a copy of the helper that isn't the run's own .bin is not the helper
        let copy = f.outside.join("video-studio.exe");
        std::fs::copy(&helper, &copy).unwrap();
        assert!(denied(bash(p, &format!("{} eleven voices", posix(&copy)))).contains("isn't an allowed command"));
    }

    /// Scope checks compare Windows paths case-insensitively and with either separator.
    #[cfg(windows)]
    #[test]
    fn windows_scope_is_case_insensitive() {
        let f = fx(Mode::Production);
        let p = &f.policy;
        let upper = f.media.join("HERO.MOV").as_str().to_uppercase();
        assert_eq!(p.decide("Read", &json!({"file_path": upper})), Decision::Allow);
        let fwd = f.run.join("work").join("notes.md").as_str().replace('\\', "/").to_lowercase();
        assert_eq!(p.decide("Write", &json!({"file_path": fwd})), Decision::Allow);
        let s = f.media.join("hero.mov").as_str().replace('\\', "/");
        let git_bash = format!("/{}{}", s[..1].to_ascii_lowercase(), &s[2..]);
        assert_eq!(p.decide("Read", &json!({"file_path": git_bash})), Decision::Allow);
        let secret = f.outside.join("secret.txt").as_str().to_lowercase().replace('\\', "/");
        assert!(denied(p.decide("Read", &json!({"file_path": secret}))).contains("outside"));
        assert!(denied(p.decide("Write", &json!({"file_path": r".BIN\cutline.exe"}))).contains("read-only"));
        assert!(denied(p.decide("Write", &json!({"file_path": "LOGS/events.ndjson"}))).contains("read-only"));
        assert!(matches!(p.decide("Glob", &json!({"pattern": "C:/Windows/**/*"})), Decision::Deny(_)));
        assert!(matches!(p.decide("Glob", &json!({"pattern": "/c/Windows/**"})), Decision::Deny(_)));
        assert!(matches!(p.decide("Grep", &json!({"pattern": "x", "path": r"\\server\share"})), Decision::Deny(_)));
    }

    #[test]
    fn capture_commands_follow_the_project_switch_and_the_mode() {
        let mut f = fx(Mode::Production);
        for ok in [
            "video-studio capture check",
            "video-studio capture open \"Syncfusion Video Studio\"",
            "video-studio capture screenshot --out look.png",
            "video-studio capture click 320 200 --double",
            "video-studio capture scroll 400 300 -5",
            "video-studio capture type 'Q4 launch'",
            "video-studio capture menu \"File\" \"New Project…\"",
            "video-studio capture record start --out scene1.mov --seconds 30 && video-studio capture wait 2",
            "video-studio capture record wait",
        ] {
            assert_eq!(bash(&f.policy, ok), Decision::Allow, "{ok}");
        }
        assert!(denied(bash(&f.policy, "video-studio capture shell ls")).contains("isn't a command"));
        // other ways to reach the same tools stay closed
        for cmd in ["osascript -e 'tell app \"Finder\" to quit'", "screencapture -v x.mov", "open -a Terminal"] {
            assert!(denied(bash(&f.policy, cmd)).contains("Only cutline"), "{cmd}");
        }
        f.policy.capture = false;
        assert!(denied(bash(&f.policy, "video-studio capture open Safari")).contains("turned off"));
        let dry = fx(Mode::DryRun);
        assert_eq!(bash(&dry.policy, "video-studio capture check"), Decision::Allow);
        assert!(denied(bash(&dry.policy, "video-studio capture record start --out a.mov")).contains("dry run"));
        assert!(denied(bash(&dry.policy, "video-studio capture click 1 2")).contains("dry run"));
        // stand-ins hold the place of run-made files, in dry runs only
        for ok in [
            "video-studio standin video --out captures/scene-2.mov --seconds 22 --size 1280x800",
            "video-studio standin audio --out audio/vo-1.mp3 --seconds 9.5",
        ] {
            assert_eq!(bash(&dry.policy, ok), Decision::Allow, "{ok}");
            assert!(denied(bash(&f.policy, ok)).contains("only for dry runs"), "{ok}");
        }
        assert!(denied(bash(&dry.policy, "video-studio standin image --out a.png")).contains("isn't a command"));
        assert!(denied(bash(&dry.policy, "video-studio eleven tts --text hi --out a.mp3")).contains("standin audio"));
    }

    #[test]
    fn dry_run_cannot_render_or_generate_audio() {
        let f = fx(Mode::DryRun);
        let p = &f.policy;
        for ok in [
            "cutline validate --json project.json5",
            "cutline expand project.json5",
            "cutline cues project.json5",
            "cutline review project.json5",
            "video-studio eleven quote tts --text-file vo.txt",
            "video-studio eleven voices",
        ] {
            assert_eq!(bash(p, ok), Decision::Allow, "{ok}");
        }
        assert!(denied(bash(p, "cutline render project.json5")).contains("dry run"));
        assert!(denied(bash(p, "cutline --json render --draft project.json5")).contains("dry run"));
        assert!(denied(bash(p, "cutline validate p.json5 && cutline render p.json5")).contains("dry run"));
        assert!(denied(bash(p, "cutline thumbnail project.json5")).contains("dry run"));
        for cmd in [
            "video-studio eleven tts --voice v --text-file t --out a.mp3",
            "video-studio eleven music --prompt x --length 10 --out m.mp3",
            "video-studio eleven sfx --prompt x --length 2 --out s.mp3",
            "video-studio eleven scribe --in a.mp3 --out a.json",
        ] {
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
        assert_eq!(
            p.decide("Edit", &json!({"file_path": "summary.md", "old_string": "a", "new_string": "b"})),
            Decision::Allow
        );
        assert!(denied(write(f.media.join("hero.mov").to_string())).contains("only files in"), "media is read-only");
        assert!(denied(write(f.outside.join("x.txt").to_string())).contains("only files in"));
        assert!(denied(write("../escape.txt".into())).contains("only files in"));
        assert!(denied(write("logs/events.ndjson".into())).contains("read-only"));
        assert!(denied(write(".bin/cutline".into())).contains("read-only"));
        assert!(denied(write("run.json".into())).contains("read-only"));
        assert!(denied(write("media-previews/photo.jpg".into())).contains("read-only"), "the app makes the previews");
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
        assert_eq!(
            p.decide("Edit", &json!({"file_path": "spec.md", "old_string": "a", "new_string": "b"})),
            Decision::Allow
        );
        assert_eq!(
            p.decide("Write", &json!({"file_path": f.run.join("spec.md").to_string(), "content": "x"})),
            Decision::Allow
        );
        assert!(denied(p.decide("Write", &json!({"file_path": "other.md", "content": "x"}))).contains("Only spec.md"));
        assert!(denied(bash(p, "cutline validate project.json5")).contains("spec assistant"));
        assert_eq!(p.decide("Read", &json!({"file_path": f.media.join("hero.mov").to_string()})), Decision::Allow);
        assert!(matches!(
            p.decide("Read", &json!({"file_path": f.outside.join("secret.txt").to_string()})),
            Decision::Deny(_)
        ));
    }

    /// BUG-0001: the assistant may run `cutline still` (only that) to copy a HEIC/HEIF it may
    /// read into `photo-copies/`; without cutline it has no shell at all.
    #[test]
    fn assistant_may_copy_heic_photos_with_cutline_still() {
        let f = fx(Mode::Assistant);
        let p = &f.policy;
        let cutline = shell_quote(p.cutline.as_ref().unwrap().as_str());
        let photo = f.media.join("IMG_7206.HEIC");
        std::fs::write(&photo, b"x").unwrap();
        let ok = format!("{cutline} still '{photo}' --out photo-copies/IMG_7206.jpg --max-side 1568");
        assert_eq!(bash(p, &ok), Decision::Allow, "{ok}");
        let abs = format!("{cutline} still {photo} -o {}", f.run.join("photo-copies/a.jpg"));
        assert_eq!(bash(p, &abs), Decision::Allow, "{abs}");
        // anywhere else in the folder, outside it, or no --out at all
        for out in ["spec.md", "media-previews/x.jpg", "photo-copies", "../x.jpg", "/tmp/x.jpg"] {
            let cmd = format!("{cutline} still '{photo}' --out {out}");
            let why = denied(bash(p, &cmd));
            assert!(why.contains("photo-copies") || why.contains("inside the run folder"), "{cmd}: {why}");
        }
        assert!(denied(bash(p, &format!("{cutline} still '{photo}'"))).contains("photo-copies"));
        // a photo it may not read
        let secret = f.outside.join("x.heic");
        assert!(denied(bash(p, &format!("{cutline} still '{secret}' --out photo-copies/x.jpg"))).contains("outside"));
        // no other cutline subcommand, helper or command
        assert!(denied(bash(p, &format!("{cutline} render project.json5"))).contains("only `cutline still`"));
        let helper = shell_quote(p.helper.as_ref().unwrap().as_str());
        assert!(denied(bash(p, &format!("{helper} eleven voices"))).contains("only run `cutline still`"));
        assert!(denied(bash(p, "rm -rf photo-copies")).contains("isn't an allowed command"));
        assert!(denied(bash(p, &format!("{ok} | cat"))).contains("pipe"));
        // the copies folder is not writable with Write/Edit
        assert!(denied(p.decide("Write", &json!({"file_path": "photo-copies/a.jpg", "content": "x"})))
            .contains("Only spec.md"));
        // no cutline: no shell
        let mut none = f.policy.clone();
        none.cutline = None;
        assert!(denied(bash(&none, &ok)).contains("Shell commands aren't available"));
    }

    /// BUG-0002: the two commands Claude tried (a variable for a temporary folder, then a copy
    /// outside photo-copies/) are denied with the exact command to run instead.
    #[test]
    fn assistant_still_denials_spell_out_the_command() {
        let f = fx(Mode::Assistant);
        let p = &f.policy;
        let cutline = shell_quote(p.cutline.as_ref().unwrap().as_str());
        let photo = f.media.join("IMG_7206.HEIC");
        std::fs::write(&photo, b"x").unwrap();
        let usage =
            format!("{cutline} still '<photo>' --out {}", shell_quote(f.run.join("photo-copies/<name>.jpg").as_str()));
        let tmp_var = format!("{cutline} still '{photo}' --out \"$TMPDIR/heic/IMG_7206.jpg\"");
        let why = denied(bash(p, &tmp_var));
        assert!(why.contains("variable") && why.contains(&usage), "{why}");
        assert!(!why.contains("video-studio eleven"), "{why}");
        let tmp = format!("{cutline} still '{photo}' --out /tmp/heic/IMG_7206.jpg");
        let why = denied(bash(p, &tmp));
        assert!(why.contains("photo-copies/ in your working folder") && why.contains(&usage), "{why}");
        let why = denied(bash(p, "sips -s format jpeg x.heic --out /tmp/x.jpg"));
        assert!(why.contains("isn't an allowed command") && why.contains(&usage), "{why}");
        // the suggested form, filled in, is allowed
        let filled = format!(
            "{cutline} still '{photo}' --out {}",
            shell_quote(f.run.join("photo-copies/IMG_7206.jpg").as_str())
        );
        assert_eq!(bash(p, &filled), Decision::Allow, "{filled}");
    }

    #[test]
    fn runs_may_copy_heic_photos_with_cutline_still() {
        let f = fx(Mode::DryRun);
        let p = &f.policy;
        let photo = f.media.join("IMG_7206.HEIC");
        assert_eq!(
            bash(p, &format!("cutline still '{photo}' --out photo-copies/a.jpg --max-side 1568")),
            Decision::Allow
        );
        assert_eq!(bash(p, &format!("cutline still '{photo}' --out work/a.jpg")), Decision::Allow);
        assert!(denied(bash(p, &format!("cutline still '{photo}' --out /tmp/a.jpg"))).contains("inside the run folder"));
        let secret = f.outside.join("x.heic");
        assert!(denied(bash(p, &format!("cutline still '{secret}' --out a.jpg"))).contains("outside"));
    }

    #[test]
    fn hook_answers_and_logs() {
        let mut f = fx(Mode::Production);
        let log = f.run.join("logs/permissions.ndjson");
        f.policy.log = Some(log.clone());
        let allow = hook_response(
            &f.policy,
            &json!({"tool_name": "Bash", "tool_input": {"command": "cutline validate project.json5"}}),
        );
        assert_eq!(allow["hookSpecificOutput"]["permissionDecision"], "allow");
        let deny = hook_response(&f.policy, &json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}));
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(deny["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("blocked by Syncfusion Video Studio"));
        let ask = hook_response(&f.policy, &json!({"tool_name": "AskUserQuestion", "tool_input": {}}));
        assert_eq!(ask, json!({}), "questions pass through to the host");
        let lines: Vec<Value> =
            std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(
            lines.iter().map(|l| l["decision"].as_str().unwrap()).collect::<Vec<_>>(),
            vec!["allow", "deny", "ask_user"]
        );
        assert!(lines[1]["reason"].as_str().unwrap().contains("Only cutline"));
        let settings = hook_settings("/Apps/Video Studio/video-studio".into(), "/lib/ctx/policy.json".into());
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "'/Apps/Video Studio/video-studio' hook --policy /lib/ctx/policy.json"
        );
        // the policy survives the trip through its file
        let text = serde_json::to_string(&f.policy).unwrap();
        assert_eq!(serde_json::from_str::<Policy>(&text).unwrap(), f.policy);
    }
}
