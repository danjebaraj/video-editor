//! Spec assistant sessions (spec §4.3): `claude -p` over stream-json with `--resume` per session
//! and the permission broker. Claude edits a materialized `spec.md` in a scratch folder; the
//! shell shows the change as a per-hunk diff and writes accepted hunks back to the project.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::claude::broker::{hook_settings, Mode, Policy};
use crate::claude::locate::{child_env, ClaudeInfo};
use crate::claude::process::{containing_dir, friendly_error, ClaudeOptions, ClaudeProcess, Incoming};
use crate::claude::stream::{self, Question, StreamEvent};
use crate::claude::Decision;
use crate::run::orchestrator::{tool_failure, tool_summary};
use crate::{paths, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantRequest {
    /// The session's scratch folder; `spec.md` is materialized here.
    pub session_dir: Utf8PathBuf,
    /// The spec as it is in the editor now.
    pub spec: String,
    pub prompt: String,
    pub model: String,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub web_access: bool,
    /// Claude session to resume, from an earlier turn of this conversation.
    #[serde(default)]
    pub resume: Option<String>,
    /// Media folders (or files) Claude may read.
    #[serde(default)]
    pub read_paths: Vec<Utf8PathBuf>,
    /// Extra folders the user shared with the assistant (briefs, docs, source material). Read-only.
    #[serde(default)]
    pub reference_paths: Vec<Utf8PathBuf>,
    /// Intent, brand, outputs and the media list (see `run::prompt::project_summary`).
    #[serde(default)]
    pub project_summary: String,
    /// The media list (`project.summary`'s `media`); HEIC/HEIF photos in it get JPEG copies in
    /// the session folder before Claude starts.
    #[serde(default)]
    pub media: Vec<crate::run::prepare::ManifestEntry>,
    /// cutline, which makes those copies. Without it no copies are made.
    #[serde(default)]
    pub cutline: Option<Utf8PathBuf>,
    /// The `video-studio` binary, for the pre-tool hook. Without it only the stdio broker runs.
    #[serde(default)]
    pub helper: Option<Utf8PathBuf>,
    /// Extra environment for the Claude Code process (proxies, test doubles).
    #[serde(default)]
    pub extra_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AssistantEvent {
    Started {
        session_id: String,
        model: Option<String>,
    },
    TextDelta {
        text: String,
    },
    Text {
        text: String,
    },
    Tool {
        id: String,
        name: String,
        summary: String,
        detail: Option<String>,
    },
    ToolResult {
        id: String,
        is_error: bool,
        detail: Option<String>,
        /// For an error: what was tried and why it didn't work, to show in place of the summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Blocked {
        reason: String,
    },
    Question {
        id: String,
        questions: Vec<Question>,
    },
    /// Claude changed `spec.md`: the whole new text, to diff against the editor's.
    SpecChanged {
        spec: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
    },
    Error {
        message: String,
    },
    Finished {
        ok: bool,
        stopped: bool,
        session_id: Option<String>,
    },
}

pub trait AssistantSink: Send + Sync {
    fn emit(&self, event: &AssistantEvent);
}

struct Shared {
    stop: AtomicBool,
    finished: AtomicBool,
    process: Mutex<Option<ClaudeProcess>>,
    answers: Mutex<HashMap<String, Option<BTreeMap<String, String>>>>,
}

/// A running assistant turn.
pub struct AssistantHandle {
    shared: Arc<Shared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl AssistantHandle {
    /// Stops the turn: interrupt, then terminate.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
    /// Answers a question card: question text → answer.
    pub fn answer(&self, question_id: &str, answers: BTreeMap<String, String>) {
        self.shared.answers.lock().unwrap().insert(question_id.to_string(), Some(answers));
    }
    /// Dismisses a question card without answering.
    pub fn dismiss(&self, question_id: &str) {
        self.shared.answers.lock().unwrap().insert(question_id.to_string(), None);
    }
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::SeqCst)
    }
    pub fn wait(&self) {
        if let Some(t) = self.thread.lock().unwrap().take() {
            let _ = t.join();
        }
    }
}

pub fn spec_file(session_dir: &Utf8Path) -> Utf8PathBuf {
    session_dir.join("spec.md")
}

/// Starts one assistant turn. Events arrive on `sink` from a background thread; the last one is
/// always `Finished`.
pub fn start(request: &AssistantRequest, claude: &ClaudeInfo, sink: Arc<dyn AssistantSink>) -> Result<AssistantHandle> {
    let dir = paths::canonical_or_normal({
        std::fs::create_dir_all(&request.session_dir)?;
        &request.session_dir
    });
    let spec_path = spec_file(&dir);
    paths::write_atomic(&spec_path, request.spec.as_bytes())?;
    // Photos Claude can't open get JPEG copies inside the session folder, which it already reads
    let mut media = request.media.clone();
    if let Some(cutline) = &request.cutline {
        crate::viewable::ensure_viewable_copies(&mut media, &dir, cutline);
    }
    let project_summary = format!("{}{}", request.project_summary, crate::run::prompt::viewable_list(&media));
    let read_paths: Vec<Utf8PathBuf> = request.read_paths.iter().filter(|p| p.exists()).cloned().collect();
    let reference_paths: Vec<Utf8PathBuf> =
        request.reference_paths.iter().filter(|p| p.is_absolute() && p.exists()).cloned().collect();
    // Claude Code keeps large tool results in its own folder for this working directory.
    let mut policy_reads = read_paths.clone();
    policy_reads.extend(reference_paths.iter().cloned());
    policy_reads.extend(crate::claude::locate::claude_project_dir(&dir));
    let policy = Policy {
        mode: Mode::Assistant,
        work_dir: dir.clone(),
        read_paths: policy_reads,
        write_files: Some(vec![spec_path.clone()]),
        // only `cutline still`, for JPEG copies of HEIC/HEIF photos outside the media list
        cutline: request.cutline.clone(),
        helper: None,
        capture: false,
        path_dirs: request.cutline.iter().filter_map(|c| c.parent().map(Utf8Path::to_path_buf)).collect(),
        web_access: request.web_access,
        log: None,
    };
    // The policy file sits next to the session folder, outside what Claude can read.
    let policy_file = dir.with_file_name(format!("{}.policy.json", dir.file_name().unwrap_or("session")));
    let settings = match &request.helper {
        Some(helper) => {
            paths::write_json_atomic(&policy_file, &policy)?;
            Some(hook_settings(helper, &policy_file))
        }
        None => None,
    };
    let mut tools: Vec<String> =
        ["Read", "Edit", "Write", "Glob", "Grep", "AskUserQuestion"].iter().map(|s| s.to_string()).collect();
    if request.web_access {
        tools.extend(["WebSearch".to_string(), "WebFetch".to_string()]);
    }
    if request.cutline.is_some() {
        tools.push("Bash".to_string());
    }
    let options = ClaudeOptions {
        executable: Utf8PathBuf::from(&claude.path),
        working_dir: dir.clone(),
        env: {
            let mut e = child_env(claude.login_path.as_deref());
            e.extend(request.extra_env.clone());
            e
        },
        model: request.model.clone(),
        effort: request.effort.clone().filter(|e| claude.effort_levels.contains(e)),
        tools,
        resume_session: request.resume.clone(),
        system_prompt_append: crate::run::prompt::assistant_system_prompt(
            &dir,
            &project_summary,
            &read_paths.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
            &reference_paths.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
            request.web_access,
            request.cutline.as_deref(),
        ),
        settings,
        add_dirs: read_paths
            .iter()
            .chain(reference_paths.iter())
            .map(|p| containing_dir(p))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect(),
        max_budget_usd: None,
        log: Some(dir.join("claude-last-run.jsonl")),
    };
    // Each turn's log replaces the last one.
    let _ = std::fs::remove_file(dir.join("claude-last-run.jsonl"));
    let (process, rx) = ClaudeProcess::start(&options, &request.prompt)?;
    let shared = Arc::new(Shared {
        stop: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        process: Mutex::new(Some(process.clone())),
        answers: Mutex::new(HashMap::new()),
    });
    let thread_shared = shared.clone();
    let original = request.spec.clone();
    let thread = std::thread::Builder::new().name("video-studio-assistant".into()).spawn(move || {
        let emit = |e: AssistantEvent| sink.emit(&e);
        let mut last_spec = original;
        let mut session: Option<String> = None;
        let mut saw_result = false;
        let mut ok = true;
        let mut stop_sent = false;
        let mut tools: HashMap<String, (String, serde_json::Value)> = HashMap::new();
        let mut pending: HashMap<String, Value> = HashMap::new();
        let mut exit: Option<(i32, String)> = None;
        let check_spec = |last: &mut String| {
            if let Ok(now) = std::fs::read_to_string(&spec_path) {
                let now = now.replace("\r\n", "\n");
                if &now != last {
                    *last = now.clone();
                    emit(AssistantEvent::SpecChanged { spec: now });
                }
            }
        };
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Incoming::Event(e)) => match e {
                    StreamEvent::Init { session_id, model, .. } => {
                        session = Some(session_id.clone());
                        emit(AssistantEvent::Started { session_id, model });
                    }
                    StreamEvent::TextDelta(text) => emit(AssistantEvent::TextDelta { text }),
                    StreamEvent::AssistantText(text) => emit(AssistantEvent::Text { text }),
                    StreamEvent::ToolUse { id, name, input } => {
                        tools.insert(id.clone(), (name.clone(), input.clone()));
                        if name != "AskUserQuestion" {
                            let detail = serde_json::to_string_pretty(&input).ok().map(|d| d.chars().take(4000).collect());
                            emit(AssistantEvent::Tool { id, summary: tool_summary(&name, &input, &dir), name, detail });
                        }
                    }
                    StreamEvent::ToolResult { tool_use_id, content, is_error } => {
                        let (name, input) = tools.remove(&tool_use_id).unwrap_or_default();
                        if name != "AskUserQuestion" {
                            emit(AssistantEvent::ToolResult {
                                id: tool_use_id,
                                is_error,
                                detail: is_error.then(|| content.chars().take(4000).collect()),
                                text: is_error.then(|| tool_failure(&name, &input, &dir, &content)),
                            });
                        }
                        if !is_error && matches!(name.as_str(), "Edit" | "Write" | "MultiEdit") {
                            check_spec(&mut last_spec);
                        }
                    }
                    StreamEvent::PermissionRequest { request_id, tool_name, input, .. } => match policy.decide(&tool_name, &input) {
                        Decision::Allow => process.allow(&request_id, &input),
                        Decision::Deny(reason) => {
                            process.deny(&request_id, &format!("{reason} This is blocked by Syncfusion Video Studio."));
                            emit(AssistantEvent::Blocked { reason });
                        }
                        Decision::AskUser => {
                            let questions = stream::questions(&input);
                            if questions.is_empty() {
                                process.allow(&request_id, &input);
                            } else {
                                emit(AssistantEvent::Question { id: request_id.clone(), questions });
                                pending.insert(request_id, input);
                            }
                        }
                    },
                    StreamEvent::Result { is_error, text, session_id, subtype, usage } => {
                        saw_result = true;
                        if session_id.is_some() {
                            session = session_id;
                        }
                        if (is_error || subtype.as_deref().map(|s| s != "success").unwrap_or(false)) && !thread_shared.stop.load(Ordering::SeqCst) {
                            ok = false;
                            emit(AssistantEvent::Error { message: friendly_error(&text.unwrap_or_else(|| format!("Claude reported an error ({}).", subtype.as_deref().unwrap_or("unknown")))) });
                        }
                        if let Some(u) = usage {
                            emit(AssistantEvent::Usage { input_tokens: u.total_input(), output_tokens: u.output_tokens, cost_usd: u.cost_usd, duration_ms: u.duration_ms });
                        }
                        process.finish_input();
                    }
                    StreamEvent::MessageUsage { .. } | StreamEvent::ControlResponse { .. } | StreamEvent::Ignored(_) => {}
                },
                Ok(Incoming::Exit { code, stderr }) => {
                    exit = Some((code, stderr));
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            // Answers to question cards
            let answered: Vec<(String, Option<BTreeMap<String, String>>)> = {
                let mut a = thread_shared.answers.lock().unwrap();
                let ids: Vec<String> = a.keys().filter(|k| pending.contains_key(*k)).cloned().collect();
                ids.into_iter().filter_map(|id| a.remove(&id).map(|v| (id, v))).collect()
            };
            for (id, answer) in answered {
                if let Some(input) = pending.remove(&id) {
                    match answer {
                        Some(answers) => process.allow(&id, &stream::answered_input(&input, &answers)),
                        None => process.deny(&id, "The user dismissed the question. Continue with your best judgement and say what you assumed."),
                    }
                }
            }
            if thread_shared.stop.load(Ordering::SeqCst) && !stop_sent {
                stop_sent = true;
                for id in pending.drain().map(|(id, _)| id).collect::<Vec<_>>() {
                    process.deny(&id, "The user stopped this turn.");
                }
                let p = process.clone();
                std::thread::spawn(move || {
                    p.stop(Duration::from_millis(2500));
                });
            }
        }
        if process.is_running() {
            process.kill();
        }
        check_spec(&mut last_spec);
        let stopped = thread_shared.stop.load(Ordering::SeqCst);
        if !saw_result && !stopped {
            ok = false;
            let (code, stderr) = exit.unwrap_or((-1, String::new()));
            let err = stderr.trim();
            emit(AssistantEvent::Error { message: friendly_error(&if err.is_empty() { format!("Claude Code exited unexpectedly (status {code}).") } else { err.to_string() }) });
        }
        let _ = std::fs::remove_file(&policy_file);
        *thread_shared.process.lock().unwrap() = None;
        thread_shared.finished.store(true, Ordering::SeqCst);
        emit(AssistantEvent::Finished { ok: ok && !stopped, stopped, session_id: session });
    })?;
    Ok(AssistantHandle { shared, thread: Mutex::new(Some(thread)) })
}

// MARK: Diff review

/// One contiguous change between the editor's spec and Claude's version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hunk {
    /// 0-based index of the first replaced line in the old text.
    pub old_start: usize,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
}

/// Line diff as hunks (longest common subsequence). Texts too large for the table fall back to
/// one hunk covering the changed middle.
pub fn diff_hunks(old: &str, new: &str) -> Vec<Hunk> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let prefix = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..].iter().rev().zip(b[prefix..].iter().rev()).take_while(|(x, y)| x == y).count();
    let am = &a[prefix..a.len() - suffix];
    let bm = &b[prefix..b.len() - suffix];
    if am.is_empty() && bm.is_empty() {
        return Vec::new();
    }
    let owned = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    if am.len().saturating_mul(bm.len()) > 4_000_000 {
        return vec![Hunk { old_start: prefix, old_lines: owned(am), new_lines: owned(bm) }];
    }
    let (n, m) = (am.len(), bm.len());
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[at(i, j)] =
                if am[i] == bm[j] { lcs[at(i + 1, j + 1)] + 1 } else { lcs[at(i + 1, j)].max(lcs[at(i, j + 1)]) };
        }
    }
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut open: Option<Hunk> = None;
    while i < n || j < m {
        if i < n && j < m && am[i] == bm[j] {
            if let Some(h) = open.take() {
                hunks.push(h);
            }
            i += 1;
            j += 1;
            continue;
        }
        let h =
            open.get_or_insert_with(|| Hunk { old_start: prefix + i, old_lines: Vec::new(), new_lines: Vec::new() });
        if j < m && (i == n || lcs[at(i, j + 1)] >= lcs[at(i + 1, j)]) {
            h.new_lines.push(bm[j].to_string());
            j += 1;
        } else {
            h.old_lines.push(am[i].to_string());
            i += 1;
        }
    }
    if let Some(h) = open {
        hunks.push(h);
    }
    hunks
}

/// Applies the accepted hunks to the old text; rejected hunks keep the old lines.
pub fn apply_hunks(old: &str, hunks: &[Hunk], accepted: &[bool]) -> String {
    let lines: Vec<&str> = old.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut pos = 0;
    for (h, take) in hunks.iter().zip(accepted.iter().chain(std::iter::repeat(&false))) {
        if h.old_start < pos || h.old_start > lines.len() {
            continue;
        }
        out.extend(lines[pos..h.old_start].iter().map(|s| s.to_string()));
        let end = (h.old_start + h.old_lines.len()).min(lines.len());
        if *take {
            out.extend(h.new_lines.iter().cloned());
        } else {
            out.extend(lines[h.old_start..end].iter().map(|s| s.to_string()));
        }
        pos = end;
    }
    out.extend(lines[pos..].iter().map(|s| s.to_string()));
    let mut s = out.join("\n");
    if !out.is_empty() && (old.ends_with('\n') || old.is_empty()) {
        s.push('\n');
    }
    s
}

/// The app-side prompt context for a turn: what part of the spec the user is looking at.
pub fn compose_prompt(text: &str, selection: Option<(&str, usize, usize)>, attachments: &[String]) -> String {
    let mut ctx: Vec<String> = Vec::new();
    if let Some((sel, a, b)) = selection {
        ctx.push(format!("- Selected text in spec.md (lines {a}–{b}):\n\n```markdown\n{sel}\n```"));
    }
    if !attachments.is_empty() {
        ctx.push(format!(
            "- Media the user pointed at: {}",
            attachments.iter().map(|m| format!("media:{m}")).collect::<Vec<_>>().join(", ")
        ));
    }
    if ctx.is_empty() {
        text.to_string()
    } else {
        format!("{text}\n\n---\nContext from the editor:\n{}", ctx.join("\n"))
    }
}

/// What a session keeps between turns, stored by the shell next to its scratch folder.
pub fn sessions_dir(library: &Utf8Path, project_id: &str) -> Utf8PathBuf {
    library.join("assistant").join(project_id)
}

/// A `Value` describing the tools a session offers, for the UI's status line.
pub fn tools_description(web_access: bool) -> Value {
    json!({"edit": "spec.md", "shell": false, "web": web_access})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunks_cover_insert_delete_and_replace() {
        let old = "# Title\n\nIntro line\n\n## Scene 1\n- Duration: 5 s\n\n## Call to action\nGo.\n";
        let new = "# Title\n\nIntro line, sharper\n\n## Scene 1\n- Duration: 5 s\n- Media: media:m-hero\n\nGo.\n";
        let h = diff_hunks(old, new);
        assert_eq!(h.len(), 3, "{h:#?}");
        assert_eq!(
            h[0],
            Hunk { old_start: 2, old_lines: vec!["Intro line".into()], new_lines: vec!["Intro line, sharper".into()] }
        );
        assert_eq!(
            (h[1].old_start, h[1].old_lines.len(), h[1].new_lines.clone()),
            (6, 0, vec!["- Media: media:m-hero".to_string()])
        );
        assert_eq!((h[2].old_lines.clone(), h[2].new_lines.len()), (vec!["## Call to action".to_string()], 0));
        // accepting everything gives Claude's text; nothing gives the old text
        assert_eq!(apply_hunks(old, &h, &[true, true, true]), new);
        assert_eq!(apply_hunks(old, &h, &[false, false, false]), old);
        assert_eq!(apply_hunks(old, &h, &[]), old);
        // per-hunk: take the media line, reject the others
        let mixed = apply_hunks(old, &h, &[false, true, false]);
        assert_eq!(
            mixed,
            "# Title\n\nIntro line\n\n## Scene 1\n- Duration: 5 s\n- Media: media:m-hero\n\n## Call to action\nGo.\n"
        );
        assert!(diff_hunks(old, old).is_empty());
    }

    #[test]
    fn hunks_at_the_edges() {
        let h = diff_hunks("", "new\ntext\n");
        assert_eq!(apply_hunks("", &h, &[true]), "new\ntext\n");
        let h = diff_hunks("a\nb\n", "");
        assert_eq!(apply_hunks("a\nb\n", &h, &[true]), "");
        let h = diff_hunks("a\nb\nc", "x\na\nb\nc\ny");
        assert_eq!(h.len(), 2);
        assert_eq!(apply_hunks("a\nb\nc", &h, &[true, true]), "x\na\nb\nc\ny");
        assert_eq!(apply_hunks("a\nb\nc", &h, &[true, false]), "x\na\nb\nc");
    }

    #[test]
    fn prompt_context() {
        assert_eq!(compose_prompt("Tighten this", None, &[]), "Tighten this");
        let p = compose_prompt("Tighten this", Some(("- Duration: 5 s", 6, 6)), &["m-hero".into()]);
        assert!(p.contains("lines 6–6") && p.contains("media:m-hero") && p.starts_with("Tighten this\n\n---"));
    }
}
