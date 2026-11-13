//! `audio.music_level` balances the music against the voice; `align: end` lands the music's
//! own ending on the end of the video.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{dominant_freq, read_audio};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;

#[test]
fn music_level_and_backtimed_ending() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("bg.png"), 320, 180, "0x223344").unwrap();
    // voice: 600 Hz bursts, silent for the last 1.5 s
    gen::bursts(tc, &d.path().join("vo.wav"), 6.0, &[(0.5, 2.0), (2.6, 4.5)], 600.0).unwrap();
    // loud music: 200 Hz, with a 1000 Hz "ending" in its last second (12 s long)
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "aevalsrc='0.7*sin(2*PI*if(gt(t,11),1000,200)*t)':s=48000:d=12", "-ac", "2"])
        .arg(d.path().join("music.wav"));
    gen::run(c).unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, name: 'mus', target: 'custom', width: 320, height: 180, fps: 30,
          assets: {bg: {path: 'bg.png'}, vo: {path: 'vo.wav', role: 'voice'}, music: {path: 'music.wav', role: 'music'}},
          main: [{id: 'c', clip: 'bg', duration: 6}],
          layers: [{type: 'audio', asset: 'vo'}, {type: 'audio', asset: 'music', align: 'end'}],
          audio: {voice_chain: 'raw', music_level: 18}}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let (plan, exec) = pipeline::render(&l, &pr, &RenderOptions { jobs: 3, ..Default::default() }).unwrap();

    // the stems after balancing: music − 12 dB ducking sits 18 LU under the voice
    let lv = cutline_media::probe::measure_loudness(tc, &plan.audio.stems["voice"]).unwrap();
    let lm = cutline_media::probe::measure_loudness(tc, &plan.audio.stems["music"]).unwrap();
    let below = lv.integrated_lufs - (lm.integrated_lufs - 12.0);
    assert!((below - 18.0).abs() < 0.6, "music sits {below:.2} LU under the voice");
    let gain = exec.loudness.as_ref().and_then(|x| x.music_gain_db).expect("gain reported");
    assert!(gain < -5.0, "loud music was turned down ({gain} dB)");

    // backtimed: the last half second plays the music's 1000 Hz ending, the middle its 200 Hz body
    let tail = read_audio(tc, &plan.output, 5.4, Some(0.5), 1).unwrap();
    assert!((dominant_freq(&tail) - 1000.0).abs() < 30.0, "ending at the end: {}", dominant_freq(&tail));
    let body = read_audio(tc, &plan.output, 2.1, Some(0.4), 1).unwrap();
    assert!((dominant_freq(&body) - 200.0).abs() < 20.0, "body before it: {}", dominant_freq(&body));
}
