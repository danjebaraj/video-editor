//! M2 acceptance: loudness accuracy, ducking depth/timing, click-free cuts, tones with
//! transitions.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{dominant_freq, read_audio};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

fn render(project: &Path) -> (PathBuf, pipeline::Prepared) {
    let l = pipeline::load(project).unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let opts = RenderOptions { jobs: 3, ..Default::default() };
    let (plan, _) = pipeline::render(&l, &pr, &opts).unwrap();
    (plan.output, pr)
}

fn tone_video(tc: &cutline_deps::Toolchain, p: &Path, hz: f64, dur: f64, amp: f64) {
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &format!("testsrc2=s=640x360:r=30:d={dur}")]);
    c.args(["-f", "lavfi", "-i", &format!("aevalsrc='{amp}*sin(2*PI*{hz}*t)':s=48000:d={dur}")]);
    c.args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "pcm_s16le", "-ac", "2"]).arg(p);
    gen::run(c).unwrap();
}

/// Goertzel magnitude (dB) of `hz` in consecutive windows.
fn goertzel_db(s: &[f32], hz: f64, win: usize) -> Vec<f64> {
    let k = 2.0 * (2.0 * std::f64::consts::PI * hz / 48000.0).cos();
    s.chunks(win)
        .map(|c| {
            let (mut s1, mut s2) = (0f64, 0f64);
            for &x in c {
                let s0 = x as f64 + k * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            let p = s1 * s1 + s2 * s2 - k * s1 * s2;
            10.0 * (p.max(1e-20) / (c.len() as f64).powi(2)).log10()
        })
        .collect()
}

#[test]
fn loudness_within_half_lu() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    tone_video(tc, &d.path().join("a.mp4"), 440.0, 6.0, 0.05);
    gen::noise_bed(tc, &d.path().join("m.wav"), 8.0, 0.3).unwrap();
    for (target, tp) in [(-14.0, -1.0), (-16.0, -1.5)] {
        let pp = d.path().join(format!("p{}.json5", -target as i32));
        std::fs::write(
            &pp,
            format!(
                r#"{{version: 1, name: "loud{}", target: "long_1080p",
                  assets: {{a: {{path: "a.mp4"}}, m: {{path: "m.wav", role: "music"}}}},
                  main: [{{id: "c", clip: "a", in: 0, out: 6}}],
                  layers: [{{type: "audio", asset: "m", gain_db: -10}}],
                  audio: {{loudness: {{integrated: {target}, true_peak: {tp}}}, voice_chain: "raw"}} }}"#,
                -target as i32
            ),
        )
        .unwrap();
        let (out, _) = render(&pp);
        let l = cutline_media::probe::measure_loudness(tc, &out).unwrap();
        assert!((l.integrated_lufs - target).abs() <= 0.5, "integrated {} vs {target}", l.integrated_lufs);
        assert!(l.true_peak_dbtp <= tp + 0.05, "true peak {} > {tp}", l.true_peak_dbtp);
    }
}

/// The true-peak limit holds in the rendered file, after the AAC encode, not just in the
/// normalized WAV. Sharp clicks make AAC overshoot by well over 1 dB (a WAV limited to
/// −1 dBTP used to come out at +0.8 dBTP).
#[test]
fn true_peak_holds_after_aac_encode() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    tone_video(tc, &d.path().join("a.mp4"), 440.0, 6.0, 0.0);
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "aevalsrc='0.8*sgn(sin(2*PI*3*t))*exp(-30*mod(t,0.33))*random(0)':s=48000:d=6"]);
    c.args(["-ac", "2"]).arg(d.path().join("clicks.wav"));
    gen::run(c).unwrap();
    let tp = -1.0;
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        format!(
            r#"{{version: 1, name: "clicks", target: "long_1080p",
              assets: {{a: {{path: "a.mp4"}}, m: {{path: "clicks.wav", role: "music"}}}},
              main: [{{id: "c", clip: "a", in: 0, out: 6, audio: "mute"}}],
              layers: [{{type: "audio", asset: "m"}}],
              audio: {{loudness: {{integrated: -14, true_peak: {tp}}}, voice_chain: "raw"}} }}"#
        ),
    )
    .unwrap();
    let (out, _) = render(&pp);
    let l = cutline_media::probe::measure_loudness(tc, &out).unwrap();
    assert!(l.true_peak_dbtp <= tp + 0.05, "encoded true peak {} > {tp}", l.true_peak_dbtp);
}

#[test]
fn ducking_depth_and_timing() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("bg.png"), 640, 360, "0x335577").unwrap();
    let speech = [(2.0, 4.0), (6.0, 7.5)];
    gen::bursts(tc, &d.path().join("vo.wav"), 10.0, &speech, 1000.0).unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "sine=frequency=200:sample_rate=48000:duration=10", "-af", "volume=-6dB", "-ac", "2"])
        .arg(d.path().join("music.wav"));
    gen::run(c).unwrap();
    let (amount, attack, release) = (-12.0, 0.12, 0.45);
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        format!(
            r#"{{version: 1, name: "duck", target: "long_1080p",
              assets: {{bg: {{path: "bg.png"}}, vo: {{path: "vo.wav", role: "voice"}}, music: {{path: "music.wav", role: "music"}}}},
              main: [{{id: "c", clip: "bg", duration: 10}}],
              layers: [{{type: "audio", asset: "vo"}}, {{type: "audio", asset: "music"}}],
              audio: {{voice_chain: "raw", duck: {{amount_db: {amount}, attack: {attack}, release: {release}}}}} }}"#
        ),
    )
    .unwrap();
    let (out, _) = render(&pp);
    let s = read_audio(tc, &out, 0.0, None, 1).unwrap();
    let win = 480; // 10 ms
    let lv = goertzel_db(&s, 200.0, win);
    let at = |t: f64| lv[(t / 0.01) as usize];
    let base = (at(1.0) + at(5.2) + at(9.0)) / 3.0;
    let ducked = (at(3.0) + at(7.0)) / 2.0;
    let depth = ducked - base;
    assert!((depth - amount).abs() <= 1.5, "duck depth {depth:.2} dB (want {amount} ± 1.5)");
    // half-depth crossings: attack ends at speech start, release starts at speech end
    let half = base + amount / 2.0;
    for &(s0, s1) in &speech {
        let from = ((s0 - 0.6) / 0.01) as usize;
        let down = (from..).find(|&i| lv[i] < half).unwrap() as f64 * 0.01 + 0.005;
        let want_down = s0 - attack / 2.0;
        assert!((down - want_down).abs() <= 0.05, "attack midpoint {down:.3}s, want {want_down:.3}s");
        let from = ((s1 - 0.05) / 0.01) as usize;
        let up = (from..).find(|&i| lv[i] > half).unwrap() as f64 * 0.01 + 0.005;
        let want_up = s1 + release / 2.0;
        assert!((up - want_up).abs() <= 0.05, "release midpoint {up:.3}s, want {want_up:.3}s");
    }
}

#[test]
fn no_clicks_at_cuts_and_tones_with_transitions() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    for (n, hz) in [("a", 440.0), ("b", 660.0), ("c", 880.0)] {
        tone_video(tc, &d.path().join(format!("{n}.mp4")), hz, 6.0, 0.5);
    }
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        r#"{version: 1, name: "cuts", target: "long_1080p",
          assets: {a: {path: "a.mp4"}, b: {path: "b.mp4"}, c: {path: "c.mp4"}},
          main: [{id: "c1", clip: "a", in: 0.37, out: 2.0},
                 {id: "c2", clip: "b", in: 1.11, out: 3.0},
                 {transition: {type: "wipeleft", duration: 0.4}},
                 {id: "c3", clip: "c", in: 0.5, out: 2.5},
                 {transition: {type: "fade", duration: 0.3, audio: "cut"}},
                 {id: "c4", clip: "a", in: 3.13, out: 5.0}],
          audio: {voice_chain: "raw", loudness: {integrated: -16}}}"#,
    )
    .unwrap();
    let (out, pr) = render(&pp);
    let tl = &pr.timeline;
    let s = read_audio(tc, &out, 0.0, None, 1).unwrap();
    let peak = s.iter().fold(0f32, |m, x| m.max(x.abs())) as f64;
    // largest legit sample step of the highest tone at this level
    let legit = peak * 2.0 * std::f64::consts::PI * 880.0 / 48000.0;
    // hard cut between c1|c2 and the audio-cut midpoint of the fade c3|c4
    let cut1 = tl.secs(tl.clips[1].start);
    let t2 = &tl.transitions[1];
    let cut2 = tl.secs(t2.start) + tl.secs(t2.frames) / 2.0;
    for cut in [cut1, cut2] {
        let c = (cut * 48000.0) as usize;
        let max_step = s[c - 480..c + 480].windows(2).map(|w| (w[1] - w[0]).abs() as f64).fold(0.0, f64::max);
        assert!(max_step <= legit * 1.5, "click at {cut:.3}s: step {max_step:.4} > {:.4}", legit * 1.5);
    }
    // tones mid-clip
    for (c, hz) in tl.clips.iter().zip([440.0, 660.0, 880.0, 440.0]) {
        let mid = tl.secs(c.start + c.frames / 2);
        let w = read_audio(tc, &out, mid - 0.05, Some(0.1), 1).unwrap();
        let f = dominant_freq(&w);
        assert!((f - hz).abs() < hz * 0.03, "clip {} tone {f:.1} want {hz}", c.id);
    }
}

#[test]
fn audio_png_renders() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    tone_video(tc, &d.path().join("a.mp4"), 440.0, 3.0, 0.3);
    gen::noise_bed(tc, &d.path().join("m.wav"), 4.0, 0.2).unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(&pp, r#"{version: 1, name: "viz", assets: {a: {path: "a.mp4"}, m: {path: "m.wav", role: "music"}}, main: [{id: "c", clip: "a"}], layers: [{type: "audio", asset: "m"}]}"#).unwrap();
    let l = pipeline::load(&pp).unwrap();
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let (plan, _) = pipeline::render(&l, &pr, &RenderOptions::default()).unwrap();
    let png = d.path().join("audio.png");
    cutline_render::audio_plan::audio_png(tc, &plan.audio, &plan.output, &png).unwrap();
    let img = image::open(&png).unwrap();
    assert_eq!(img.width(), 1568);
    assert!(img.height() > 300 && img.height() <= 1568);
}
