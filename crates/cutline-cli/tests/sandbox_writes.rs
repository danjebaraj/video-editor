//! In Codex's sandbox a cutline run may write only its own folder: the system temp folder and
//! cutline's shared cache are read-only ("Operation not permitted"). These runs reproduce that:
//! `TMPDIR` and the home's `cache` are regular files, so nothing can be created under them, while
//! the home's `tools` still points at the real tools.

mod common;
use common::*;
use std::path::Path;

/// A cutline home whose `cache` can't be written, and a `TMPDIR` that can't either.
fn sandboxed(cmd: &mut std::process::Command, root: &Path) {
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("cache"), b"").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(cutline_deps::paths::tools_dir(), home.join("tools")).unwrap();
    let tmp = root.join("tmp-is-a-file");
    std::fs::write(&tmp, b"").unwrap();
    cmd.env("CUTLINE_HOME", &home).env("TMPDIR", &tmp).env("TMP", &tmp).env("TEMP", &tmp);
}

/// Files under `dir` (recursively).
fn files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(files(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn beats_and_transcripts_are_cached_in_the_project_when_nothing_else_is_writable() {
    let Some(tc) = cutline_testkit::test_toolchain() else { return };
    if cfg!(not(unix)) {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let proj = d.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    // 120 BPM kick
    let kick = "gt(t,0.25)*(0.8*sin(2*PI*60*t)*exp(-mod(t-0.25,0.5)*30)+0.5*(random(0)-0.5)*exp(-mod(t-0.25,0.5)*400))";
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &format!("aevalsrc='{kick}':s=48000:d=8"), "-ac", "2"]).arg(proj.join("music.wav"));
    cutline_testkit::gen::run(c).unwrap();
    cutline_testkit::gen::still(tc, &proj.join("bg.png"), 320, 180, "0x223344").unwrap();
    let pp = proj.join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, target: 'custom', width: 320, height: 180, fps: 30,
          assets: {bg: {path: 'bg.png'}, music: {path: 'music.wav', role: 'music'}},
          main: [{id: 'a', clip: 'bg', duration: 4}],
          layers: [{type: 'audio', asset: 'music'}]}",
    )
    .unwrap();

    // beats: worked before too, but warned and lost the result on every run
    let mut cmd = bin();
    cmd.arg("beats").arg(&pp);
    sandboxed(&mut cmd, &d.path().join("sb1"));
    let (code, v, err) = run_json(&mut cmd);
    assert_eq!(code, 0, "{err} {v}");
    assert!(!err.contains("couldn't save to the cache"), "{err}");
    let cached = files(&proj.join(".cutline/cache/beats"));
    assert_eq!(cached.len(), 1, "{cached:?}");
    // a second run reads it back from the project
    let mut cmd = bin();
    cmd.arg("beats").arg(&pp);
    sandboxed(&mut cmd, &d.path().join("sb2"));
    let (code, v2, err) = run_json(&mut cmd);
    assert_eq!(code, 0, "{err}");
    assert!(v["beats"]["bpm"].is_number(), "{v}");
    assert_eq!(v["beats"], v2["beats"]);

    // transcribe (a Scribe import, so no whisper is needed): kept in the working directory's
    // `.cutline/cache`, the only folder a sandboxed run may write
    let scribe = proj.join("scribe.json");
    std::fs::write(&scribe, r#"{"words": [{"text": "Hi", "start": 0.1, "end": 0.3}]}"#).unwrap();
    let mut cmd = bin();
    cmd.current_dir(&proj).arg("transcribe").arg(proj.join("music.wav")).arg("--import-scribe").arg(&scribe);
    sandboxed(&mut cmd, &d.path().join("sb3"));
    let (code, v, err) = run_json(&mut cmd);
    assert_eq!(code, 0, "{err} {v}");
    assert!(!err.contains("couldn't save to the cache"), "{err}");
    assert_eq!(files(&proj.join(".cutline/cache/transcripts")).len(), 1);
    // nothing left in the project's work folder
    assert!(files(&proj.join(".cutline/tmp")).is_empty());
}
