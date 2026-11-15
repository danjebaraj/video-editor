//! Run progress for the terminal (spec §13): stage lines with a progress bar on a TTY, or
//! NDJSON events with `--json`.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Instant;

use video_studio_core::claude::stream::Question;
use video_studio_core::project::RunKind;
use video_studio_core::run::events::{Event, EventKind, EventSink};

/// `--json`: one event per line on stdout, the same schema as `events.ndjson`.
pub struct JsonSink;

impl EventSink for JsonSink {
    fn emit(&self, event: &Event) {
        if matches!(event.kind, EventKind::TextDelta { .. }) {
            return;
        }
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", event.to_json());
        let _ = out.flush();
    }
}

pub struct PendingQuestion {
    pub id: String,
    pub questions: Vec<Question>,
}

struct State {
    /// The tenth of progress last printed as a plain line (consoles without VT sequences).
    last_step: i32,
    bar_shown: bool,
    stage: String,
    stage_started: Instant,
    cost: Option<f64>,
    credits: u64,
}

/// Whether stderr understands VT escape sequences (the `\r\x1b[2K` that redraws the progress bar).
/// Unix terminals do; on Windows the console is asked to turn them on, and an old console that
/// can't gets plain progress lines instead (ui/spec.md §23.5).
pub fn vt_supported() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::Console::{
            GetConsoleMode, GetStdHandle, SetConsoleMode, CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            STD_ERROR_HANDLE,
        };
        unsafe {
            let Ok(h) = GetStdHandle(STD_ERROR_HANDLE) else { return false };
            let mut mode = CONSOLE_MODE::default();
            if GetConsoleMode(h, &mut mode).is_err() {
                return false;
            }
            mode.0 & ENABLE_VIRTUAL_TERMINAL_PROCESSING.0 != 0
                || SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING).is_ok()
        }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

pub struct TerminalSink {
    debug: bool,
    quiet_header: bool,
    tty: bool,
    /// The terminal redraws lines (VT sequences); without it progress is printed as plain lines.
    vt: bool,
    questions: Option<Mutex<Sender<PendingQuestion>>>,
    state: Mutex<State>,
}

impl TerminalSink {
    pub fn new(debug: bool, quiet_header: bool, questions: Option<Sender<PendingQuestion>>) -> Self {
        TerminalSink {
            debug,
            quiet_header,
            tty: std::io::stderr().is_terminal(),
            vt: std::io::stderr().is_terminal() && vt_supported(),
            questions: questions.map(Mutex::new),
            state: Mutex::new(State {
                last_step: -1,
                bar_shown: false,
                stage: String::new(),
                stage_started: Instant::now(),
                cost: None,
                credits: 0,
            }),
        }
    }

    fn clear_bar(&self, st: &mut State) {
        if st.bar_shown && self.vt {
            eprint!("\r\x1b[2K");
            st.bar_shown = false;
        }
    }

    fn line(&self, st: &mut State, text: &str) {
        self.clear_bar(st);
        eprintln!("{text}");
    }
}

fn bar(fraction: f64) -> String {
    let width = 24;
    let filled = ((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize;
    format!("[{}{}] {:>3.0}%", "#".repeat(filled), "-".repeat(width - filled), fraction * 100.0)
}

fn clip(s: &str, max: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() > max {
        format!("{}…", one_line.chars().take(max).collect::<String>())
    } else {
        one_line
    }
}

impl EventSink for TerminalSink {
    fn emit(&self, event: &Event) {
        let mut st = self.state.lock().unwrap();
        match &event.kind {
            EventKind::RunStarted { kind, run_config, folder, model, outputs, .. } => {
                if !self.quiet_header || self.debug {
                    let what = if *kind == RunKind::DryRun { "Dry run" } else { "Run" };
                    self.line(&mut st, &format!("{what} \"{run_config}\" with {model}: {}", outputs.join(", ")));
                }
                self.line(&mut st, &format!("  folder: {folder}"));
            }
            EventKind::StageStarted { stage, label } => {
                st.stage = stage.clone();
                st.stage_started = Instant::now();
                if stage != "done" {
                    self.line(&mut st, &format!("▸ {label}"));
                }
            }
            EventKind::StageFinished { ok, seconds, .. } => {
                if !ok {
                    self.line(
                        &mut st,
                        &format!("  (this step reported a failure after {seconds:.0} s; Claude is handling it)"),
                    );
                } else {
                    self.clear_bar(&mut st);
                }
            }
            EventKind::Progress { fraction, detail, .. } if self.tty && !self.vt => {
                // No line redrawing: a plain line at every tenth of the way.
                if let Some(f) = fraction {
                    let step = (f.clamp(0.0, 1.0) * 10.0).floor() as i32;
                    if step != st.last_step {
                        st.last_step = step;
                        eprintln!("  {} {}", bar(*f), clip(detail.as_deref().unwrap_or(""), 60));
                    }
                }
            }
            EventKind::Progress { fraction, detail, .. } => {
                if self.tty {
                    let elapsed = st.stage_started.elapsed().as_secs();
                    let text = match fraction {
                        Some(f) => format!("  {} {}", bar(*f), clip(detail.as_deref().unwrap_or(""), 60)),
                        None => format!("  {}s  {}", elapsed, clip(detail.as_deref().unwrap_or(""), 80)),
                    };
                    eprint!("\r\x1b[2K{text}");
                    let _ = std::io::stderr().flush();
                    st.bar_shown = true;
                } else if self.debug {
                    eprintln!("  {}", clip(detail.as_deref().unwrap_or(""), 160));
                }
            }
            EventKind::Message { role, text, detail, .. } => match role.as_str() {
                "error" => self.line(&mut st, &format!("error: {text}")),
                "note" => self.line(&mut st, &format!("  {text}")),
                "tool" => {
                    if self.debug {
                        self.line(&mut st, &format!("  · {}", clip(text, 200)));
                        if text.ends_with("failed") {
                            if let Some(d) = detail {
                                self.line(&mut st, &format!("    {}", clip(d, 400)));
                            }
                        }
                    }
                }
                _ => {
                    if self.debug {
                        self.line(&mut st, &format!("  Claude: {}", clip(text, 600)));
                    }
                }
            },
            EventKind::TextDelta { .. } => {}
            EventKind::Question { id, questions, timeout_s } => {
                self.clear_bar(&mut st);
                match &self.questions {
                    Some(tx) => {
                        let _ =
                            tx.lock().unwrap().send(PendingQuestion { id: id.clone(), questions: questions.clone() });
                    }
                    None => {
                        for q in questions {
                            eprintln!("? {}", q.question);
                        }
                        let _ = timeout_s;
                    }
                }
            }
            EventKind::QuestionAnswered { by, .. } => {
                if by != "user" {
                    self.line(
                        &mut st,
                        "  (not asked: Claude proceeds on its stated assumption; the question is logged)",
                    );
                }
            }
            EventKind::Artifact { kind, path, .. } => {
                if kind == "output" || (self.debug && kind != "log") {
                    self.line(&mut st, &format!("  ✓ {kind}: {path}"));
                }
            }
            EventKind::Warning { message } => self.line(&mut st, &format!("warning: {message}")),
            EventKind::Cost { claude_usd, eleven_credits, .. } => {
                st.cost = *claude_usd;
                st.credits = *eleven_credits;
            }
            EventKind::RunFinished { status, cost_usd, eleven_credits, seconds, message, .. } => {
                self.clear_bar(&mut st);
                let cost = cost_usd.or(st.cost).map(|c| format!(", Claude ${c:.2}")).unwrap_or_default();
                let credits =
                    if *eleven_credits > 0 { format!(", {eleven_credits} ElevenLabs credits") } else { String::new() };
                eprintln!("■ {} in {:.0} s{cost}{credits}", status.label(), seconds);
                if let (Some(m), true) = (message, self.debug) {
                    eprintln!("  {m}");
                }
            }
        }
    }
}

/// Prompts for each question on the terminal and returns question text → answer.
pub fn ask_on_terminal(q: &PendingQuestion) -> BTreeMap<String, String> {
    let mut answers = BTreeMap::new();
    for question in &q.questions {
        eprintln!();
        eprintln!("Claude asks: {}", question.question);
        for (i, o) in question.options.iter().enumerate() {
            match &o.description {
                Some(d) => eprintln!("  {}. {} — {d}", i + 1, o.label),
                None => eprintln!("  {}. {}", i + 1, o.label),
            }
        }
        let hint = if question.options.is_empty() {
            "Your answer".to_string()
        } else if question.multi_select {
            "Numbers separated by commas, or your own answer".to_string()
        } else {
            "A number, or your own answer".to_string()
        };
        eprint!("{hint} (Enter for 1): ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        answers.insert(question.question.clone(), parse_answer(line.trim(), question));
    }
    answers
}

/// Turns typed input into the answer text: option numbers become labels, anything else is kept.
pub fn parse_answer(input: &str, q: &Question) -> String {
    if input.is_empty() {
        return q.options.first().map(|o| o.label.clone()).unwrap_or_default();
    }
    let picks: Vec<usize> = input.split(',').filter_map(|p| p.trim().parse::<usize>().ok()).collect();
    let all_numbers = input.split(',').all(|p| p.trim().parse::<usize>().is_ok());
    if all_numbers && !picks.is_empty() && picks.iter().all(|n| *n >= 1 && *n <= q.options.len()) {
        let take = if q.multi_select { picks.len() } else { 1 };
        return picks.iter().take(take).map(|n| q.options[*n - 1].label.clone()).collect::<Vec<_>>().join(", ");
    }
    input.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use video_studio_core::claude::stream::QuestionOption;

    fn q(multi: bool) -> Question {
        Question {
            question: "Tone?".into(),
            header: None,
            options: vec![
                QuestionOption { label: "Calm".into(), description: None },
                QuestionOption { label: "Bold".into(), description: None },
            ],
            multi_select: multi,
        }
    }

    #[test]
    fn answers_from_typed_input() {
        assert_eq!(parse_answer("", &q(false)), "Calm");
        assert_eq!(parse_answer("2", &q(false)), "Bold");
        assert_eq!(parse_answer("1, 2", &q(true)), "Calm, Bold");
        assert_eq!(parse_answer("1,2", &q(false)), "Calm");
        assert_eq!(parse_answer("7", &q(false)), "7");
        assert_eq!(parse_answer("Warm but direct", &q(false)), "Warm but direct");
        assert_eq!(bar(0.5), "[############------------]  50%");
        assert_eq!(clip("a\nb", 10), "a b");
    }
}
