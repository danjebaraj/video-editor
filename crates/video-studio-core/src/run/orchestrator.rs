//! The run orchestrator (spec §12): launches Claude Code in the run folder, answers its
//! permission requests through the broker, turns its stream into progress events, handles
//! questions and cancellation, and writes a complete log set whatever the outcome.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::collect::{self, OutputFile};
use super::events::{progress_fraction, stage_for_tool, Event, EventKind, EventSink, LoggingSink};
use super::exit_code;
use super::prepare::{prepare, Prepared, RunEnv, RunRequest};
use super::proxy;
use super::readiness::ReadinessReport;
use crate::agent::AgentProcess;
use crate::claude::process::{friendly_error, ClaudeProcess, Incoming};
use crate::claude::stream::{self, StreamEvent, Usage};
use crate::claude::Decision;
use crate::codex::agent::CodexProcess;
use crate::project::{record_run, RunKind, RunRecord, RunStatus};
use crate::{eleven, paths, timeutil, Result};

/// How long Claude gets to stop on its own before the process tree is killed (spec §12.4).
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// The message Claude gets when nobody answers a question.
pub const ASSUME_MESSAGE: &str = "No one is available to answer. Proceed on your stated assumption (your first, recommended option), and record this question and the assumption you made in summary.md.";

/// The run result's note for capture the system's permissions blocked (BUG-stellar-lily-K8CN):
/// what each missing permission stops.
fn capture_blocked_note(missing: &[String]) -> String {
    let what: Vec<String> = missing
        .iter()
        .map(|m| match m.as_str() {
            crate::capture::SCREEN_RECORDING => format!("recording and screenshots need {m}"),
            crate::capture::ACCESSIBILITY => format!("clicking, typing and menus need {m}"),
            other => format!("capture needs {other}"),
        })
        .collect();
    format!(
        "Desktop capture was blocked: {}, which {} doesn't have (System Settings › Privacy & Security). Anything delivered was made without it.",
        what.join("; "),
        crate::APP_NAME
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunOutcome {
    pub run_id: String,
    pub folder: String,
    pub status: RunStatus,
    pub exit_code: i32,
    pub record: RunRecord,
    pub outputs: Vec<OutputFile>,
    pub readiness: Option<ReadinessReport>,
    pub warnings: Vec<String>,
    pub message: Option<String>,
}

struct Shared {
    cancel: AtomicBool,
    kill: AtomicBool,
    finished: AtomicBool,
    process: Mutex<Option<AgentProcess>>,
    answers: Mutex<HashMap<String, BTreeMap<String, String>>>,
}

/// A running (or finished) run.
pub struct RunHandle {
    shared: Arc<Shared>,
    run_id: String,
    folder: Utf8PathBuf,
    thread: Mutex<Option<std::thread::JoinHandle<RunOutcome>>>,
    outcome: Mutex<Option<RunOutcome>>,
}

impl RunHandle {
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn folder(&self) -> &Utf8Path {
        &self.folder
    }
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::SeqCst)
    }

    /// Graceful stop: an interrupt, then after the grace period the whole process tree is killed.
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::SeqCst);
    }

    /// Immediate stop (a second Ctrl-C).
    pub fn kill(&self) {
        self.shared.cancel.store(true, Ordering::SeqCst);
        self.shared.kill.store(true, Ordering::SeqCst);
        if let Some(p) = self.shared.process.lock().unwrap().as_ref() {
            p.kill();
        }
    }

    /// Answers a question event: question text → chosen label(s), comma-joined for multi-select.
    pub fn answer(&self, question_id: &str, answers: BTreeMap<String, String>) {
        self.shared.answers.lock().unwrap().insert(question_id.to_string(), answers);
    }

    /// Blocks until the run ends and returns its outcome. Safe to call more than once.
    pub fn wait(&self) -> RunOutcome {
        if let Some(t) = self.thread.lock().unwrap().take() {
            let outcome = t.join().unwrap_or_else(|_| panic!("the run thread panicked"));
            *self.outcome.lock().unwrap() = Some(outcome);
        }
        self.outcome.lock().unwrap().clone().expect("outcome is set once the thread has joined")
    }
}

/// Prepares the run folder and starts the run on its own thread. Errors here mean nothing was
/// launched (bad run config, unwritable output folder, …).
pub fn start_run(request: &RunRequest, env: &RunEnv, sink: Arc<dyn EventSink>) -> Result<RunHandle> {
    let prepared = prepare(request, env)?;
    let shared = Arc::new(Shared {
        cancel: AtomicBool::new(false),
        kill: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        process: Mutex::new(None),
        answers: Mutex::new(HashMap::new()),
    });
    let run_id = prepared.run_id.clone();
    let folder = prepared.folder.clone();
    let sink: Arc<dyn EventSink> = Arc::new(LoggingSink::new(&prepared.logs.join("events.ndjson"), sink));
    let price = env.models.family_of(prepared.record.model.as_deref().unwrap_or("")).and_then(|f| f.price.clone());
    let project_path = request.project_path.clone();
    let thread_shared = shared.clone();
    let thread = std::thread::Builder::new().name("video-studio-run".into()).spawn(move || {
        // nothing the run starts may inherit the caller's blocked signals (BUG-stellar-lily-K8CN)
        crate::platform::unblock_signals();
        let outcome = Driver::new(prepared, sink, thread_shared.clone(), price, project_path).drive();
        thread_shared.finished.store(true, Ordering::SeqCst);
        outcome
    })?;
    Ok(RunHandle { shared, run_id, folder, thread: Mutex::new(Some(thread)), outcome: Mutex::new(None) })
}

struct PendingQuestion {
    input: Value,
    deadline: Option<Instant>,
}

/// Reads lines appended to a file since the last call.
struct Tail {
    path: Utf8PathBuf,
    offset: u64,
}

impl Tail {
    fn new(path: Utf8PathBuf) -> Self {
        Tail { path, offset: 0 }
    }
    fn read_new(&mut self) -> Vec<String> {
        let Ok(mut f) = std::fs::File::open(&self.path) else { return Vec::new() };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut reader = BufReader::new(&mut f);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // a partial last line is read again next time
                    if !line.ends_with('\n') {
                        break;
                    }
                    self.offset += n as u64;
                    out.push(line.trim_end().to_string());
                }
            }
        }
        out
    }
}

struct Driver {
    p: Prepared,
    sink: Arc<dyn EventSink>,
    shared: Arc<Shared>,
    price: Option<crate::models::Price>,
    project_path: Utf8PathBuf,
    started: Instant,
    stage: Option<(String, String, Instant, bool)>,
    /// Tool use ID → (tool name, stage it started).
    /// Tool calls waiting for their result: name, stage and input.
    tools: HashMap<String, (String, Option<String>, Value)>,
    pending: HashMap<String, PendingQuestion>,
    usage: HashMap<String, Usage>,
    final_usage: Option<Usage>,
    eleven_credits: u64,
    session: Option<String>,
    model_used: Option<String>,
    /// The assistant CLI's version, as it reported it when the session started.
    cli_version: Option<String>,
    result_error: Option<String>,
    /// The first tool call that failed (not one the broker refused): kept for the outcome so a
    /// missing output comes with what went wrong first (BUG-rustic-olive-U4PY).
    first_failure: Option<String>,
    /// cutline calls (by number) that only asked for `--help` (BUG-green-cricket-P6HQ).
    help_calls: std::collections::HashSet<u64>,
    /// Operations that failed in the current stage and weren't retried successfully: only a
    /// success of the same operation clears its failure (BUG-deft-lily-344X).
    stage_failures: std::collections::HashSet<String>,
    /// ElevenLabs calls that failed and weren't later made successfully: (command, output, error).
    eleven_failures: Vec<(String, Option<String>, String)>,
    saw_result: bool,
    warnings: Vec<String>,
    denials: u32,
    last_validate_code: Option<i64>,
    last_cost_emit: Instant,
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// A one-line description of a tool call for the transcript.
pub fn tool_summary(name: &str, input: &Value, root: &Utf8Path) -> String {
    let rel = |p: &str| {
        let path = Utf8Path::new(p);
        path.strip_prefix(root).map(|r| r.to_string()).unwrap_or_else(|_| p.to_string())
    };
    let path = ["file_path", "notebook_path", "path"].iter().find_map(|k| input[*k].as_str()).map(rel);
    match name {
        "Read" => format!("Read `{}`", path.unwrap_or_else(|| "file".into())),
        "Edit" | "MultiEdit" => format!("Edited `{}`", path.unwrap_or_else(|| "file".into())),
        "Write" => format!("Wrote `{}`", path.unwrap_or_else(|| "file".into())),
        "Glob" => format!("Searched files `{}`", input["pattern"].as_str().unwrap_or("")),
        "Grep" => format!("Searched for `{}`", input["pattern"].as_str().unwrap_or("")),
        "Bash" => format!("Ran `{}`", truncate(input["command"].as_str().unwrap_or(""), 160)),
        "WebSearch" => format!("Web search: {}", input["query"].as_str().unwrap_or("")),
        "WebFetch" => format!("Fetched {}", input["url"].as_str().unwrap_or("")),
        "TodoWrite" => "Updated its plan".into(),
        other => other.to_string(),
    }
}

/// A one-line description of a tool call that came back as an error, for the transcript: what
/// was tried and why it didn't work, in place of a bare "Bash failed". Claude usually reads the
/// same message and tries again, so this is a note on the way, not the run failing.
/// What a tool call does, for telling a retry from another call: `video-studio eleven music`,
/// `cutline audio`, `Write summary.md`. Options and values don't count (BUG-deft-lily-344X).
pub fn operation(name: &str, input: &Value) -> String {
    if name != "Bash" {
        return format!("{name} {}", input["file_path"].as_str().unwrap_or(""));
    }
    let command = input["command"].as_str().unwrap_or("");
    let words: Vec<String> = command
        .split_whitespace()
        .map(|w| w.trim_matches(|c| c == '\'' || c == '"').to_string())
        .filter(|w| !w.is_empty())
        .collect();
    let mut rest = words
        .iter()
        .skip_while(|w| matches!(w.as_str(), "bash" | "sh" | "zsh" | "-lc" | "-c" | "env") || w.contains('='));
    let Some(program) = rest.next() else { return command.to_string() };
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program).trim_end_matches(".exe").to_string();
    let subcommands = match program.as_str() {
        "video-studio" => 2,
        "cutline" => 1,
        _ => 0,
    };
    let mut op = program;
    for w in rest.filter(|w| !w.starts_with('-')).take(subcommands) {
        op.push(' ');
        op.push_str(w);
    }
    op
}

pub fn tool_failure(name: &str, input: &Value, root: &Utf8Path, content: &str) -> String {
    let rel = |p: &str| {
        let path = Utf8Path::new(p);
        path.strip_prefix(root).map(|r| r.to_string()).unwrap_or_else(|_| p.to_string())
    };
    let file = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .map(rel)
        .unwrap_or_else(|| "file".into());
    // The tool's own words, without wrappers: "<tool_use_error>…", "Exit code 2", "error: ".
    let text = content.replace("<tool_use_error>", "").replace("</tool_use_error>", "");
    let mut exit: Option<&str> = None;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty()).peekable();
    if let Some(code) = lines.peek().and_then(|l| l.strip_prefix("Exit code ")) {
        exit = Some(code);
        lines.next();
    }
    let body: Vec<&str> = lines.collect();
    let strip = |l: &str| {
        let l = l.trim_start_matches("[err] ").trim_start_matches("[out] ");
        let l = l.strip_prefix("error: ").or_else(|| l.strip_prefix("Error: ")).unwrap_or(l);
        truncate(l.split(" Note: your current working directory").next().unwrap_or(l).trim_end_matches('.'), 200)
    };
    let first = body.first().map(|l| strip(l)).unwrap_or_default();
    let reason = |fallback: &str| if first.is_empty() { fallback.to_string() } else { first.clone() };
    match name {
        "Bash" => {
            let command = input["command"].as_str().unwrap_or("").trim();
            let shown = truncate(command, 80);
            // A check that found problems is a result, not a breakdown: say what it found.
            let json: Option<Value> = serde_json::from_str(&body.join("\n")).ok();
            if let Some(errors) = json.as_ref().and_then(|v| v["errors"].as_array()).filter(|e| !e.is_empty()) {
                let what = errors[0]["message"].as_str().unwrap_or("see the details");
                let hint = errors[0]["hint"].as_str().map(|h| format!(" ({h})")).unwrap_or_default();
                let more = if errors.len() > 1 { format!(", and {} more", errors.len() - 1) } else { String::new() };
                let n = errors.len();
                return format!(
                    "`{shown}` found {n} problem{} to fix: {}{hint}{more}",
                    if n == 1 { "" } else { "s" },
                    truncate(what, 160)
                );
            }
            match (first.is_empty(), exit) {
                (false, Some(code)) => format!("`{shown}` didn't work (exit code {code}): {first}"),
                (false, None) => format!("`{shown}` didn't work: {first}"),
                (true, Some(code)) => format!("`{shown}` ended with exit code {code}"),
                (true, None) => format!("`{shown}` didn't work"),
            }
        }
        "Read" => format!("Couldn't read `{file}`: {}", reason("it couldn't be opened")),
        "Edit" | "MultiEdit" if text.contains("No changes to make") => {
            format!("No change made to `{file}`: the new text is the same as the old")
        }
        "Edit" | "MultiEdit" => format!("Couldn't edit `{file}`: {}", reason("the edit didn't apply")),
        "Write" => format!("Couldn't write `{file}`: {}", reason("the file couldn't be saved")),
        "Glob" | "Grep" => format!("The search didn't work: {}", reason("no details were given")),
        "WebSearch" | "WebFetch" => format!("The web request didn't work: {}", reason("no details were given")),
        other => format!("{other} didn't work: {}", reason("no details were given")),
    }
}

impl Driver {
    fn new(
        p: Prepared,
        sink: Arc<dyn EventSink>,
        shared: Arc<Shared>,
        price: Option<crate::models::Price>,
        project_path: Utf8PathBuf,
    ) -> Self {
        Driver {
            p,
            sink,
            shared,
            price,
            project_path,
            started: Instant::now(),
            stage: None,
            tools: HashMap::new(),
            pending: HashMap::new(),
            usage: HashMap::new(),
            final_usage: None,
            eleven_credits: 0,
            session: None,
            model_used: None,
            cli_version: None,
            result_error: None,
            first_failure: None,
            help_calls: Default::default(),
            stage_failures: Default::default(),
            eleven_failures: Vec::new(),
            saw_result: false,
            warnings: Vec::new(),
            denials: 0,
            last_validate_code: None,
            last_cost_emit: Instant::now() - Duration::from_secs(60),
        }
    }

    fn agent_name(&self) -> &'static str {
        if self.p.codex.is_some() {
            "Codex"
        } else {
            "Claude Code"
        }
    }

    /// The provider as the run's messages name it: "Codex" or "Claude" (BUG-rustic-olive-U4PY).
    fn agent_short(&self) -> &'static str {
        if self.p.codex.is_some() {
            "Codex"
        } else {
            "Claude"
        }
    }

    fn emit(&self, kind: EventKind) {
        self.sink.emit(&Event::now(kind));
    }

    fn note(&self, role: &str, text: impl Into<String>) {
        self.emit(EventKind::Message { role: role.into(), text: text.into(), tool: None, detail: None, failed: false });
    }

    fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.emit(EventKind::Warning { message: message.clone() });
        self.warnings.push(message);
    }

    fn start_stage(&mut self, stage: &str, label: &str) {
        if let Some((s, l, _, _)) = &self.stage {
            if s == stage && l == label {
                return;
            }
        }
        self.finish_stage();
        self.stage_failures.clear();
        self.emit(EventKind::StageStarted { stage: stage.into(), label: label.into() });
        self.stage = Some((stage.into(), label.into(), Instant::now(), true));
    }

    fn finish_stage(&mut self) {
        if let Some((stage, _, at, ok)) = self.stage.take() {
            self.emit(EventKind::StageFinished { stage, ok, seconds: at.elapsed().as_secs_f64() });
        }
    }

    /// Whether a write is reported only once it is confirmed: Codex's file changes, which are
    /// approved first and applied after (BUG-green-cricket-P6HQ).
    fn confirms_writes(&self, tool: &str) -> bool {
        self.p.codex.is_some() && matches!(tool, "Write" | "Edit" | "MultiEdit")
    }

    /// A path relative to the run folder when inside it.
    fn shown(&self, path: &str) -> String {
        Utf8Path::new(path).strip_prefix(&self.p.folder).map(|p| p.to_string()).unwrap_or_else(|_| path.to_string())
    }

    /// The writes that started and were never reported done, `a`, `b`; `None` when there are none.
    fn unfinished_writes(&self) -> Option<String> {
        let mut paths: Vec<String> = self
            .tools
            .values()
            .filter(|(name, ..)| matches!(name.as_str(), "Write" | "Edit" | "MultiEdit"))
            .map(|(_, _, input)| format!("`{}`", self.shown(input["file_path"].as_str().unwrap_or("file"))))
            .collect();
        if paths.is_empty() {
            return None;
        }
        paths.sort();
        paths.dedup();
        Some(paths.join(", "))
    }

    /// " First problem: …" for an outcome message, when a tool call failed during the run.
    fn first_failure_note(&self) -> String {
        self.first_failure
            .as_ref()
            .map(|f| format!(" First problem: {}", f.trim_end_matches('.')))
            .map(|f| f + ".")
            .unwrap_or_default()
    }

    fn estimated_cost(&self) -> (Option<f64>, u64, u64) {
        let (mut input, mut output, mut cost) = (0u64, 0u64, 0.0f64);
        for u in self.usage.values() {
            input += u.total_input();
            output += u.output_tokens;
            if let Some(p) = &self.price {
                cost +=
                    (u.input_tokens as f64 + u.cache_creation_tokens as f64 * 1.25 + u.cache_read_tokens as f64 * 0.1)
                        * p.input
                        / 1e6
                        + u.output_tokens as f64 * p.output / 1e6;
            }
        }
        (self.price.as_ref().map(|_| cost), input, output)
    }

    fn emit_cost(&mut self, force: bool) {
        if !force && self.last_cost_emit.elapsed() < Duration::from_millis(900) {
            return;
        }
        self.last_cost_emit = Instant::now();
        let (est, mut input, mut output) = self.estimated_cost();
        let reported = self.final_usage.as_ref().and_then(|u| u.cost_usd);
        // Messages report their tokens as they start, which undercounts output; the totals
        // Claude Code gives at the end are the real ones.
        if let Some(u) = self.final_usage.as_ref().filter(|u| u.output_tokens > output) {
            input = u.total_input();
            output = u.output_tokens;
        }
        self.emit(EventKind::Cost {
            claude_usd: reported.or(est),
            claude_estimated: reported.is_none(),
            input_tokens: input,
            output_tokens: output,
            eleven_credits: self.eleven_credits,
            eleven_cap: self.p.run_config.eleven_credit_cap,
        });
    }

    fn save_record(&self, record: &RunRecord) {
        if let Err(e) = record_run(&self.project_path, record) {
            self.emit(EventKind::Warning {
                message: format!("Couldn't update the run history in {}: {e}", self.project_path),
            });
        }
    }

    fn drive(mut self) -> RunOutcome {
        let kind = self.p.request.kind;
        self.emit(EventKind::RunStarted {
            run_id: self.p.run_id.clone(),
            kind,
            run_config: self.p.run_config.name.clone(),
            folder: self.p.folder.to_string(),
            model: self.p.options.model.clone(),
            outputs: self.p.outputs.iter().map(|o| o.name.clone()).collect(),
        });
        self.start_stage("prepare", "Prepare");
        self.save_record(&self.p.record.clone());
        for m in self.p.manifest.iter().filter(|m| m.path.is_none()) {
            let msg = format!("Media `{}` is missing and isn't available to this run.", m.id);
            self.emit(EventKind::Warning { message: msg.clone() });
            self.warnings.push(msg);
        }
        self.emit(EventKind::Artifact { kind: "log".into(), path: self.p.logs.to_string(), output: None, bytes: None });

        let mut launch_error: Option<String> = None;
        let mut activity = Tail::new(proxy::activity_path(&self.p.helper_context));
        let mut eleven_log = Tail::new(self.p.helper_context.log_path());
        let mut stop_started: Option<Instant> = None;
        let mut exit: Option<(i32, String)> = None;
        let mut turns_over = false;

        // Codex or Claude Code, as chosen when the run started (FEAT-spry-alpaca-5RQZ)
        if self.p.codex.is_some() {
            // traced, not counted: it's true of every Codex run, so it doesn't change the outcome
            self.emit(EventKind::Warning { message: crate::codex::agent::READS_UNCHECKED_WARNING.into() });
        }
        // Codex's sandbox can't reach the key store or the network, and Chrome can't start in
        // it: the host makes the ElevenLabs calls and renders the browser jobs the helper hands
        // it (BUG-deft-lily-344X, BUG-stellar-lily-K8CN)
        let relay = match (&self.p.codex, &self.p.helper_context.relay_dir) {
            (Some(_), Some(dir)) => Some(super::relay::HostRelay::start(super::relay::HostSetup {
                dir: dir.clone(),
                helper: self.p.helper.clone(),
                context_file: self.p.context_dir.join("context.json"),
                run_folder: self.p.folder.clone(),
                web_access: self.p.codex.as_ref().is_some_and(|c| c.web_access),
            })),
            _ => None,
        };
        let launched = match &self.p.codex {
            Some(c) => CodexProcess::start(c, &self.p.prompt).map(|(p, rx)| (AgentProcess::Codex(p), rx)),
            None => ClaudeProcess::start(&self.p.options, &self.p.prompt).map(|(p, rx)| (AgentProcess::Claude(p), rx)),
        };
        match launched {
            Err(e) => launch_error = Some(e.to_string()),
            Ok((process, rx)) => {
                *self.shared.process.lock().unwrap() = Some(process.clone());
                loop {
                    match rx.recv_timeout(Duration::from_millis(120)) {
                        Ok(Incoming::Event(e)) => self.handle(&process, e),
                        Ok(Incoming::ContextLost { previous, reason }) => self.warn(format!(
                            "Codex couldn't continue the revised run's conversation ({previous}: {}), so it starts a new one from this run's files.",
                            reason.trim().trim_end_matches('.')
                        )),
                        Ok(Incoming::Exit { code, stderr }) => {
                            exit = Some((code, stderr));
                            break;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    self.tick(&process, &mut activity, &mut eleven_log);
                    if let Some(max) = self.p.run_config.max_turns.filter(|m| *m > 0) {
                        if !turns_over && self.usage.len() as u32 > max && kind == RunKind::Production {
                            turns_over = true;
                            self.result_error = Some(format!("The run reached its limit of {max} turns before finishing. Raise the limit in the run config, or simplify the spec."));
                            self.shared.cancel.store(true, Ordering::SeqCst);
                        }
                    }
                    if self.shared.cancel.load(Ordering::SeqCst) && stop_started.is_none() {
                        stop_started = Some(Instant::now());
                        if !turns_over {
                            self.note("note", "Stopping…");
                        }
                        for id in self.pending.drain().map(|(id, _)| id).collect::<Vec<_>>() {
                            process.deny(&id, "The run was cancelled.");
                        }
                        // Interrupt, then give Claude the grace period; `stop` kills the tree after it.
                        let p = process.clone();
                        let immediate = self.shared.kill.load(Ordering::SeqCst);
                        std::thread::spawn(move || {
                            p.stop(if immediate { Duration::from_millis(0) } else { STOP_GRACE });
                        });
                    }
                }
                // Whatever Claude started must not outlive the run.
                if process.is_running() {
                    process.kill();
                } else {
                    crate::platform::ProcessGroup::kill_pid(process.pid());
                }
                self.tick_logs(&mut activity, &mut eleven_log);
                *self.shared.process.lock().unwrap() = None;
            }
        }
        if let Some(r) = relay {
            r.stop();
        }
        let cancelled = self.shared.cancel.load(Ordering::SeqCst) && !turns_over;
        self.finish(launch_error, exit, cancelled)
    }

    fn handle(&mut self, process: &AgentProcess, e: StreamEvent) {
        match e {
            StreamEvent::Init { session_id, model, cli_version, .. } => {
                self.session = Some(session_id);
                if cli_version.is_some() {
                    self.cli_version = cli_version;
                }
                if model.is_some() {
                    self.model_used = model;
                }
                if matches!(&self.stage, Some((s, ..)) if s == "prepare") {
                    self.finish_stage();
                }
            }
            StreamEvent::TextDelta(text) => self.emit(EventKind::TextDelta { text }),
            StreamEvent::AssistantText(text) => self.note("assistant", text),
            StreamEvent::MessageUsage { message_id, usage } => {
                self.usage.insert(message_id, usage);
                self.emit_cost(false);
            }
            StreamEvent::ToolUse { id, name, input } => {
                let stage = stage_for_tool(&name, &input);
                if let Some((s, label)) = &stage {
                    self.start_stage(s, label);
                }
                self.tools.insert(id, (name.clone(), stage.map(|s| s.0), input.clone()));
                if self.confirms_writes(&name) {
                    // Codex's change is written only once it reports it applied
                    // (BUG-green-cricket-P6HQ): pending until then
                    let path = input["file_path"].as_str().unwrap_or("file");
                    self.emit(EventKind::Progress {
                        stage: self.stage.as_ref().map(|s| s.0.clone()).unwrap_or_else(|| "authoring".into()),
                        fraction: None,
                        detail: Some(format!("Writing `{}`…", self.shown(path))),
                    });
                } else if name != "AskUserQuestion" {
                    let detail = serde_json::to_string_pretty(&input).ok().map(|d| truncate(&d, 4000));
                    self.emit(EventKind::Message {
                        role: "tool".into(),
                        text: tool_summary(&name, &input, &self.p.folder),
                        tool: Some(name),
                        detail,
                        failed: false,
                    });
                }
            }
            StreamEvent::ToolResult { tool_use_id, content, is_error } => {
                let Some((name, stage, input)) = self.tools.remove(&tool_use_id) else { return };
                if content == crate::codex::agent::DECLINED_BY_HOST {
                    // the broker's refusal was already reported
                    return;
                }
                if !is_error && self.confirms_writes(&name) {
                    let detail = serde_json::to_string_pretty(&input).ok().map(|d| truncate(&d, 4000));
                    self.emit(EventKind::Message {
                        role: "tool".into(),
                        text: tool_summary(&name, &input, &self.p.folder),
                        tool: Some(name.clone()),
                        detail,
                        failed: false,
                    });
                }
                if is_error && content.contains("blocked by Syncfusion Video Studio") {
                    // Refused by the pre-tool hook: an answer Claude adapts to, not a failure.
                    let reason = content
                        .split("hook error:")
                        .last()
                        .unwrap_or(&content)
                        .replace("This is blocked by Syncfusion Video Studio.", "");
                    self.note("note", format!("Blocked: {}", reason.trim()));
                } else if is_error {
                    if let (Some(s), Some(cur)) = (&stage, self.stage.as_mut()) {
                        if &cur.0 == s {
                            cur.3 = false;
                            self.stage_failures.insert(operation(&name, &input));
                        }
                    }
                    let text = tool_failure(&name, &input, &self.p.folder, &content);
                    if self.first_failure.is_none() {
                        self.first_failure = Some(truncate(&text, 300));
                    }
                    self.emit(EventKind::Message {
                        role: "tool".into(),
                        text,
                        tool: Some(name),
                        detail: Some(truncate(&content, 4000)),
                        failed: true,
                    });
                } else if let (Some(s), Some(cur)) = (&stage, self.stage.as_mut()) {
                    // a later success of the same operation clears its failure (fix and retry);
                    // another one, such as a quote after a failed generation, doesn't
                    if &cur.0 == s {
                        self.stage_failures.remove(&operation(&name, &input));
                        cur.3 = self.stage_failures.is_empty();
                    }
                }
            }
            StreamEvent::PermissionRequest { request_id, tool_name, input, .. } => {
                self.decide(process, request_id, tool_name, input)
            }
            StreamEvent::Result { is_error, text, session_id, subtype, usage } => {
                self.saw_result = true;
                if session_id.is_some() {
                    self.session = session_id;
                }
                if let Some(u) = usage {
                    self.final_usage = Some(u);
                }
                let stopping = self.shared.cancel.load(Ordering::SeqCst);
                let failed = is_error
                    || subtype.as_deref().map(|s| s != "success" && !(stopping && s == "interrupted")).unwrap_or(false);
                if failed && self.result_error.is_none() && !self.shared.cancel.load(Ordering::SeqCst) {
                    let msg = match subtype.as_deref() {
                        Some("error_max_budget_usd") => format!(
                            "The run reached its {} spend limit before finishing. Raise the limit in the run config.",
                            self.agent_short()
                        ),
                        Some("error_max_turns") => "The run reached its turn limit before finishing.".to_string(),
                        _ => friendly_error(&text.unwrap_or_else(|| {
                            format!(
                                "{} reported an error ({}).",
                                self.agent_name(),
                                subtype.as_deref().unwrap_or("unknown")
                            )
                        })),
                    };
                    self.result_error = Some(msg);
                }
                self.emit_cost(true);
                // One turn per run: closing input lets the CLI exit.
                process.finish_input();
            }
            StreamEvent::ControlResponse { .. } | StreamEvent::Ignored(_) => {}
        }
    }

    fn decide(&mut self, process: &AgentProcess, request_id: String, tool: String, input: Value) {
        let decision = self.p.policy.decide(&tool, &input);
        let verdict = match &decision {
            Decision::Allow => "allow",
            Decision::Deny(_) => "deny",
            Decision::AskUser => "ask_user",
        };
        process.log_note(json!({"type": "broker_decision", "ts": timeutil::now_rfc3339(), "request_id": request_id, "tool": tool, "decision": verdict, "reason": match &decision { Decision::Deny(r) => Some(r.clone()), _ => None }}));
        match decision {
            Decision::Allow => process.allow(&request_id, &input),
            Decision::Deny(reason) => {
                self.denials += 1;
                process.deny(&request_id, &format!("{reason} This is blocked by Syncfusion Video Studio."));
                self.note("note", format!("Blocked: {reason}"));
            }
            Decision::AskUser => {
                let questions = stream::questions(&input);
                if questions.is_empty() {
                    process.allow(&request_id, &input);
                    return;
                }
                let timeout =
                    self.p.request.question_timeout_s.or(self.p.run_config.question_timeout_s).filter(|t| *t > 0);
                self.emit(EventKind::Question {
                    id: request_id.clone(),
                    questions: questions.clone(),
                    timeout_s: timeout,
                });
                if self.p.request.assume_answers {
                    process.deny(&request_id, ASSUME_MESSAGE);
                    self.emit(EventKind::QuestionAnswered {
                        id: request_id,
                        by: "assumed".into(),
                        answers: BTreeMap::new(),
                    });
                    let q = questions.iter().map(|q| q.question.as_str()).collect::<Vec<_>>().join(" / ");
                    self.warn(format!(
                        "{} asked \"{q}\" and proceeded on its own assumption (no one was asked).",
                        self.agent_short()
                    ));
                } else {
                    self.pending.insert(
                        request_id,
                        PendingQuestion {
                            input,
                            deadline: timeout.map(|t| Instant::now() + Duration::from_secs(t as u64)),
                        },
                    );
                }
            }
        }
    }

    fn tick(&mut self, process: &AgentProcess, activity: &mut Tail, eleven_log: &mut Tail) {
        // Answers from the user, and questions that waited too long.
        let ids: Vec<String> = self.pending.keys().cloned().collect();
        for id in ids {
            let answer = self.shared.answers.lock().unwrap().remove(&id);
            if let Some(answers) = answer {
                if let Some(q) = self.pending.remove(&id) {
                    process.allow(&id, &stream::answered_input(&q.input, &answers));
                    self.emit(EventKind::QuestionAnswered { id, by: "user".into(), answers });
                }
            } else if self.pending.get(&id).and_then(|q| q.deadline).map(|d| Instant::now() >= d).unwrap_or(false) {
                let q = self.pending.remove(&id).unwrap();
                process.deny(&id, ASSUME_MESSAGE);
                self.emit(EventKind::QuestionAnswered { id, by: "timeout".into(), answers: BTreeMap::new() });
                let text =
                    stream::questions(&q.input).iter().map(|q| q.question.clone()).collect::<Vec<_>>().join(" / ");
                self.warn(format!(
                    "{} asked \"{text}\" and proceeded on its own assumption (no answer in time).",
                    self.agent_short()
                ));
            }
        }
        self.tick_logs(activity, eleven_log);
    }

    fn tick_logs(&mut self, activity: &mut Tail, eleven_log: &mut Tail) {
        for line in activity.read_new() {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let stage = self.stage.as_ref().map(|s| s.0.clone()).unwrap_or_else(|| "authoring".into());
            match v["type"].as_str() {
                Some("start") => {
                    let argv = v["argv"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>());
                    if argv.is_some_and(|a| a.iter().any(|x| super::events::is_help_flag(x))) {
                        self.help_calls.insert(v["n"].as_u64().unwrap_or(0));
                    }
                }
                Some("line") => {
                    let text = v["text"].as_str().unwrap_or("");
                    self.emit(EventKind::Progress {
                        stage,
                        fraction: progress_fraction(text),
                        detail: Some(truncate(text, 200)),
                    });
                }
                Some("exit") => {
                    let code = v["code"].as_i64().unwrap_or(-1);
                    if v["command"] == "validate" {
                        self.last_validate_code = Some(code);
                    }
                    let n = v["n"].as_u64().unwrap_or(0);
                    let command = v["command"].as_str().unwrap_or("");
                    let log = self.p.logs.join(format!("cutline/{n:03}-{command}.log"));
                    // a lookup (schema, help, doctor) finishing completes nothing (BUG-rustic-olive-U4PY)
                    let lookup = super::events::is_informational(command) || self.help_calls.contains(&n);
                    self.emit(EventKind::Progress {
                        stage,
                        fraction: (code == 0 && !lookup).then_some(1.0),
                        detail: Some(format!(
                            "cutline {command} finished (exit {code}, {:.1} s)",
                            v["seconds"].as_f64().unwrap_or(0.0)
                        )),
                    });
                    if log.is_file() {
                        self.emit(EventKind::Artifact {
                            kind: "log".into(),
                            path: log.to_string(),
                            output: None,
                            bytes: None,
                        });
                    }
                }
                _ => {}
            }
        }
        let mut changed = false;
        for line in eleven_log.read_new() {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            changed = true;
            let command = v["command"].as_str().unwrap_or("eleven").to_string();
            if v["ok"].as_bool() == Some(false) {
                let error = v["error"].as_str().unwrap_or("it failed").to_string();
                self.eleven_failures.retain(|(c, ..)| c != &command);
                self.eleven_failures.push((command, v["out"].as_str().map(|o| self.shown(o)), error));
            } else if v["ok"].as_bool() == Some(true) {
                self.eleven_failures.retain(|(c, ..)| c != &command);
                self.eleven_credits += v["credits"].as_u64().unwrap_or(0);
                if let Some(out) = v["out"].as_str() {
                    self.emit(EventKind::Artifact {
                        kind: "audio".into(),
                        path: out.to_string(),
                        output: None,
                        bytes: v["bytes"].as_u64(),
                    });
                }
            }
        }
        if changed {
            self.emit_cost(true);
        }
    }

    fn finish(mut self, launch_error: Option<String>, exit: Option<(i32, String)>, cancelled: bool) -> RunOutcome {
        let kind = self.p.request.kind;
        let folder = self.p.folder.clone();
        self.eleven_credits = eleven::spent(&self.p.helper_context.log_path());
        // Calls the hook refused never reach the host, so count them from its log.
        self.denials += std::fs::read_to_string(self.p.logs.join("permissions.ndjson"))
            .map(|t| t.lines().filter(|l| l.contains("\"decision\":\"deny\"")).count() as u32)
            .unwrap_or(0);

        // What went wrong, if anything.
        let mut message: Option<String> = None;
        let mut failed = false;
        if let Some(e) = launch_error {
            failed = true;
            message = Some(e);
        } else if cancelled {
            message = Some("The run was cancelled. Partial outputs are kept and flagged incomplete.".into());
        } else if let Some(e) = self.result_error.clone() {
            failed = true;
            message = Some(e);
        } else if !self.saw_result {
            failed = true;
            let (code, stderr) = exit.clone().unwrap_or((-1, String::new()));
            let err = stderr.trim();
            message = Some(friendly_error(&if err.is_empty() {
                format!("{} exited unexpectedly (status {code}) before finishing.", self.agent_name())
            } else {
                err.to_string()
            }));
        }

        let readiness = if kind == RunKind::DryRun { ReadinessReport::load(&folder) } else { None };
        let ok_so_far = !failed && !cancelled;
        let outputs = match kind {
            RunKind::Production => collect::collect_outputs(&folder, &self.p.outputs, ok_so_far),
            RunKind::DryRun => Vec::new(),
            RunKind::AudioReview => self
                .p
                .outputs
                .first()
                .map(|o| vec![collect::collect_audio_review(&folder, o, ok_so_far)])
                .unwrap_or_default(),
        };
        if ok_so_far {
            match kind {
                RunKind::Production => {
                    let missing: Vec<&str> =
                        outputs.iter().filter(|o| !o.exists || o.bytes == 0).map(|o| o.export.as_str()).collect();
                    if !missing.is_empty() {
                        failed = true;
                        message = Some(format!(
                            "{} finished without producing {}: {}.{} See summary.md and the cutline logs for why.",
                            self.agent_short(),
                            if missing.len() == 1 { "this output" } else { "these outputs" },
                            missing.join(", "),
                            self.first_failure_note()
                        ));
                    }
                    for o in outputs.iter().filter(|o| o.exists) {
                        match o.qa_verdict.as_str() {
                            "fail" => self
                                .warnings
                                .push(format!("{}: QA failed with {} error(s).", o.output_name, o.qa_errors)),
                            "warnings" => self
                                .warnings
                                .push(format!("{}: QA passed with {} warning(s).", o.output_name, o.qa_warnings)),
                            "none" => self.warnings.push(format!("{}: there is no QA report.", o.output_name)),
                            _ => {}
                        }
                    }
                    if !folder.join("summary.md").is_file() {
                        self.warnings.push(format!("{} didn't write summary.md.", self.agent_short()));
                    }
                }
                RunKind::AudioReview => {
                    if outputs.iter().all(|o| !o.exists || o.bytes <= 44) {
                        failed = true;
                        message = Some(format!(
                            "{} finished the Audio Review without mixing the soundtrack ({}).{} See summary.md and the cutline logs for why.",
                            self.agent_short(),
                            collect::AUDIO_REVIEW_WAV,
                            self.first_failure_note()
                        ));
                    } else if outputs.iter().any(|o| collect::wav_is_silent(Utf8Path::new(&o.path))) {
                        self.warnings.push("The timeline has no audible tracks: the mix is silence.".into());
                    }
                    if !folder.join("summary.md").is_file() {
                        self.warnings.push(format!("{} didn't write summary.md.", self.agent_short()));
                    }
                }
                RunKind::DryRun => {
                    if !folder.join("project.json5").is_file() {
                        failed = true;
                        message = Some("The dry run didn't get as far as writing project.json5.".into());
                    } else if readiness.is_none() {
                        failed = true;
                        message = Some("The dry run didn't write a readiness report (readiness.json).".into());
                    } else {
                        if let Some(code) = self.last_validate_code.filter(|c| *c != 0) {
                            self.warnings.push(format!(
                                "`cutline validate` was still failing at the end of the dry run (exit {code})."
                            ));
                        }
                        if let Some(r) = &readiness {
                            if !r.is_clean() || !r.ready {
                                self.warnings.push(format!(
                                    "The dry run found {} point(s) to look at before a production run.",
                                    r.ambiguities.len() + r.missing_media.len() + r.risks.len()
                                ));
                            }
                        }
                    }
                }
            }
        }
        // Writes that never completed are named, whatever else happened (BUG-green-cricket-P6HQ).
        if let Some(paths) = self.unfinished_writes() {
            if failed {
                // unless the failure already names each of them (the patch deadline does)
                if let Some(m) = message.as_mut().filter(|m| !paths.split(", ").all(|p| m.contains(p))) {
                    m.push_str(&format!(" These changes never completed: {paths}."));
                }
            } else if !cancelled {
                self.warn(format!("{} never reported these changes complete: {paths}.", self.agent_short()));
            }
        }
        // An ElevenLabs call that failed and wasn't made again is said with its own error,
        // whatever else stopped the run (BUG-deft-lily-344X).
        if !self.eleven_failures.is_empty() && !cancelled {
            let said: Vec<String> = self
                .eleven_failures
                .iter()
                .map(|(command, out, error)| {
                    let what = match out {
                        Some(o) => format!("`video-studio eleven {command}` (for `{o}`)"),
                        None => format!("`video-studio eleven {command}`"),
                    };
                    format!("{what} failed: {}", error.trim().trim_end_matches('.'))
                })
                .collect();
            let note = format!("ElevenLabs: {}.", said.join("; "));
            if failed {
                if let Some(m) = message.as_mut().filter(|m| !m.contains(&note)) {
                    m.push(' ');
                    m.push_str(&note);
                }
            } else {
                self.warn(note);
            }
        }
        // Capture that needed a permission the app doesn't have is said, even when the run
        // delivered something else in its place (BUG-stellar-lily-K8CN).
        let blocked = crate::capture::blocked_permissions(&self.p.helper_context.logs_dir);
        if !blocked.is_empty() && !cancelled {
            let note = capture_blocked_note(&blocked);
            if failed {
                if let Some(m) = message.as_mut().filter(|m| !m.contains(&note)) {
                    m.push(' ');
                    m.push_str(&note);
                }
            } else {
                self.warn(note);
            }
        }
        // An Audio Review that failed for another reason also says the soundtrack wasn't mixed
        // (BUG-rustic-olive-U4PY).
        if kind == RunKind::AudioReview && failed && outputs.iter().all(|o| !o.exists || o.bytes <= 44) {
            if let Some(m) = message.as_mut().filter(|m| !m.contains(collect::AUDIO_REVIEW_WAV)) {
                m.push_str(&format!(" No soundtrack was mixed ({}).", collect::AUDIO_REVIEW_WAV));
            }
        }
        // The last stage succeeds only if the run did: a summary written before a failure isn't a
        // finished wrap-up (BUG-rustic-olive-U4PY).
        if failed || cancelled {
            if let Some(stage) = self.stage.as_mut() {
                stage.3 = false;
            }
        }
        self.finish_stage();
        let status = if cancelled {
            RunStatus::Cancelled
        } else if failed {
            RunStatus::Failed
        } else if self.warnings.is_empty() {
            RunStatus::Succeeded
        } else {
            RunStatus::SucceededWithWarnings
        };

        // Artifacts
        for o in outputs.iter().filter(|o| o.exists) {
            self.emit(EventKind::Artifact {
                kind: "output".into(),
                path: o.path.clone(),
                output: Some(o.output_name.clone()),
                bytes: Some(o.bytes),
            });
            for (k, p) in [("qa", &o.qa_report), ("captions", &o.captions_srt)] {
                if let Some(p) = p {
                    self.emit(EventKind::Artifact {
                        kind: k.into(),
                        path: p.clone(),
                        output: Some(o.output_name.clone()),
                        bytes: None,
                    });
                }
            }
        }
        for (name, kind_name) in
            [("project.json5", "project"), ("summary.md", "summary"), ("readiness.json", "summary")]
        {
            let p = folder.join(name);
            if p.is_file() {
                self.emit(EventKind::Artifact {
                    kind: kind_name.into(),
                    path: p.to_string(),
                    output: None,
                    bytes: None,
                });
            }
        }
        if !self.warnings.is_empty() && status == RunStatus::SucceededWithWarnings {
            for w in self.warnings.clone() {
                // warnings raised during the run were already emitted
                if w.contains("QA")
                    || w.contains("summary.md")
                    || w.contains("dry run")
                    || w.contains("cutline validate")
                {
                    self.emit(EventKind::Warning { message: w });
                }
            }
        }

        // Logs: QA copies, the summary with the app's own status, and the final run record.
        collect::copy_qa_to_logs(&folder, &outputs);
        let cost = self.final_usage.as_ref().and_then(|u| u.cost_usd).or(self.estimated_cost().0.filter(|c| *c > 0.0));
        let seconds = self.started.elapsed().as_secs_f64();
        let finished_at = timeutil::now_rfc3339();
        let mut summary = String::new();
        if let Ok(text) = std::fs::read_to_string(folder.join("summary.md")) {
            summary.push_str(text.trim_end());
            summary.push_str("\n\n---\n\n");
        }
        summary.push_str(&format!("## Run status (written by {})\n\n", crate::APP_NAME));
        summary.push_str(&format!(
            "- Status: **{}**\n- Kind: {}\n- Assistant: {}\n- Run config: {}\n- Model: {}\n- Duration: {}\n",
            status.label(),
            match kind {
                RunKind::DryRun => "dry run",
                RunKind::Production => "production",
                RunKind::AudioReview => "Audio Review",
            },
            self.agent_name(),
            self.p.run_config.name,
            self.model_used.clone().or(self.p.record.model.clone()).unwrap_or_default(),
            crate::media::format_duration(seconds)
        ));
        if let Some(c) = cost {
            summary.push_str(&format!(
                "- {} cost: ${c:.2}{}\n",
                self.agent_short(),
                if self.final_usage.as_ref().and_then(|u| u.cost_usd).is_some() {
                    ""
                } else {
                    " (estimated from token counts)"
                }
            ));
        }
        summary.push_str(&format!(
            "- ElevenLabs credits: {} of {}\n",
            self.eleven_credits, self.p.run_config.eleven_credit_cap
        ));
        if let Some(m) = &message {
            summary.push_str(&format!("- Note: {m}\n"));
        }
        if self.denials > 0 {
            summary.push_str(&format!("- Blocked tool calls: {}{}\n", self.denials, {
                // only logs that exist (BUG-green-cricket-P6HQ)
                let logs: Vec<String> = ["permissions.ndjson", "codex.jsonl", "claude.jsonl"]
                    .iter()
                    .filter(|f| self.p.logs.join(f).is_file())
                    .map(|f| format!("logs/{f}"))
                    .collect();
                if logs.is_empty() {
                    String::new()
                } else {
                    format!(" (see {})", logs.join(" and "))
                }
            }));
        }
        if !outputs.is_empty() {
            summary.push_str("\n| Output | File | Size | QA |\n|---|---|---|---|\n");
            for o in &outputs {
                let file = Utf8Path::new(&o.path)
                    .strip_prefix(&folder)
                    .map(|p| p.to_string())
                    .unwrap_or_else(|_| o.path.clone());
                let state = if !o.exists {
                    "missing".to_string()
                } else if o.incomplete {
                    "incomplete".to_string()
                } else {
                    paths::format_bytes(o.bytes)
                };
                summary.push_str(&format!("| {} | {file} | {state} | {} |\n", o.output_name, o.qa_verdict));
            }
        }
        if !self.warnings.is_empty() {
            summary.push_str("\nWarnings:\n");
            for w in &self.warnings {
                summary.push_str(&format!("- {w}\n"));
            }
        }
        let _ = paths::write_atomic(&self.p.logs.join("summary.md"), summary.as_bytes());

        let mut record = self.p.record.clone();
        record.status = status;
        record.finished = Some(finished_at.clone());
        record.claude_session = self.session.clone();
        if self.model_used.is_some() {
            record.model = self.model_used.clone();
        }
        record.cost_usd = cost;
        record.eleven_credits = Some(self.eleven_credits);

        let run_json = self.p.logs.join("run.json");
        if let Ok(mut v) = paths::read_json::<Value>(&run_json) {
            if let Some(o) = v.as_object_mut() {
                o.insert("status".into(), json!(status));
                o.insert("finished".into(), json!(finished_at));
                o.insert("seconds".into(), json!(seconds));
                o.insert("claude_session".into(), json!(self.session));
                o.insert("model_used".into(), json!(record.model));
                if let (Some(c), Some(v)) = (o.get_mut("codex").and_then(Value::as_object_mut), &self.cli_version) {
                    c.insert("version".into(), json!(v));
                }
                o.insert("cost_usd".into(), json!(cost));
                o.insert("usage".into(), json!(self.final_usage));
                o.insert("eleven_credits".into(), json!(self.eleven_credits));
                o.insert("message".into(), json!(message));
                o.insert("warnings".into(), json!(self.warnings));
                o.insert("blocked_tool_calls".into(), json!(self.denials));
                o.insert("exit".into(), json!(exit.as_ref().map(|e| e.0)));
                o.insert("results".into(), json!(outputs));
            }
            let _ = paths::write_json_atomic(&run_json, &v);
        }
        self.save_record(&record);
        crate::capture::stop_stray(&crate::capture::state_file(&self.p.context_dir));
        let _ = std::fs::remove_dir_all(&self.p.context_dir);

        let code = exit_code::for_status(status);
        self.emit(EventKind::StageStarted { stage: "done".into(), label: "Done".into() });
        self.emit(EventKind::RunFinished {
            status,
            exit_code: code,
            cost_usd: cost,
            eleven_credits: self.eleven_credits,
            seconds,
            message: message.clone(),
        });
        RunOutcome {
            run_id: self.p.run_id.clone(),
            folder: folder.to_string(),
            status,
            exit_code: code,
            record,
            outputs,
            readiness,
            warnings: self.warnings.clone(),
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BUG-stellar-lily-K8CN: the run result names each missing permission and what it stops.
    #[test]
    fn blocked_capture_is_named_in_the_run_result() {
        let screen = capture_blocked_note(&[crate::capture::SCREEN_RECORDING.to_string()]);
        assert!(
            screen.starts_with(
                "Desktop capture was blocked: recording and screenshots need Screen & System Audio Recording, which"
            ),
            "{screen}"
        );
        let both =
            capture_blocked_note(&[crate::capture::SCREEN_RECORDING.into(), crate::capture::ACCESSIBILITY.into()]);
        assert!(both.contains("; clicking, typing and menus need Accessibility"), "{both}");
        assert!(both.contains("Anything delivered was made without it."), "{both}");
    }

    #[test]
    fn an_operation_is_the_command_without_its_options() {
        use serde_json::json;
        let bash = |c: &str| operation("Bash", &json!({"command": c}));
        assert_eq!(
            bash("bash -lc 'video-studio eleven music --prompt \"calm piano\" --length 49 --out music.mp3'"),
            "video-studio eleven music"
        );
        assert_eq!(bash("video-studio eleven quote music --length 49"), "video-studio eleven quote");
        assert_eq!(bash("PATH=/x /bin/cutline audio project.json5 --export youtube-shorts"), "cutline audio");
        assert_ne!(bash("video-studio eleven music --length 10"), bash("video-studio eleven quote music"));
        assert_eq!(operation("Write", &json!({"file_path": "/r/summary.md"})), "Write /r/summary.md");
    }

    #[test]
    fn a_failed_tool_call_says_what_was_tried_and_why() {
        use serde_json::json;
        let root = Utf8Path::new("/runs/r1");
        let bash = |cmd: &str, out: &str| tool_failure("Bash", &json!({"command": cmd}), root, out);
        assert_eq!(
            bash("cutline validate --json project.json5", "Exit code 1\n{\n  \"errors\": [{\"code\": \"E_MISSING_FIELD\", \"hint\": \"add `version: 1`\", \"message\": \"missing field `version`\"}],\n  \"ok\": false\n}"),
            "`cutline validate --json project.json5` found 1 problem to fix: missing field `version` (add `version: 1`)"
        );
        assert_eq!(
            bash("cutline transcribe audio/a.mp3 audio/b.mp3", "Exit code 2\nerror: unexpected argument 'audio/b.mp3' found\nUsage: cutline transcribe [OPTIONS] <FILE>"),
            "`cutline transcribe audio/a.mp3 audio/b.mp3` didn't work (exit code 2): unexpected argument 'audio/b.mp3' found"
        );
        assert_eq!(
            bash("cutline render project.json5", "Exit code 3"),
            "`cutline render project.json5` ended with exit code 3"
        );
        assert_eq!(
            tool_failure(
                "Read",
                &json!({"file_path": "/runs/r1/work/notes.md"}),
                root,
                "File does not exist. Note: your current working directory is /runs/r1."
            ),
            "Couldn't read `work/notes.md`: File does not exist"
        );
        assert_eq!(
            tool_failure(
                "Edit",
                &json!({"file_path": "/runs/r1/project.json5"}),
                root,
                "<tool_use_error>No changes to make: old_string and new_string are exactly the same.</tool_use_error>"
            ),
            "No change made to `project.json5`: the new text is the same as the old"
        );
        assert!(!bash("x", "Exit code 1\nboom").contains("failed"));
    }

    #[test]
    fn tool_summaries() {
        let root = Utf8Path::new("/runs/r1");
        assert_eq!(
            tool_summary("Write", &json!({"file_path": "/runs/r1/project.json5"}), root),
            "Wrote `project.json5`"
        );
        assert_eq!(tool_summary("Read", &json!({"file_path": "/footage/hero.mov"}), root), "Read `/footage/hero.mov`");
        assert_eq!(
            tool_summary("Bash", &json!({"command": "cutline validate project.json5"}), root),
            "Ran `cutline validate project.json5`"
        );
        assert_eq!(tool_summary("Grep", &json!({"pattern": "cue:"}), root), "Searched for `cue:`");
        assert_eq!(truncate("abcdef", 3), "abc…");
    }

    #[test]
    fn tail_reads_only_complete_new_lines() {
        let d = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(d.path()).unwrap().join("a.ndjson");
        let mut t = Tail::new(path.clone());
        assert!(t.read_new().is_empty(), "a missing file is just empty");
        std::fs::write(&path, "one\ntwo\npart").unwrap();
        assert_eq!(t.read_new(), vec!["one", "two"]);
        assert!(t.read_new().is_empty());
        std::fs::write(&path, "one\ntwo\npartial done\nthree\n").unwrap();
        assert_eq!(t.read_new(), vec!["partial done", "three"]);
    }
}
