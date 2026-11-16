//! Shared fixtures for the end-to-end tests: a temp world with a project, the fake Claude and
//! cutline binaries, and a fake ElevenLabs server.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};

use video_studio_core::media::{self, MediaRole};
use video_studio_core::platform::NoFileRefs;
use video_studio_core::presets::{OutputConfig, OutputOverrides};
use video_studio_core::project::{IntentKind, Project, RunConfig};

pub const VIDEO_STUDIO: &str = env!("CARGO_BIN_EXE_video-studio");
pub const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_vs-fake-claude");
pub const FAKE_CUTLINE: &str = env!("CARGO_BIN_EXE_vs-fake-cutline");

pub const SPEC: &str = "# Launch teaser\n\n## Scene 1: Hook\n- **Duration:** 20 s\n- **Media:** ![hero](media:m-hero)\n\n```vo\nMeet the fastest way to ship.\n```\n\n## Scene 2: Close\n- **Duration:** 25 s\n- **On-screen text:** Try it today\n\n## Call to action\nStart a free trial.\n";

/// A project.json5 whose exports match the fixture's two outputs.
pub const PROJECT_JSON5: &str = r#"{
  name: "launch",
  target: "shorts",
  exports: [
    {name: "youtube-shorts", target: "shorts", out: "outputs/youtube-shorts/youtube-shorts.mp4"},
    {name: "linkedin", target: "portrait_4x5", out: "outputs/linkedin/linkedin.mp4"},
  ],
}
"#;

pub struct World {
    pub dir: tempfile::TempDir,
    pub root: Utf8PathBuf,
    pub lib: Utf8PathBuf,
    pub project: Utf8PathBuf,
    pub out: Utf8PathBuf,
    pub trace: Utf8PathBuf,
    pub script: Utf8PathBuf,
    pub secrets: Utf8PathBuf,
    pub eleven_url: Option<String>,
}

impl World {
    pub fn new() -> World {
        let dir = tempfile::tempdir().unwrap();
        // canonical, without the verbatim `\\?\` prefix Windows' canonicalize adds
        let root = video_studio_core::paths::canonical_or_normal(Utf8Path::from_path(dir.path()).unwrap());
        let lib = root.join("library");
        let out = root.join("out");
        std::fs::create_dir_all(root.join("footage")).unwrap();
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(root.join("footage/hero.mov"), b"fake video bytes for the hero shot").unwrap();
        let project = root.join("proj/launch.cutproj");
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        let mut p = Project::new("Launch teaser");
        p.spec = SPEC.into();
        p.intent.kind = IntentKind::Marketing;
        p.intent.target_length_s = Some(45.0);
        let id = media::add_media(&mut p, Some(&project), &root.join("footage/hero.mov"), &NoFileRefs).unwrap();
        assert_eq!(id, "m-hero");
        p.media[0].role = Some(MediaRole::Footage);
        p.output_configs = vec![
            OutputConfig {
                id: "out_shorts".into(),
                preset: "youtube_shorts".into(),
                name: None,
                overrides: OutputOverrides::default(),
            },
            OutputConfig {
                id: "out_linkedin".into(),
                preset: "linkedin".into(),
                name: None,
                overrides: OutputOverrides::default(),
            },
        ];
        let mut rc = RunConfig::new("Social pack");
        rc.id = "rc_social".into();
        rc.outputs = vec!["out_shorts".into(), "out_linkedin".into()];
        rc.output_folder = out.to_string();
        rc.dry_run_first = false;
        rc.eleven_credit_cap = 500;
        p.run_configs.push(rc);
        p.save(&project).unwrap();
        let w = World {
            trace: root.join("trace.ndjson"),
            script: root.join("script.json"),
            secrets: root.join("secrets.json"),
            dir,
            root,
            lib,
            project,
            out,
            eleven_url: Some("http://127.0.0.1:9".into()),
        };
        std::fs::write(&w.secrets, json!({"elevenlabs-api-key": "test-key"}).to_string()).unwrap();
        w
    }

    pub fn edit_project(&self, f: impl FnOnce(&mut Project)) {
        let mut p = Project::load(&self.project).unwrap();
        f(&mut p);
        p.save(&self.project).unwrap();
    }

    pub fn load_project(&self) -> Project {
        Project::load(&self.project).unwrap()
    }

    pub fn set_script(&self, steps: Value) {
        std::fs::write(&self.script, steps.to_string()).unwrap();
    }

    /// Environment shared by the CLI and anything it starts.
    pub fn env(&self) -> Vec<(String, String)> {
        let mut v = vec![
            ("VIDEO_STUDIO_HOME".to_string(), self.lib.to_string()),
            ("VIDEO_STUDIO_CLAUDE".to_string(), FAKE_CLAUDE.to_string()),
            ("VIDEO_STUDIO_CUTLINE".to_string(), FAKE_CUTLINE.to_string()),
            ("VIDEO_STUDIO_TEST_SECRETS".to_string(), self.secrets.to_string()),
            ("FAKE_CLAUDE_SCRIPT".to_string(), self.script.to_string()),
            ("FAKE_CLAUDE_TRACE".to_string(), self.trace.to_string()),
            ("HOME".to_string(), self.root.to_string()),
        ];
        if let Some(u) = &self.eleven_url {
            v.push(("VIDEO_STUDIO_ELEVEN_BASE_URL".to_string(), u.clone()));
        }
        v
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(VIDEO_STUDIO);
        c.args(args)
            .envs(self.env())
            .env_remove("VIDEO_STUDIO_NO_CLAUDE")
            .env_remove("VIDEO_STUDIO_RUN_CONTEXT")
            .current_dir(&self.root);
        c
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    pub fn trace(&self) -> Vec<Value> {
        ndjson(&self.trace)
    }

    /// Tool calls from the trace as `(tool, decision, via, what)`.
    pub fn tool_calls(&self) -> Vec<(String, String, String, String)> {
        self.trace()
            .iter()
            .filter(|t| t["tool"].is_string())
            .map(|t| {
                let what =
                    t["input"]["command"].as_str().or(t["input"]["file_path"].as_str()).unwrap_or("").to_string();
                (
                    t["tool"].as_str().unwrap().to_string(),
                    t["decision"].as_str().unwrap_or("").to_string(),
                    t["via"].as_str().unwrap_or("").to_string(),
                    what,
                )
            })
            .collect()
    }

    /// The single run folder under `out/` (or the newest when there are several).
    pub fn run_folder(&self) -> Utf8PathBuf {
        let mut dirs: Vec<Utf8PathBuf> = std::fs::read_dir(&self.out)
            .unwrap()
            .flatten()
            .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs.pop().expect("a run folder")
    }
}

pub fn ndjson(path: &Utf8Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

pub fn events(o: &Output) -> Vec<Value> {
    stdout(o).lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["event"].is_string()).collect()
}

pub fn names(events: &[Value]) -> Vec<String> {
    events.iter().map(|e| e["event"].as_str().unwrap_or("").to_string()).collect()
}

/// The script of a complete, well-behaved production run.
pub fn happy_script() -> Value {
    json!([
        {"say": "Reading the spec."},
        {"tool": "Read", "input": {"file_path": "spec.md"}},
        {"tool": "Write", "input": {"file_path": "project.json5", "content": PROJECT_JSON5}},
        {"tool": "Bash", "input": {"command": "cutline validate --json project.json5"}},
        {"tool": "Bash", "input": {"command": "video-studio eleven tts --voice v1 --text 'Hello there' --out vo.mp3"}},
        {"tool": "Bash", "input": {"command": "cutline review project.json5"}},
        {"tool": "Bash", "input": {"command": "cutline render project.json5"}},
        {"tool": "Write", "input": {"file_path": "summary.md", "content": "## What I made\nTwo outputs.\n\n## Assumptions\nNone.\n"}},
        {"result": {"text": "Both outputs are rendered.", "cost": 0.5}}
    ])
}

// MARK: Fake ElevenLabs server

pub struct FakeEleven {
    pub url: String,
    pub requests: Arc<Mutex<Vec<String>>>,
}

impl FakeEleven {
    pub fn count(&self, needle: &str) -> usize {
        self.requests.lock().unwrap().iter().filter(|r| r.contains(needle)).count()
    }
}

/// Starts a tiny HTTP server that answers the ElevenLabs endpoints the helper uses. Requests
/// without `xi-api-key: test-key` get a 401.
pub fn fake_eleven() -> FakeEleven {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let log = log.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    return;
                }
                let mut length = 0usize;
                let mut key = String::new();
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                    let lower = h.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("xi-api-key:") {
                        key = h[11..].trim().to_string();
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let mut parts = request_line.split_whitespace();
                let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                log.lock().unwrap().push(format!("{method} {path}"));
                let body_json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let respond = |stream: &mut std::net::TcpStream,
                               status: &str,
                               content_type: &str,
                               extra: &str,
                               bytes: &[u8]| {
                    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nrequest-id: req-test\r\n{extra}Connection: close\r\n\r\n", bytes.len());
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(bytes);
                    let _ = stream.flush();
                };
                if key != "test-key" {
                    respond(
                        &mut stream,
                        "401 Unauthorized",
                        "application/json",
                        "",
                        br#"{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}"#,
                    );
                    return;
                }
                if path.starts_with("/v1/user/subscription") {
                    respond(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        "",
                        br#"{"tier":"creator","character_count":1000,"character_limit":100000}"#,
                    );
                } else if path.starts_with("/v2/voices") {
                    respond(&mut stream, "200 OK", "application/json", "", br#"{"voices":[{"voice_id":"v1","name":"Aria","category":"premade","description":"Warm narrator","labels":{"gender":"female","accent":"american"}},{"voice_id":"v2","name":"Bram","category":"premade","labels":{"gender":"male"},"created_at_unix":1760000000}]}"#);
                } else if path.starts_with("/v1/text-to-speech/") {
                    let text = body_json["text"].as_str().unwrap_or("");
                    let mut audio = format!("FAKE-MP3 voice text: {text}");
                    if let Some(seed) = body_json["seed"].as_u64() {
                        audio.push_str(&format!(" [take {seed}]"));
                    }
                    for (key, label) in [("previous_text", "previous"), ("next_text", "next")] {
                        if let Some(t) = body_json[key].as_str() {
                            audio.push_str(&format!(" [{label}: {t}]"));
                        }
                    }
                    respond(
                        &mut stream,
                        "200 OK",
                        "audio/mpeg",
                        &format!("character-cost: {}\r\n", text.chars().count()),
                        audio.as_bytes(),
                    );
                } else if path.starts_with("/v1/music") {
                    respond(
                        &mut stream,
                        "200 OK",
                        "audio/mpeg",
                        "",
                        format!("FAKE-MP3 music: {}", body_json["prompt"].as_str().unwrap_or("")).as_bytes(),
                    );
                } else if path.starts_with("/v1/sound-generation") {
                    respond(
                        &mut stream,
                        "200 OK",
                        "audio/mpeg",
                        "",
                        format!("FAKE-MP3 sfx: {}", body_json["text"].as_str().unwrap_or("")).as_bytes(),
                    );
                } else if path.starts_with("/v1/speech-to-text") {
                    respond(&mut stream, "200 OK", "application/json", "", br#"{"language_code":"en","text":"Hello there","words":[{"text":"Hello","start":0.0,"end":0.4,"type":"word"},{"text":"there","start":0.5,"end":0.9,"type":"word"}]}"#);
                } else {
                    respond(&mut stream, "404 Not Found", "application/json", "", br#"{"detail":"not found"}"#);
                }
            });
        }
    });
    FakeEleven { url, requests }
}
