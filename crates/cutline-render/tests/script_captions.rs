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

/// The voice said something else than the script in one place: captions cannot show it (they
/// use the script), so the script check points at the time.
#[test]
fn a_misspoken_word_is_reported_with_its_time() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let p = setup(d.path(), "[warm] Seven Todos, each one keystroke away. [confident] Written in Rust.");
    // "pains" (HEARD) for "Todos": not close in letters or sound
    let l = pipeline::load(&p).unwrap_or_else(|e| panic!("{e}"));
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    assert!(pr.extras.srt.contains("Seven Todos,"), "captions show the script: {}", pr.extras.srt);
    let w: Vec<_> = pr.diags.iter().filter(|x| x.code == "W_SCRIPT_MISMATCH").collect();
    assert_eq!(w.len(), 1, "{:?}", pr.diags);
    assert_eq!(w[0].path, "/assets/vo/script");
    // the voice layer starts at 0.5 s; "pains" is at 0.6–1.1 s of the asset
    assert!(w[0].message.contains("\"Todos,\"") && w[0].message.contains("\"pains,\""), "{}", w[0].message);
    assert!(w[0].message.contains("00:01.100") && w[0].message.contains("0.60–1.10s"), "{}", w[0].message);
    let m = &pr.extras.script_mismatches;
    assert_eq!(m.len(), 1);
    assert_eq!((m[0].asset.as_str(), m[0].at, m[0].source_start), ("vo", Some(1.1), 0.6));
    // `validate` reports it too, without preparing a render
    assert!(cutline_render::extras::warnings(&l).iter().any(|x| x.code == "W_SCRIPT_MISMATCH"));

    // the brand dictionary / caption corrections say what a heard spelling means
    let text = std::fs::read_to_string(&p).unwrap();
    let fixed = d.path().join("fixed.json5");
    std::fs::write(&fixed, text.replace("captions: {", "captions: {corrections: {pains: 'Todos'}, ")).unwrap();
    let l = pipeline::load(&fixed).unwrap_or_else(|e| panic!("{e}"));
    assert!(cutline_render::extras::warnings(&l).is_empty());
    // … or the word is ignored, or the check is off
    for audio in ["audio: {script_check: {ignore: ['todos']}}", "audio: {script_check: {enabled: false}}"] {
        std::fs::write(&fixed, text.replace("captions: {", &format!("{audio}, captions: {{"))).unwrap();
        let l = pipeline::load(&fixed).unwrap_or_else(|e| panic!("{e}"));
        assert!(cutline_render::extras::warnings(&l).is_empty(), "{audio}");
    }
}

/// From a run's log: a voice-over generated one passage at a time, one take of which came back
/// at half the pace of the others. A passage far from the rest's pace is a warning.
#[test]
fn a_passage_spoken_much_slower_than_the_rest_is_a_warning() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("bg.png"), 320, 180, "0x223344").unwrap();
    let mut assets = String::new();
    // five words of five letters each; `per_word` seconds a word
    for (name, per_word) in [("a", 0.4), ("b", 0.42), ("c", 0.38), ("slow", 0.8)] {
        let mut c = tc.ffmpeg_cmd();
        c.args(["-f", "lavfi", "-i", "sine=f=220:d=5", "-ac", "2"]).arg(d.path().join(format!("{name}.wav")));
        gen::run(c).unwrap();
        let words: Vec<_> = (0..5)
            .map(|k| serde_json::json!({"w": "words", "start": k as f64 * per_word, "end": (k + 1) as f64 * per_word}))
            .collect();
        std::fs::write(
            d.path().join(format!("{name}.json")),
            serde_json::json!({"asset": name, "words": words}).to_string(),
        )
        .unwrap();
        assets.push_str(&format!(", {name}: {{path: '{name}.wav', role: 'voice', transcript: '{name}.json'}}"));
    }
    let p = d.path().join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, target: 'custom', width: 320, height: 180, fps: 30,
              assets: {{bg: {{path: 'bg.png'}}{assets}}},
              main: [{{id: 'm', clip: 'bg', duration: 20}}],
              layers: [{{type: 'audio', asset: 'a', start: 0}}, {{type: 'audio', asset: 'b', start: 5}},
                       {{type: 'audio', asset: 'c', start: 10}}, {{type: 'audio', asset: 'slow', start: 15}}]}}"
        ),
    )
    .unwrap();
    let l = pipeline::load(&p).unwrap_or_else(|e| panic!("{e}"));
    let pace: Vec<_> = cutline_render::extras::warnings(&l).into_iter().filter(|x| x.code == "W_VOICE_PACE").collect();
    assert_eq!(pace.len(), 1, "{pace:?}");
    assert_eq!(pace[0].path, "/assets/slow");
    assert!(pace[0].message.contains("slower than the other voice passages (75 words a minute)"), "{pace:?}");
}
