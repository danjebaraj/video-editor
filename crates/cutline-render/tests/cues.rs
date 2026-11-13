//! Cue references resolve against the voice-over transcript at load time.

use cutline_render::pipeline::{self, LoadError};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

/// Words with times in the voice-over file (source seconds).
const WORDS: &[(&str, f64, f64)] = &[
    ("Meet", 0.0, 0.3),
    ("Glance,", 0.3, 0.8),
    ("a", 0.8, 0.9),
    ("dashboard.", 0.9, 1.6),
    ("Seven", 2.0, 2.4),
    ("pains,", 2.4, 2.9),
    ("one", 3.0, 3.2),
    ("keystroke", 3.2, 3.8),
    ("away.", 3.8, 4.2),
    ("World", 4.6, 4.9),
    ("clocks", 4.9, 5.4),
    ("keep", 5.4, 5.6),
    ("cities.", 5.6, 6.2),
    ("The", 6.6, 6.7),
    ("calendar", 6.7, 7.2),
    ("jumps", 7.2, 7.6),
    ("ahead.", 7.6, 8.1),
    ("Install", 8.6, 9.0),
    ("it", 9.0, 9.1),
    ("today.", 9.1, 9.6),
];

fn setup(d: &Path) {
    let tc = test_toolchain().unwrap();
    gen::still(tc, &d.join("bg.png"), 320, 180, "0x223344").unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "sine=f=220:d=10", "-ac", "2"]).arg(d.join("vo.wav"));
    gen::run(c).unwrap();
    let words: Vec<_> = WORDS.iter().map(|(w, s, e)| serde_json::json!({"w": w, "start": s, "end": e})).collect();
    std::fs::write(d.join("vo.json"), serde_json::json!({"asset": "vo", "words": words}).to_string()).unwrap();
}

fn project(d: &Path, body: &str) -> std::path::PathBuf {
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, target: 'custom', width: 320, height: 180, fps: 30,
              assets: {{bg: {{path: 'bg.png'}}, vo: {{path: 'vo.wav', role: 'voice', transcript: 'vo.json'}}}},
              {body}}}"
        ),
    )
    .unwrap();
    p
}

#[test]
fn cues_drive_clip_cuts_and_layer_times() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path());
    let p = project(
        d.path(),
        "cues: {panes: 'seven panes', clocks: 'World clocks', cal: {phrase: 'calendar jumps', edge: 'end'}, outro: 'install it'},
         main: [{id: 'intro', clip: 'bg', end_at: 'cue:clocks'},
                {transition: {type: 'fade', duration: 0.4}},
                {id: 'time', clip: 'bg', end_at: 'cue:cal'},
                {id: 'end', clip: 'bg', end_at: 'cue:outro+1.5'}],
         layers: [{type: 'audio', asset: 'vo', start: 1.5},
                  {type: 'text', template: 'lower_third', text: 'Seven panes', start: 'cue:panes', end: 'cue:clocks-0.2'},
                  {type: 'sfx', asset: 'whoosh', at: 'cue:clocks+0.25'}],
         markers: [{at: 'cue:cal', title: 'Calendar'}]",
    );
    let l = pipeline::load(&p).unwrap_or_else(|e| panic!("{e}"));
    let tl = l.timeline.as_ref().expect("resolves");
    // the voice layer starts at 1.5 s, so every cue is 1.5 s after its word
    let fade_mid = |i: usize| {
        let t = tl.transitions.iter().find(|t| t.after == i).unwrap();
        tl.secs(t.start) + tl.secs(t.frames) / 2.0
    };
    assert!((fade_mid(0) - (1.5 + 4.6)).abs() < 1.0 / 30.0, "intro cut {}", fade_mid(0));
    assert!((tl.secs(tl.clips[1].end()) - (1.5 + 7.6)).abs() < 1.0 / 30.0, "edge: end of 'jumps'");
    assert!((tl.secs(tl.clips[2].end()) - (1.5 + 8.6 + 1.5)).abs() < 1.0 / 30.0, "offset +1.5");
    let text = tl.layers.iter().find(|x| x.layer.kind() == "text").unwrap();
    assert!((tl.secs(text.start) - (1.5 + 2.0)).abs() < 1.0 / 30.0);
    assert!((tl.secs(text.end) - (1.5 + 4.6 - 0.2)).abs() < 1.0 / 30.0);
    assert!((l.project.markers[0].at.seconds(tl.fps, 0.0) - 9.1).abs() < 1e-6);
    // "seven panes" matched the transcript's "Seven pains," fuzzily: a warning, not an error
    assert!(l.diags.iter().any(|x| x.code == "W_CUE_FUZZY" && x.path == "/cues/panes"), "{:?}", l.diags);
    let names: Vec<&str> = l.cues.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["cal", "clocks", "outro", "panes"]);
}

#[test]
fn cue_errors_point_at_the_problem() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path());
    let cases = [
        (
            "cues: {clocks: 'World clocks'}, main: [{id: 'c', clip: 'bg', duration: 3}],
             layers: [{type: 'audio', asset: 'vo'}, {type: 'text', template: 'callout', text: 'x', start: 'cue:clock'}]",
            "E_UNKNOWN_CUE",
            "/layers/1/start",
        ),
        (
            "cues: {clocks: 'World clocks'}, main: [{id: 'c', clip: 'bg', duration: 'cue:clocks'}],
             layers: [{type: 'audio', asset: 'vo'}]",
            "E_CUE_NOT_ALLOWED",
            "/main/0/duration",
        ),
        (
            "cues: {cal: 'calendar walked off'}, main: [{id: 'c', clip: 'bg', end_at: 'cue:cal'}],
             layers: [{type: 'audio', asset: 'vo'}]",
            "E_CUE_NOT_FOUND",
            "/cues/cal",
        ),
        (
            "cues: {clocks: {phrase: 'World clocks', occurrence: 2}}, main: [{id: 'c', clip: 'bg', end_at: 'cue:clocks'}],
             layers: [{type: 'audio', asset: 'vo'}]",
            "E_CUE_NOT_FOUND",
            "/cues/clocks",
        ),
        (
            "cues: {clocks: 'World clocks'}, main: [{id: 'c', clip: 'bg', end_at: 'cue:clocks'}],
             layers: [{type: 'audio', asset: 'vo', start: 'f:12'}]",
            "E_CUE_LAYER_TIME",
            "/layers/0",
        ),
    ];
    for (body, code, path) in cases {
        let p = project(d.path(), body);
        let diags = match pipeline::load(&p) {
            Err(LoadError::Invalid(ds)) => ds,
            Err(e) => panic!("{code}: unexpected load error {e}"),
            Ok(l) => l.diags,
        };
        assert!(
            diags.iter().any(|x| x.code == code && x.path == path),
            "want {code} at {path}, got {:?}",
            diags.iter().map(|x| (&x.code, &x.path, &x.message)).collect::<Vec<_>>()
        );
    }
    // the not-found error names the closest phrase
    let p = project(
        d.path(),
        "cues: {cal: 'the calender walked'}, main: [{id: 'c', clip: 'bg', end_at: 'cue:cal'}], layers: [{type: 'audio', asset: 'vo'}]",
    );
    let Err(LoadError::Invalid(ds)) = pipeline::load(&p) else { panic!("expected an error") };
    let hint = ds[0].hint.clone().unwrap_or_default();
    assert!(hint.contains("calendar"), "hint: {hint}");
}

#[test]
fn cues_from_a_voice_layer_anchored_to_a_clip() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path());
    // the voice starts 0.5 s into `intro`, which starts after a 2 s title and a 0.4 s fade
    let p = project(
        d.path(),
        "cues: {clocks: 'World clocks', cal: 'calendar jumps'},
         main: [{id: 'title', clip: 'bg', duration: 2},
                {transition: {type: 'fade', duration: 0.4}},
                {id: 'intro', clip: 'bg', end_at: 'cue:clocks'},
                {id: 'time', clip: 'bg', end_at: 'cue:cal'},
                {id: 'end', clip: 'bg', duration: 1}],
         layers: [{type: 'audio', asset: 'vo', anchor: 'intro', offset: 0.5},
                  {type: 'text', template: 'callout', text: 'x', start: 'cue:clocks', duration: 1}]",
    );
    let l = pipeline::load(&p).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let tl = l.timeline.as_ref().unwrap();
    let start = 2.0 - 0.4 + 0.5; // intro start + offset
    let clocks = l.cues.iter().find(|c| c.name == "clocks").unwrap();
    assert!((clocks.at - (start + 4.6)).abs() < 1.0 / 30.0, "cue at {}", clocks.at);
    assert!((tl.secs(tl.clips[1].end()) - clocks.at).abs() < 1.0 / 30.0, "intro cut on the cue");
    let voice = tl.layers.iter().find(|x| x.layer.kind() == "audio").unwrap();
    assert!((tl.secs(voice.start) - start).abs() < 1.0 / 30.0);
    let text = tl.layers.iter().find(|x| x.layer.kind() == "text").unwrap();
    assert!((tl.secs(text.start) - clocks.at).abs() < 1.0 / 30.0);
}

#[test]
fn anchoring_the_voice_after_its_own_cues_is_a_cycle() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path());
    // `b` starts where `a` ends, and `a` ends on a cue measured from a voice starting at `b`
    let p = project(
        d.path(),
        "cues: {clocks: 'World clocks'},
         main: [{id: 'a', clip: 'bg', end_at: 'cue:clocks'}, {id: 'b', clip: 'bg', duration: 3}],
         layers: [{type: 'audio', asset: 'vo', anchor: 'b'}]",
    );
    match pipeline::load(&p) {
        Err(LoadError::Invalid(ds)) => {
            assert!(ds.iter().any(|x| x.code == "E_CUE_CYCLE" && x.path == "/layers/0/anchor"), "{ds:?}")
        }
        Err(e) => panic!("{e}"),
        Ok(l) => panic!("expected a cycle error, got {:?}", l.diags),
    }
}
