//! `auto.snap_to_beats` moves cuts onto the music's beats (in order, with ripple) when they are
//! within `max_shift`, and leaves the rest alone.

use cutline_render::pipeline;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;

#[test]
fn cuts_snap_to_nearby_beats() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("bg.png"), 320, 180, "0x223344").unwrap();
    // 120 BPM kick (60 Hz body + click) with beats at 0.25 + k·0.5 s
    let kick = "gt(t,0.25)*(0.8*sin(2*PI*60*t)*exp(-mod(t-0.25,0.5)*30)+0.5*(random(0)-0.5)*exp(-mod(t-0.25,0.5)*400))";
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &format!("aevalsrc='{kick}':s=48000:d=12"), "-ac", "2"])
        .arg(d.path().join("music.wav"));
    gen::run(c).unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, target: 'custom', width: 320, height: 180, fps: 30,
          assets: {bg: {path: 'bg.png'}, music: {path: 'music.wav', role: 'music'}},
          main: [{id: 'a', clip: 'bg', duration: 2.15},
                 {id: 'b', clip: 'bg', duration: 1.6},
                 {id: 'c', clip: 'bg', duration: 1.25},
                 {id: 'd', clip: 'bg', duration: 2}],
          layers: [{type: 'audio', asset: 'music'}],
          auto: {snap_to_beats: {max_shift: 0.15}},
          audio: {voice_chain: 'raw'}}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    let beats = cutline_render::auto::beats_for(&l, "music").unwrap().expect("beats");
    assert!((beats.bpm - 120.0).abs() < 2.0, "bpm {}", beats.bpm);
    let brand = cutline_render::extras::load_brand(&l, &l.project).unwrap();
    let (q, notes) = cutline_render::auto::expand(&l, &l.project, &brand).unwrap();
    let (tl, diags) = cutline_core::resolve(&q, &l.media, &pipeline::context(&l.dir));
    assert!(!cutline_core::diag::has_errors(&diags), "{diags:?}");
    let tl = tl.unwrap();
    let cut = |i: usize| tl.secs(tl.clips[i].end());
    // a: 2.15 → beat 2.25; b then ends at 2.25 + 1.6 = 3.85 → beat 3.75;
    // c then ends at 5.0, 0.25 s from the nearest beats: left alone
    let tol = 0.04; // beat detection resolution (~23 ms frames) + frame rounding
    assert!((cut(0) - 2.25).abs() < tol, "cut a {}", cut(0));
    assert!((cut(1) - 3.75).abs() < tol, "cut b {}", cut(1));
    // c keeps its own length (1.25 s, rounded to whole frames)
    assert!((cut(2) - cut(1) - 1.25).abs() <= 0.5 / 30.0 + 1e-9, "c keeps its length: {}", cut(2) - cut(1));
    let moves = notes.iter().filter(|n| n.code == "I_BEAT_SNAP" && n.path.starts_with("/main/")).count();
    assert_eq!(moves, 2, "{notes:?}");
}

/// `bar:N` / `beat:N` times resolve against the music layer; snapping prefers bar starts.
#[test]
fn bar_and_beat_references_and_downbeat_preference() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("bg.png"), 320, 180, "0x223344").unwrap();
    // 120 BPM kick on every beat from 0; every fourth beat (0, 2, 4 s …) adds a loud bass note
    let kick = "0.5*sin(2*PI*60*t)*exp(-mod(t,0.5)*30)+0.3*(random(0)-0.5)*exp(-mod(t,0.5)*400)+0.45*sin(2*PI*(80+25*mod(floor(t/2),3))*t)*exp(-mod(t,2)*2.5)";
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &format!("aevalsrc='{kick}':s=48000:d=20"), "-ac", "2"])
        .arg(d.path().join("music.wav"));
    gen::run(c).unwrap();
    let project = |main: &str, auto: &str| {
        format!(
            "{{version: 1, target: 'custom', width: 320, height: 180, fps: 30,
              assets: {{bg: {{path: 'bg.png'}}, music: {{path: 'music.wav', role: 'music'}}}},
              main: [{main}], layers: [{{type: 'audio', asset: 'music', start: 0}}], {auto}
              audio: {{voice_chain: 'raw'}}}}"
        )
    };
    let pp = d.path().join("p.json5");
    // bar 3 starts at 4 s; beat 14 is at 6.5 s; bar 5 beat 3 is at 9 s
    std::fs::write(
        &pp,
        project(
            "{id: 'a', clip: 'bg', end_at: 'bar:3'}, {id: 'b', clip: 'bg', end_at: 'beat:14'},
             {id: 'c', clip: 'bg', end_at: 'bar:5.3'}, {id: 'd', clip: 'bg', duration: 1}",
            "",
        ),
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    assert!(!l.diags.iter().any(|x| x.code == "W_BARS_UNCERTAIN"), "{:?}", l.diags);
    let tl = l.timeline.as_ref().expect("timeline");
    let cut = |i: usize| tl.secs(tl.clips[i].end());
    for (i, want) in [(0, 4.0), (1, 6.5), (2, 9.0)] {
        assert!((cut(i) - want).abs() <= 0.5 / 30.0 + 0.005, "cut {i} at {} (want {want})", cut(i));
    }
    // a cut at 3.7 s is nearer the beat at 3.5 s, but the bar start at 4.0 s is within reach
    std::fs::write(
        &pp,
        project(
            "{id: 'a', clip: 'bg', duration: 3.7}, {id: 'b', clip: 'bg', duration: 2}",
            "auto: {snap_to_beats: {}},",
        ),
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    let brand = cutline_render::extras::load_brand(&l, &l.project).unwrap();
    let (q, notes) = cutline_render::auto::expand(&l, &l.project, &brand).unwrap();
    let (tl, _) = cutline_core::resolve(&q, &l.media, &pipeline::context(&l.dir));
    let tl = tl.unwrap();
    assert!((tl.secs(tl.clips[0].end()) - 4.0).abs() < 0.03, "cut at {}: {notes:?}", tl.secs(tl.clips[0].end()));
}
