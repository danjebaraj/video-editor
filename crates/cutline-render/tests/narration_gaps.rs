//! Asset `gaps`: silence inserted into a narration at word boundaries before cues are
//! resolved, so cues, captions and the audio all follow the padded narration.

use cutline_render::pipeline::{self, LoadError};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

/// What the recogniser heard, in source seconds.
const HEARD: &[(&str, f64, f64)] = &[
    ("Stay", 0.2, 0.5),
    ("organized.", 0.5, 1.1),
    ("Need", 1.5, 1.8),
    ("more", 1.8, 2.0),
    ("room?", 2.0, 2.4),
    ("Written", 3.0, 3.4),
    ("in", 3.4, 3.5),
    ("Rust.", 3.5, 4.0),
];

fn setup(d: &Path, gaps: &str) -> std::path::PathBuf {
    let tc = test_toolchain().unwrap();
    gen::still(tc, &d.join("bg.png"), 320, 180, "0x223344").unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "sine=f=220:d=5", "-ac", "2"]).arg(d.join("vo.wav"));
    gen::run(c).unwrap();
    let words: Vec<_> = HEARD.iter().map(|(w, s, e)| serde_json::json!({"w": w, "start": s, "end": e})).collect();
    std::fs::write(d.join("vo.json"), serde_json::json!({"asset": "vo", "words": words}).to_string()).unwrap();
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, target: 'custom', width: 320, height: 180, fps: 30,
              assets: {{bg: {{path: 'bg.png'}}, vo: {{path: 'vo.wav', role: 'voice', transcript: 'vo.json', gaps: {gaps}}}}},
              cues: {{room: 'Need more room', rust: 'written in Rust'}},
              main: [{{id: 'a', clip: 'bg', end_at: 'cue:room'}}, {{id: 'b', clip: 'bg', end_at: 'cue:rust'}}, {{id: 'c', clip: 'bg', duration: 2}}],
              layers: [{{type: 'audio', asset: 'vo', start: 0.5}}],
              captions: {{style: 'clean_sentence'}}}}"
        ),
    )
    .unwrap();
    p
}

fn cue(l: &pipeline::Loaded, name: &str) -> f64 {
    l.cues.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no cue {name}")).at
}

#[test]
fn gaps_pad_the_narration_and_move_cues_and_captions() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // without gaps
    let l0 = pipeline::load(&setup(d.path(), "[]")).unwrap_or_else(|e| panic!("{e}"));
    assert!((cue(&l0, "room") - 2.0).abs() < 1e-6 && (cue(&l0, "rust") - 3.5).abs() < 1e-6);
    assert!(l0.gaps.is_empty());
    // 0.7 s in front of "Need more room" (by cue), 0.3 s behind it (by phrase)
    let p = setup(d.path(), "[{before: 'cue:room', add: 0.7}, {after: 'need more room', add: 0.3}]");
    let l = pipeline::load(&p).unwrap_or_else(|e| panic!("{e}"));
    assert!(l.diags.is_empty(), "{:?}", l.diags);
    // the cut is in the middle of the pause before "Need" (1.1–1.5 s) and after "room?" (2.4–3.0 s)
    assert_eq!(l.gaps.len(), 2);
    assert_eq!((l.gaps[0].side.as_str(), l.gaps[0].matched.as_str()), ("before", "Need more room?"));
    assert!((l.gaps[0].source_at.unwrap() - 1.3).abs() < 1e-6 && (l.gaps[0].at - 1.3).abs() < 1e-6);
    assert!((l.gaps[1].source_at.unwrap() - 2.7).abs() < 1e-6 && (l.gaps[1].at - 3.4).abs() < 1e-6, "{:?}", l.gaps[1]);
    // cues follow: "Need" is 0.7 s later, "Written" 1.0 s later
    assert!((cue(&l, "room") - 2.7).abs() < 1e-6, "{}", cue(&l, "room"));
    assert!((cue(&l, "rust") - 4.5).abs() < 1e-6, "{}", cue(&l, "rust"));
    // the asset is now the padded narration: 5 s + 1 s, same channels
    let vo = &l.project.assets["vo"];
    assert!(vo.gaps.is_empty() && vo.path.contains("gaps") && vo.path.ends_with(".wav"), "{vo:?}");
    let info = cutline_media::probe(tc, Path::new(&vo.path), None).unwrap();
    assert!((info.duration - 6.0).abs() < 0.02, "{}", info.duration);
    assert_eq!(info.audio.as_ref().unwrap().channels, 2);
    // the inserted stretch is silent and the narration around it is not
    let level = |from: f64, len: f64| -> f64 {
        let o = std::process::Command::new(&tc.ffmpeg)
            .args(["-hide_banner", "-nostdin", "-ss", &format!("{from}"), "-t", &format!("{len}"), "-i"])
            .arg(&vo.path)
            .args(["-af", "volumedetect", "-f", "null", "-"])
            .output()
            .unwrap();
        let s = String::from_utf8_lossy(&o.stderr).to_string();
        let v = s.split("max_volume:").nth(1).and_then(|x| x.split_whitespace().next().map(str::to_string));
        v.and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("{s}"))
    };
    assert!(level(1.4, 0.5) < -80.0, "silence inserted at 1.3–2.0 s: {}", level(1.4, 0.5));
    assert!(level(0.5, 0.5) > -30.0 && level(2.2, 0.5) > -30.0 && level(4.0, 0.5) > -30.0);
    assert!(level(3.45, 0.2) < -80.0, "silence inserted at 3.4–3.7 s");
    // captions are timed from the padded narration (layer starts at 0.5 s)
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    assert!(pr.extras.srt.contains("00:00:02,700 -->"), "{}", pr.extras.srt);
    // loading again reuses the cached narration
    let stamp = std::fs::metadata(&vo.path).unwrap().modified().unwrap();
    let l2 = pipeline::load(&p).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(l2.project.assets["vo"].path, vo.path);
    assert_eq!(std::fs::metadata(&vo.path).unwrap().modified().unwrap(), stamp);
    // a regenerated narration (other timings) is cut again at its own word boundaries
    let words: Vec<_> =
        HEARD.iter().map(|(w, s, e)| serde_json::json!({"w": w, "start": s + 0.2, "end": e + 0.2})).collect();
    std::fs::write(d.path().join("vo.json"), serde_json::json!({"asset": "vo", "words": words}).to_string()).unwrap();
    let l3 = pipeline::load(&p).unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(l3.project.assets["vo"].path, vo.path);
    assert!((l3.gaps[0].source_at.unwrap() - 1.5).abs() < 1e-6);
    assert!((cue(&l3, "room") - 2.9).abs() < 1e-6);
}

#[test]
fn gaps_at_the_ends_and_errors() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // before the first word and after the last: silence at the very start and end
    let p = setup(d.path(), "[{before: 'Stay organized', add: 0.5}, {after: 'in Rust', add: 0.25}]");
    let l = pipeline::load(&p).unwrap_or_else(|e| panic!("{e}"));
    assert!((cue(&l, "room") - 2.5).abs() < 1e-6);
    assert!(l.gaps[1].source_at.is_none());
    let info = cutline_media::probe(tc, Path::new(&l.project.assets["vo"].path), None).unwrap();
    assert!((info.duration - 5.75).abs() < 0.02, "{}", info.duration);

    let errs = |gaps: &str| -> Vec<(String, String)> {
        match pipeline::load(&setup(d.path(), gaps)) {
            Err(LoadError::Invalid(d)) => d.into_iter().map(|x| (x.code, x.path)).collect(),
            Ok(l) => panic!("loaded: {:?}", l.diags),
            Err(e) => panic!("{e}"),
        }
    };
    let e = errs("[{before: 'calendar view', add: 0.5}]");
    assert_eq!(e, [("E_GAP_NOT_FOUND".to_string(), "/assets/vo/gaps/0/before".to_string())]);
    assert_eq!(errs("[{after: 'cue:nope', add: 0.5}]")[0].0, "E_UNKNOWN_CUE");
    assert_eq!(errs("[{after: 'in Rust', occurrence: 2, add: 0.5}]")[0].0, "E_GAP_NOT_FOUND");
    assert_eq!(errs("[{before: 'in Rust', after: 'Stay', add: 0.5}]")[0].0, "E_FIELD_CONFLICT");
    assert_eq!(
        errs("[{before: 'in Rust', add: -1}]")[0],
        ("E_BAD_VALUE".to_string(), "/assets/vo/gaps/0/add".to_string())
    );
}
