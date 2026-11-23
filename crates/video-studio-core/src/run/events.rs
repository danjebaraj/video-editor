//! The run event stream. The same schema is written to `events.ndjson`, printed by
//! `video-studio run --json`, and delivered to the app (spec §13, §15).

use std::collections::BTreeMap;
use std::sync::Mutex;

use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::claude::stream::Question;
use crate::project::{RunKind, RunStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventKind {
    RunStarted {
        run_id: String,
        kind: RunKind,
        run_config: String,
        folder: String,
        model: String,
        outputs: Vec<String>,
    },
    /// A stage became current: `prepare`, `audio`, `authoring`, `validate`, `review`, `render`,
    /// `qa` or `done`.
    StageStarted {
        stage: String,
        label: String,
    },
    Progress {
        stage: String,
        /// 0–1 when cutline reports frames or segments.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fraction: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Something for the transcript: `assistant`, `tool`, `note` or `error`.
    Message {
        role: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// A tool call that came back as an error. The run goes on; Claude usually retries.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        failed: bool,
    },
    /// Streaming assistant text for the live transcript. Not written to `events.ndjson`.
    TextDelta {
        text: String,
    },
    /// Claude asked the user something; the run waits for an answer.
    Question {
        id: String,
        questions: Vec<Question>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_s: Option<u32>,
    },
    QuestionAnswered {
        id: String,
        /// `user`, `timeout` or `assumed` (headless with `--yes` or no terminal).
        by: String,
        #[serde(default)]
        answers: BTreeMap<String, String>,
    },
    Artifact {
        /// `output`, `audio`, `project`, `summary`, `review`, `qa`, `captions` or `log`.
        kind: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes: Option<u64>,
    },
    Warning {
        message: String,
    },
    /// Live cost (spec §12.3): Claude usage from the stream, ElevenLabs credits from the helper.
    Cost {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_usd: Option<f64>,
        /// True while the figure is computed from token counts rather than reported by the CLI.
        claude_estimated: bool,
        input_tokens: u64,
        output_tokens: u64,
        eleven_credits: u64,
        eleven_cap: u64,
    },
    StageFinished {
        stage: String,
        ok: bool,
        seconds: f64,
    },
    RunFinished {
        status: RunStatus,
        exit_code: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
        eleven_credits: u64,
        seconds: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub ts: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    pub fn now(kind: EventKind) -> Self {
        Event { ts: crate::timeutil::now_rfc3339(), kind }
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
}

/// Receives run events. Called from the run's thread.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &Event);
}

/// Appends events to `events.ndjson` and forwards them to another sink.
pub struct LoggingSink {
    file: Mutex<Option<std::fs::File>>,
    next: std::sync::Arc<dyn EventSink>,
}

impl LoggingSink {
    pub fn new(path: &Utf8Path, next: std::sync::Arc<dyn EventSink>) -> Self {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
        LoggingSink { file: Mutex::new(file), next }
    }
}

impl EventSink for LoggingSink {
    fn emit(&self, event: &Event) {
        if !matches!(event.kind, EventKind::TextDelta { .. }) {
            use std::io::Write;
            if let Some(f) = self.file.lock().unwrap().as_mut() {
                let _ = writeln!(f, "{}", event.to_json());
            }
        }
        self.next.emit(event);
    }
}

/// Collects events in memory (tests, and callers that only want the outcome).
#[derive(Default)]
pub struct MemorySink(pub Mutex<Vec<Event>>);

impl EventSink for MemorySink {
    fn emit(&self, event: &Event) {
        self.0.lock().unwrap().push(event.clone());
    }
}

impl MemorySink {
    pub fn events(&self) -> Vec<Event> {
        self.0.lock().unwrap().clone()
    }
}

/// The stage a tool call belongs to, and its label (spec §12.3).
pub fn stage_for_tool(tool: &str, input: &serde_json::Value) -> Option<(String, String)> {
    let s = |a: &str, b: &str| Some((a.to_string(), b.to_string()));
    match tool {
        "Write" | "Edit" | "MultiEdit" => {
            let path = input["file_path"].as_str().unwrap_or("");
            if path.ends_with("summary.md") || path.ends_with("readiness.json") {
                s("done", "Wrapping up")
            } else {
                s("authoring", "Authoring")
            }
        }
        "Bash" => {
            let segments = crate::claude::broker::split_commands(input["command"].as_str().unwrap_or("")).ok()?;
            let argv = segments.last()?;
            let name = Utf8Path::new(&argv[0]).file_stem().unwrap_or("");
            let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
            if name == "cutline" {
                let mut it = args.iter();
                let mut sub = None;
                while let Some(a) = it.next() {
                    if *a == "--variant" || *a == "--locale" {
                        it.next();
                    } else if !a.starts_with('-') {
                        sub = Some(*a);
                        break;
                    }
                }
                match sub? {
                    "validate" | "expand" | "cues" | "beats" => s("validate", "Validate"),
                    "review" | "preview" => s("review", "Review"),
                    "render" | "thumbnail" | "chapters" => {
                        let export = args.iter().position(|a| *a == "--export").and_then(|i| args.get(i + 1));
                        match export {
                            Some(e) => Some(("render".to_string(), format!("Render {e}"))),
                            None => s("render", "Render"),
                        }
                    }
                    "qa" => s("qa", "QA"),
                    // Audio Review: the soundtrack is mixed without a picture (FEAT-neat-osprey-DKK7)
                    "audio" => s("mix", "Mix"),
                    "transcribe" | "probe" | "record" | "init" => s("authoring", "Authoring"),
                    // schema, help, doctor, guide: looking things up moves no stage forward
                    // (BUG-rustic-olive-U4PY)
                    _ => None,
                }
            } else if args.first() == Some(&"capture") {
                // Driving and recording an app is its own stage; the permission check is not.
                if args.contains(&"check") {
                    None
                } else {
                    s("capture", "Recording")
                }
            } else if args.first() == Some(&"eleven") {
                match args.get(1).copied() {
                    Some("tts" | "music" | "sfx" | "scribe") => s("audio", "Audio"),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// A fraction from a cutline progress line: `segment 3/12`, `frame 120/900`, or `45%`.
/// cutline commands that only look things up (BUG-rustic-olive-U4PY): their success says nothing
/// about the project, so they neither start a stage nor complete one.
pub fn is_informational(sub: &str) -> bool {
    matches!(sub, "schema" | "help" | "doctor" | "guide" | "templates" | "version" | "--help" | "-h" | "--version")
}

pub fn progress_fraction(line: &str) -> Option<f64> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let a: f64 = line[start..i].parse().ok()?;
            if i < bytes.len() && bytes[i] == b'%' {
                return (a <= 100.0).then_some(a / 100.0);
            }
            if i < bytes.len() && bytes[i] == b'/' && bytes.get(i + 1).map(|b| b.is_ascii_digit()).unwrap_or(false) {
                let s2 = i + 1;
                let mut j = s2;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                let b: f64 = line[s2..j].parse().ok()?;
                // dates and paths have more separators after the second number
                if b > 0.0 && a <= b && !(j < bytes.len() && (bytes[j] == b'/' || bytes[j] == b'-')) {
                    return Some(a / b);
                }
                i = j;
            }
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_schema_is_stable() {
        let e = Event {
            ts: "2026-11-14T12:00:00Z".into(),
            kind: EventKind::StageStarted { stage: "render".into(), label: "Render shorts".into() },
        };
        assert_eq!(
            e.to_json(),
            r#"{"ts":"2026-11-14T12:00:00Z","event":"stage_started","stage":"render","label":"Render shorts"}"#
        );
        let p = Event {
            ts: "t".into(),
            kind: EventKind::Progress { stage: "render".into(), fraction: Some(0.25), detail: None },
        };
        assert_eq!(p.to_json(), r#"{"ts":"t","event":"progress","stage":"render","fraction":0.25}"#);
        let f = Event {
            ts: "t".into(),
            kind: EventKind::RunFinished {
                status: RunStatus::SucceededWithWarnings,
                exit_code: 1,
                cost_usd: Some(1.5),
                eleven_credits: 420,
                seconds: 61.0,
                message: None,
            },
        };
        let v: serde_json::Value = serde_json::from_str(&f.to_json()).unwrap();
        assert_eq!(
            v,
            json!({"ts": "t", "event": "run_finished", "status": "succeeded_with_warnings", "exit_code": 1, "cost_usd": 1.5, "eleven_credits": 420, "seconds": 61.0})
        );
        // every event name the spec lists round-trips
        for name in [
            "stage_started",
            "progress",
            "message",
            "question",
            "artifact",
            "warning",
            "stage_finished",
            "run_finished",
        ] {
            let sample = match name {
                "stage_started" => json!({"stage": "audio", "label": "Audio"}),
                "progress" => json!({"stage": "render"}),
                "message" => json!({"role": "assistant", "text": "hi"}),
                "question" => {
                    json!({"id": "q1", "questions": [{"question": "Tone?", "options": [], "multi_select": false}]})
                }
                "artifact" => json!({"kind": "output", "path": "/x.mp4"}),
                "warning" => json!({"message": "careful"}),
                "stage_finished" => json!({"stage": "audio", "ok": true, "seconds": 2.0}),
                _ => json!({"status": "failed", "exit_code": 3, "eleven_credits": 0, "seconds": 1.0}),
            };
            let mut obj = sample.as_object().unwrap().clone();
            obj.insert("ts".into(), json!("t"));
            obj.insert("event".into(), json!(name));
            let parsed: Event = serde_json::from_value(serde_json::Value::Object(obj.clone()))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(serde_json::from_str::<serde_json::Value>(&parsed.to_json()).unwrap()["event"], name);
        }
    }

    #[test]
    fn stages_from_tool_calls() {
        let bash = |c: &str| stage_for_tool("Bash", &json!({"command": c})).map(|s| s.1);
        assert_eq!(bash("cutline validate --json project.json5").as_deref(), Some("Validate"));
        assert_eq!(bash("cutline --json review project.json5").as_deref(), Some("Review"));
        assert_eq!(bash("cutline render project.json5 --export shorts").as_deref(), Some("Render shorts"));
        assert_eq!(bash("cutline --variant teaser render project.json5").as_deref(), Some("Render"));
        assert_eq!(bash("cutline qa outputs/a/a.mp4").as_deref(), Some("QA"));
        assert_eq!(
            bash("cutline audio project.json5 --export full --out outputs/audio-review.wav").as_deref(),
            Some("Mix")
        );
        assert_eq!(bash("video-studio eleven tts --voice v --text-file t --out a.mp3").as_deref(), Some("Audio"));
        assert_eq!(bash("video-studio eleven quote tts --text-file t"), None);
        assert_eq!(bash("/abs/path/cutline validate p.json5 && cutline review p.json5").as_deref(), Some("Review"));
        assert_eq!(bash("ls | wc"), None);
        assert_eq!(stage_for_tool("Write", &json!({"file_path": "project.json5"})).unwrap().0, "authoring");
        let bash = |c: &str| stage_for_tool("Bash", &json!({"command": c}));
        assert_eq!(
            bash("video-studio capture click --x 10 --y 20").unwrap(),
            ("capture".to_string(), "Recording".to_string())
        );
        assert_eq!(bash("video-studio capture record start --out captures/a.mov").unwrap().0, "capture");
        assert_eq!(bash("video-studio capture check"), None);
        assert_eq!(stage_for_tool("Write", &json!({"file_path": "/run/summary.md"})).unwrap().0, "done");
        assert_eq!(stage_for_tool("Read", &json!({"file_path": "spec.md"})), None);
    }

    #[test]
    fn progress_lines() {
        assert_eq!(progress_fraction("render: segment 3/12"), Some(0.25));
        assert_eq!(progress_fraction("frame=  450/900 fps=30"), Some(0.5));
        assert_eq!(progress_fraction("encoding 45% done"), Some(0.45));
        assert_eq!(progress_fraction("wrote out/2026/11/14/file.mp4"), None);
        assert_eq!(progress_fraction("no numbers here"), None);
        assert_eq!(progress_fraction("12/3 is not progress"), None);
    }

    #[test]
    fn logging_sink_skips_deltas() {
        let d = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(d.path()).unwrap().join("logs/events.ndjson");
        let mem = std::sync::Arc::new(MemorySink::default());
        let sink = LoggingSink::new(&path, mem.clone());
        sink.emit(&Event::now(EventKind::TextDelta { text: "par".into() }));
        sink.emit(&Event::now(EventKind::Warning { message: "w".into() }));
        assert_eq!(mem.events().len(), 2);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("\"event\":\"warning\""));
    }
}
