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
