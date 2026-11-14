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
use crate::claude::process::{friendly_error, ClaudeProcess, Incoming};
use crate::claude::stream::{self, StreamEvent, Usage};
use crate::claude::Decision;
use crate::project::{record_run, RunKind, RunRecord, RunStatus};
use crate::{eleven, paths, timeutil, Result};

/// How long Claude gets to stop on its own before the process tree is killed (spec §12.4).
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// The message Claude gets when nobody answers a question.
pub const ASSUME_MESSAGE: &str = "No one is available to answer. Proceed on your stated assumption (your first, recommended option), and record this question and the assumption you made in summary.md.";

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
    process: Mutex<Option<ClaudeProcess>>,
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
    tools: HashMap<String, (String, Option<String>)>,
    pending: HashMap<String, PendingQuestion>,
    usage: HashMap<String, Usage>,
    final_usage: Option<Usage>,
    eleven_credits: u64,
    session: Option<String>,
    model_used: Option<String>,
    result_error: Option<String>,
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
            result_error: None,
            saw_result: false,
            warnings: Vec::new(),
            denials: 0,
            last_validate_code: None,
            last_cost_emit: Instant::now() - Duration::from_secs(60),
        }
    }

    fn emit(&self, kind: EventKind) {
        self.sink.emit(&Event::now(kind));
    }

    fn note(&self, role: &str, text: impl Into<String>) {
        self.emit(EventKind::Message { role: role.into(), text: text.into(), tool: None, detail: None });
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
        self.emit(EventKind::StageStarted { stage: stage.into(), label: label.into() });
        self.stage = Some((stage.into(), label.into(), Instant::now(), true));
    }

    fn finish_stage(&mut self) {
        if let Some((stage, _, at, ok)) = self.stage.take() {
            self.emit(EventKind::StageFinished { stage, ok, seconds: at.elapsed().as_secs_f64() });
        }
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
        let (est, input, output) = self.estimated_cost();
        let reported = self.final_usage.as_ref().and_then(|u| u.cost_usd);
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

        match ClaudeProcess::start(&self.p.options, &self.p.prompt) {
            Err(e) => launch_error = Some(e.to_string()),
            Ok((process, rx)) => {
                *self.shared.process.lock().unwrap() = Some(process.clone());
                loop {
                    match rx.recv_timeout(Duration::from_millis(120)) {
                        Ok(Incoming::Event(e)) => self.handle(&process, e),
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
        let cancelled = self.shared.cancel.load(Ordering::SeqCst) && !turns_over;
        self.finish(launch_error, exit, cancelled)
    }

    fn handle(&mut self, process: &ClaudeProcess, e: StreamEvent) {
        match e {
            StreamEvent::Init { session_id, model, .. } => {
                self.session = Some(session_id);
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
                self.tools.insert(id, (name.clone(), stage.map(|s| s.0)));
                if name != "AskUserQuestion" {
                    let detail = serde_json::to_string_pretty(&input).ok().map(|d| truncate(&d, 4000));
                    self.emit(EventKind::Message {
                        role: "tool".into(),
                        text: tool_summary(&name, &input, &self.p.folder),
                        tool: Some(name),
                        detail,
                    });
                }
            }
            StreamEvent::ToolResult { tool_use_id, content, is_error } => {
                let Some((name, stage)) = self.tools.remove(&tool_use_id) else { return };
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
                        }
                    }
                    self.emit(EventKind::Message {
                        role: "tool".into(),
                        text: format!("{name} failed"),
                        tool: Some(name),
                        detail: Some(truncate(&content, 4000)),
                    });
                } else if let (Some(s), Some(cur)) = (&stage, self.stage.as_mut()) {
                    // a later success in the same stage clears an earlier failure (fix and retry)
                    if &cur.0 == s {
                        cur.3 = true;
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
                let failed = is_error || subtype.as_deref().map(|s| s != "success").unwrap_or(false);
                if failed && self.result_error.is_none() && !self.shared.cancel.load(Ordering::SeqCst) {
                    let msg = match subtype.as_deref() {
                        Some("error_max_budget_usd") => "The run reached its Claude spend limit before finishing. Raise the limit in the run config.".to_string(),
                        Some("error_max_turns") => "The run reached its turn limit before finishing.".to_string(),
                        _ => friendly_error(&text.unwrap_or_else(|| format!("Claude reported an error ({}).", subtype.as_deref().unwrap_or("unknown")))),
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

    fn decide(&mut self, process: &ClaudeProcess, request_id: String, tool: String, input: Value) {
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
                    self.warn(format!("Claude asked \"{q}\" and proceeded on its own assumption (no one was asked)."));
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

    fn tick(&mut self, process: &ClaudeProcess, activity: &mut Tail, eleven_log: &mut Tail) {
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
                self.warn(format!("Claude asked \"{text}\" and proceeded on its own assumption (no answer in time)."));
            }
        }
        self.tick_logs(activity, eleven_log);
    }

    fn tick_logs(&mut self, activity: &mut Tail, eleven_log: &mut Tail) {
        for line in activity.read_new() {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let stage = self.stage.as_ref().map(|s| s.0.clone()).unwrap_or_else(|| "authoring".into());
            match v["type"].as_str() {
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
                    self.emit(EventKind::Progress {
                        stage,
                        fraction: (code == 0).then_some(1.0),
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
            if v["ok"].as_bool() == Some(true) {
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
        self.finish_stage();
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
                format!("Claude Code exited unexpectedly (status {code}) before finishing.")
            } else {
                err.to_string()
            }));
        }

        let readiness = if kind == RunKind::DryRun { ReadinessReport::load(&folder) } else { None };
        let ok_so_far = !failed && !cancelled;
        let outputs = match kind {
            RunKind::Production => collect::collect_outputs(&folder, &self.p.outputs, ok_so_far),
            RunKind::DryRun => Vec::new(),
        };
        if ok_so_far {
            match kind {
                RunKind::Production => {
                    let missing: Vec<&str> =
                        outputs.iter().filter(|o| !o.exists || o.bytes == 0).map(|o| o.export.as_str()).collect();
                    if !missing.is_empty() {
                        failed = true;
                        message = Some(format!(
                            "Claude finished without producing {}: {}. See summary.md and the cutline logs for why.",
                            if missing.len() == 1 { "this output" } else { "these outputs" },
                            missing.join(", ")
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
                        self.warnings.push("Claude didn't write summary.md.".into());
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
            "- Status: **{}**\n- Kind: {}\n- Run config: {}\n- Model: {}\n- Duration: {}\n",
            status.label(),
            if kind == RunKind::DryRun { "dry run" } else { "production" },
            self.p.run_config.name,
            self.model_used.clone().or(self.p.record.model.clone()).unwrap_or_default(),
            crate::media::format_duration(seconds)
        ));
        if let Some(c) = cost {
            summary.push_str(&format!(
                "- Claude cost: ${c:.2}{}\n",
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
            summary.push_str(&format!(
                "- Blocked tool calls: {} (see logs/permissions.ndjson and logs/claude.jsonl)\n",
                self.denials
            ));
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
