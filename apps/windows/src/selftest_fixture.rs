//! The self test's scratch world (ui/spec.md §23.9 item 3): a library of its own, the fake Claude
//! CLI, the real cutline, `cutline selftest --quick` footage, and the scripts the fake Claude
//! plays for the assistant turn, the run and the stopped run.

use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde_json::json;

use video_studio_core::paths;

#[derive(Debug, Clone, Serialize)]
pub struct Fixture {
    /// Scratch folder: library, footage, project, scripts, snapshots and the log.
    pub out: Utf8PathBuf,
    pub library: Utf8PathBuf,
    pub project: Utf8PathBuf,
    pub footage: Utf8PathBuf,
    /// A folder the assistant may read (`assistant_folders`).
    pub shared: Utf8PathBuf,
    pub output_folder: Utf8PathBuf,
    pub shots: Utf8PathBuf,
    pub trace: Utf8PathBuf,
    pub assistant_script: Utf8PathBuf,
    pub run_script: Utf8PathBuf,
    pub hang_script: Utf8PathBuf,
    /// The spec the edit step types.
    pub spec: String,
    pub claude: String,
    pub cutline: String,
}

/// The spec the self test edits: three media references to resolve, a Mermaid diagram, a
/// voice-over block and a table.
pub const SPEC: &str = "# Launch teaser

## Scene 1: Hook
- **Duration:** 3 s
- **Media:** ![hero](media:m-a)

```vo
Meet the fastest way to ship.
```

## Scene 2: Product
- **Duration:** 3 s
- **Media:** ![b-roll](media:m-b)

```mermaid
graph LR
  Spec --> Claude --> cutline
```

| Shot | Length |
|---|---|
| Hero | 3 s |

## Call to action
Start a free trial. ![logo](media:m-logo) ![later](media:m-not-yet)
";

fn exe_dir() -> Utf8PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("."))
}

fn find_cutline() -> Option<Utf8PathBuf> {
    if let Some(c) = std::env::var("VIDEO_STUDIO_CUTLINE").ok().filter(|s| !s.is_empty()) {
        return Some(c.into());
    }
    let next = exe_dir().join("cutline.exe");
    if next.is_file() {
        return Some(next);
    }
    video_studio_core::claude::locate::find_cutline("", "").map(|c| c.path.into())
}

pub fn prepare() -> Result<Fixture, String> {
    let out = match std::env::var("VS_SELFTEST").ok().filter(|s| !s.is_empty()) {
        Some(d) => Utf8PathBuf::from(d),
        None => {
            let base = std::env::temp_dir().join(format!("vs-selftest-{}", std::process::id()));
            Utf8PathBuf::from_path_buf(base).map_err(|_| "temp folder isn't UTF-8")?
        }
    };
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let out = paths::canonical_or_normal(&out);
    let library = out.join("library");
    let footage = out.join("footage");
    let shared = out.join("shared");
    for d in [&library, &footage, &shared, &out.join("shots")] {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(shared.join("brand-notes.md"), "Our voice: calm, precise.\n").map_err(|e| e.to_string())?;
    let claude = exe_dir().join("vs-fake-claude.exe");
    if !claude.is_file() {
        return Err(format!("{claude} is missing: build the workspace first (build.ps1 selftest does)"));
    }
    let cutline = find_cutline().ok_or("cutline wasn't found: build it or set VIDEO_STUDIO_CUTLINE")?;

    // The app and everything it starts use the scratch library, the fake Claude and a test key.
    std::env::set_var("VIDEO_STUDIO_HOME", library.as_str());
    std::env::set_var("VIDEO_STUDIO_CLAUDE", claude.as_str());
    std::env::set_var("VIDEO_STUDIO_CUTLINE", cutline.as_str());
    let secrets = out.join("secrets.json");
    std::fs::write(&secrets, json!({"elevenlabs-api-key": "selftest-key"}).to_string()).map_err(|e| e.to_string())?;
    std::env::set_var("VIDEO_STUDIO_TEST_SECRETS", secrets.as_str());
    let trace = out.join("trace.ndjson");
    std::env::set_var("FAKE_CLAUDE_TRACE", trace.as_str());

    // Footage from cutline's own quick self test.
    let assets = out.join("selftest-assets");
    let status = Command::new(cutline.as_str())
        .args(["selftest", "--quick", "--keep", "--out", assets.as_str()])
        .output()
        .map_err(|e| format!("cutline selftest: {e}"))?;
    let src = assets.join("pipeline_shorts_render");
    if !src.join("a.mp4").is_file() {
        return Err(format!(
            "cutline selftest --quick produced no footage (exit {:?}): {}",
            status.status.code(),
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    for f in ["a.mp4", "b.mp4", "c.mp4", "logo.png", "music.wav"] {
        std::fs::copy(src.join(f), footage.join(f)).map_err(|e| format!("{f}: {e}"))?;
    }

    // A new project, saved as the New Project sheet saves it.
    let project = out.join("projects").join("Launch teaser.cutproj");
    std::fs::create_dir_all(project.parent().unwrap()).map_err(|e| e.to_string())?;
    let p = video_studio_ffi::call(
        "project.new".into(),
        json!({"name": "Launch teaser", "intent": "marketing"}).to_string(),
    )
    .map_err(|e| e.to_string())?;
    let p: serde_json::Value = serde_json::from_str(&p).map_err(|e| e.to_string())?;
    video_studio_ffi::call("project.save".into(), json!({"project": p, "path": project}).to_string())
        .map_err(|e| e.to_string())?;

    let output_folder = out.join("out");
    let fx = Fixture {
        library,
        project,
        footage: footage.clone(),
        shared: shared.clone(),
        output_folder: output_folder.clone(),
        shots: out.join("shots"),
        trace,
        assistant_script: out.join("assistant-script.json"),
        run_script: out.join("run-script.json"),
        hang_script: out.join("hang-script.json"),
        spec: SPEC.into(),
        claude: claude.to_string(),
        cutline: cutline.to_string(),
        out,
    };
    write_scripts(&fx)?;
    Ok(fx)
}

fn write_scripts(fx: &Fixture) -> Result<(), String> {
    let write = |p: &Utf8Path, v: serde_json::Value| std::fs::write(p, v.to_string()).map_err(|e| e.to_string());
    // The assistant asks one question, edits the spec twice, and tries two things it may not.
    write(
        &fx.assistant_script,
        json!([
            {"say": "Let me check the audience first."},
            {"tool": "AskUserQuestion", "input": {"questions": [{"question": "Who is the audience?", "header": "Audience", "multiSelect": false, "options": [{"label": "Developers"}, {"label": "Managers"}]}]}},
            {"tool": "Read", "input": {"file_path": fx.shared.join("brand-notes.md").to_string()}},
            {"tool": "Edit", "input": {"file_path": "spec.md", "old_string": "Meet the fastest way to ship.", "new_string": "Meet the fastest way for developers to ship."}},
            {"tool": "Edit", "input": {"file_path": "spec.md", "old_string": "Start a free trial.", "new_string": "Start a free trial today."}},
            {"tool": "Write", "input": {"file_path": fx.project.to_string(), "content": "{}"}},
            {"tool": "Bash", "input": {"command": "rm -rf ~"}},
            {"result": {"text": "I tightened the hook and the call to action.", "cost": 0.02}}
        ]),
    )?;
    // The production run: writes the edit, validates, asks, renders, summarizes.
    // Where the relink step leaves the footage (real Claude reads it from media-manifest.json).
    let moved = fx.out.join("Volumes").join("Work").join("footage");
    let a = moved.join("a.mp4").as_str().replace('\\', "/");
    let b = moved.join("b.mp4").as_str().replace('\\', "/");
    let project_json5 = format!(
        "{{\n  version: 1, name: \"launch\", target: \"shorts\",\n  assets: {{ a: {{path: \"{a}\"}}, b: {{path: \"{b}\"}} }},\n  main: [\n    {{ id: \"hook\", clip: \"a\", in: 1.0, out: 3.0 }},\n    {{ transition: {{ type: \"fade\", duration: 0.3 }} }},\n    {{ id: \"product\", clip: \"b\", in: 2.0, out: 4.0 }},\n  ],\n  exports: [ {{name: \"youtube-shorts\", target: \"shorts\", out: \"outputs/youtube-shorts/youtube-shorts.mp4\"}} ],\n}}\n"
    );
    write(
        &fx.run_script,
        json!([
            {"say": "Reading the spec and the manifest."},
            {"tool": "Read", "input": {"file_path": "spec.md"}},
            {"tool": "Write", "input": {"file_path": "project.json5", "content": project_json5}},
            {"tool": "Bash", "input": {"command": "ls"}},
            {"tool": "Bash", "input": {"command": "cutline validate --json project.json5"}},
            {"tool": "AskUserQuestion", "input": {"questions": [{"question": "Render the draft now?", "header": "Render", "multiSelect": false, "options": [{"label": "Yes"}, {"label": "No"}]}]}},
            {"tool": "Bash", "input": {"command": "cutline render --draft project.json5"}},
            {"tool": "Write", "input": {"file_path": "summary.md", "content": "## What I made\nA draft Shorts cut.\n\n## Assumptions\nNone.\n"}},
            {"result": {"text": "The draft is rendered.", "cost": 0.42}}
        ]),
    )?;
    write(&fx.hang_script, json!([{"say": "Working on it."}, {"hang": true}]))?;
    Ok(())
}
