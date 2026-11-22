//! FEAT-neat-osprey-DKK7: `cutline audio` (`pipeline::render_audio`) writes one export's final mix
//! without any picture work, and its samples are the production render's mix before muxing.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::read_audio;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

/// A 6 s clip with its own sound, a voice-over starting at 1.0 s, music ducked under it and a
/// sound effect at 4.2 s; a full export and a 1.0–4.0 s cut-out.
fn fixture(tc: &cutline_deps::Toolchain, d: &Path, music_gain: f64) -> std::path::PathBuf {
    if !d.join("clip.mp4").exists() {
        let mut c = tc.ffmpeg_cmd();
        c.args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=320x180:r=30:d=6",
            "-f",
            "lavfi",
            "-i",
            "sine=f=300:sample_rate=48000:d=6",
        ])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "pcm_s16le", "-ac", "2", "-shortest"])
        .arg(d.join("clip.mp4"));
        gen::run(c).unwrap();
        gen::bursts(tc, &d.join("vo.wav"), 3.0, &[(0.2, 1.2), (1.6, 2.8)], 700.0).unwrap();
        gen::noise_bed(tc, &d.join("music.wav"), 8.0, 0.2).unwrap();
        gen::tone(tc, &d.join("hit.wav"), 1500.0, 0.3, -6.0).unwrap();
    }
    let p = d.join(format!("p{}.json5", music_gain as i32));
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'mix', target: 'custom', width: 320, height: 180, fps: 30,
              assets: {{clip: {{path: 'clip.mp4'}}, vo: {{path: 'vo.wav', role: 'voice'}}, music: {{path: 'music.wav', role: 'music'}}, hit: {{path: 'hit.wav', role: 'sfx'}}}},
              main: [{{id: 'c', clip: 'clip', gain_db: -6}}],
              layers: [{{type: 'audio', asset: 'vo', start: 1.0}},
                       {{type: 'audio', asset: 'music', gain_db: {music_gain}, fade_out: 0.5}},
                       {{type: 'sfx', asset: 'hit', at: 4.2}}],
              exports: [{{name: 'full'}}, {{name: 'cut', range: [1.0, 4.0]}}]}}"
        ),
    )
    .unwrap();
    p
}

fn samples(tc: &cutline_deps::Toolchain, wav: &Path) -> Vec<f32> {
    read_audio(tc, wav, 0.0, None, 2).unwrap()
}

#[test]
fn the_audio_only_mix_is_the_production_mix() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let p = fixture(tc, d.path(), -4.0);
    let l = pipeline::load(&p).unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    for (name, e) in pipeline::exports(&l.project) {
        let label = name.clone().unwrap();
        // audio only, from a clean cache: no picture is rendered
        let pr = pipeline::prepare_audio(&l, name.clone(), &e).unwrap();
        let wav = d.path().join(format!("review-{label}.wav"));
        let r = pipeline::render_audio(&l, &pr, &wav, &RenderOptions::default()).unwrap();
        assert!(r.audible);
        let segs = d.path().join(".cutline/cache/segments");
        assert!(!segs.exists() || std::fs::read_dir(&segs).unwrap().next().is_none(), "no video segment was rendered");
        // production: the mix just before muxing, cut to the export's range
        let pr = pipeline::prepare(&l, name, &e, false).unwrap();
        let opts = pipeline::options_for(&pr, &RenderOptions { jobs: 2, ..Default::default() });
        let (plan, _) = pipeline::render(&l, &pr, &opts).unwrap();
        let full = samples(tc, &plan.audio.final_wav);
        let (from, to) =
            plan.delivery.range.map(|(a, b)| (a as usize * 1600 * 2, b as usize * 1600 * 2)).unwrap_or((0, full.len()));
        let want = &full[from..to.min(full.len())];
        let got = samples(tc, &wav);
        assert_eq!(got.len(), want.len(), "{label}: same length");
        let worst = got.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-6, "{label}: samples differ by up to {worst}");
        assert!((r.duration - plan.delivery.frames(plan.frames) as f64 / 30.0).abs() < 1e-6, "{label}: {}", r.duration);
        std::fs::remove_dir_all(&segs).ok();
    }
}

#[test]
fn a_gain_change_rebuilds_only_the_mix_and_silence_is_reported() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let take = |gain: f64, out: &str| {
        let l = pipeline::load(&fixture(tc, d.path(), gain)).unwrap();
        let (name, e) = pipeline::exports(&l.project).remove(0);
        let pr = pipeline::prepare_audio(&l, name, &e).unwrap();
        pipeline::render_audio(&l, &pr, &d.path().join(out), &RenderOptions::default()).unwrap()
    };
    let a = take(-4.0, "take1.wav");
    let b = take(-16.0, "take2.wav");
    assert_eq!(a.segments_rendered, 1);
    assert_eq!((b.segments_rendered, b.segments_cached), (0, 1), "the clip's sound is reused");
    let (x, y) = (samples(tc, &d.path().join("take1.wav")), samples(tc, &d.path().join("take2.wav")));
    assert_eq!(x.len(), y.len());
    assert!(x.iter().zip(&y).any(|(p, q)| (p - q).abs() > 1e-3), "the music change is heard");

    // a timeline with nothing to hear: silence, and said so
    gen::still(tc, &d.path().join("bg.png"), 320, 180, "0x223344").unwrap();
    let p = d.path().join("silent.json5");
    std::fs::write(&p, "{version: 1, target: 'custom', width: 320, height: 180, fps: 30, assets: {bg: {path: 'bg.png'}}, main: [{id: 'b', clip: 'bg', duration: 2}]}").unwrap();
    let l = pipeline::load(&p).unwrap();
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare_audio(&l, name, &e).unwrap();
    let r = pipeline::render_audio(&l, &pr, &d.path().join("silent.wav"), &RenderOptions::default()).unwrap();
    assert!(!r.audible);
    assert!((r.duration - 2.0).abs() < 1e-6);
}
