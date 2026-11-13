//! An asset `script` gives captions and cues the script's exact words, timed by the transcript.

use cutline_render::pipeline::{self, LoadError};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

/// What the recogniser heard (with its slips), in source seconds.
const HEARD: &[(&str, f64, f64)] = &[
    ("Seven", 0.2, 0.6),
    ("pains,", 0.6, 1.1),
    ("each", 1.2, 1.4),
    ("one", 1.4, 1.6),
    ("keystroke", 1.6, 2.1),
    ("away.", 2.1, 2.6),
    ("Written", 3.0, 3.4),
    ("in", 3.4, 3.5),
    ("rust.", 3.5, 4.0),
];
const SCRIPT: &str = "[warm] Seven panes, each one keystroke away. [confident] Written in Rust.";

fn setup(d: &Path, script: &str) -> std::path::PathBuf {
    let tc = test_toolchain().unwrap();
    gen::still(tc, &d.join("bg.png"), 320, 180, "0x223344").unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "sine=f=220:d=5", "-ac", "2"]).arg(d.join("vo.wav"));
    gen::run(c).unwrap();
    let words: Vec<_> = HEARD.iter().map(|(w, s, e)| serde_json::json!({"w": w, "start": s, "end": e})).collect();
    std::fs::write(d.join("vo.json"), serde_json::json!({"asset": "vo", "words": words}).to_string()).unwrap();
    std::fs::write(d.join("script.txt"), script).unwrap();
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        "{version: 1, target: 'custom', width: 320, height: 180, fps: 30,
          assets: {bg: {path: 'bg.png'}, vo: {path: 'vo.wav', role: 'voice', transcript: 'vo.json', script: 'script.txt'}},
          cues: {rust: 'written in Rust'},
          main: [{id: 'a', clip: 'bg', end_at: 'cue:rust'}, {id: 'b', clip: 'bg', duration: 1.5}],
          layers: [{type: 'audio', asset: 'vo', start: 0.5}],
          captions: {style: 'clean_sentence'}}",
    )
    .unwrap();
    p
}

#[test]
fn captions_and_cues_use_the_script_words() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let p = setup(d.path(), SCRIPT);
    let l = pipeline::load(&p).unwrap_or_else(|e| panic!("{e}"));
    // the cue phrase is the script's wording and matches exactly (no fuzzy warning)
    assert!(l.cues[0].exact, "{:?}", l.cues);
    assert_eq!(l.cues[0].matched, "Written in Rust.");
    assert!((l.cues[0].at - (0.5 + 3.0)).abs() < 1e-9);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let srt = &pr.extras.srt;
    assert!(srt.contains("Seven panes, each one keystroke away."), "{srt}");
    assert!(srt.contains("Written in Rust."), "{srt}");
    assert!(!srt.contains("pains") && !srt.contains("warm"), "{srt}");
    // timing still comes from the audio: the first caption starts with "Seven" (0.2 s + 0.5 s)
    assert!(srt.contains("00:00:00,700 -->"), "{srt}");
    assert!(!pr.diags.iter().any(|x| x.code == "W_SCRIPT_MISMATCH"));
}

#[test]
fn wrong_script_and_missing_file_are_reported() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let p = setup(d.path(), "Meet Glance, a terminal dashboard you can leave open all day. Written in Rust.");
    let l = pipeline::load(&p).unwrap_or_else(|e| panic!("{e}"));
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    assert!(pr.diags.iter().any(|x| x.code == "W_SCRIPT_MISMATCH" && x.path == "/assets/vo/script"), "{:?}", pr.diags);
    std::fs::remove_file(d.path().join("script.txt")).unwrap();
    let diags = match pipeline::load(&p) {
        Ok(l) => l.diags,
        Err(LoadError::Invalid(d)) => d,
        Err(e) => panic!("{e}"),
    };
    assert!(diags.iter().any(|x| x.path == "/assets/vo/script"), "{diags:?}");
}
