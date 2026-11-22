//! FEAT-magic-alpaca-JPD6: split edits (J-cut and L-cut) with existing fields, as in the recipe
//! in docs/CLAUDE_GUIDE.md ("Split edits"). Shot A is red with a 400 Hz "voice", shot B blue
//! with 1000 Hz (whole cycles in every 5 ms window, so neither leaks into the other). The picture cuts at 2.0 s; in the J-cut B's sound starts at 1.5 s, in the L-cut
//! A's sound carries on to 2.5 s. Each clip's own sound is muted and placed as an audio layer,
//! so no dialogue plays twice.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{read_audio, read_frames};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

fn shot(tc: &cutline_deps::Toolchain, out: &Path, color: &str, hz: u32) {
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &format!("color=c={color}:s=320x180:r=30:d=5")])
        .args(["-f", "lavfi", "-i", &format!("sine=frequency={hz}:sample_rate=48000:duration=5")])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "pcm_s16le", "-ac", "2", "-shortest"])
        .arg(out);
    gen::run(c).unwrap();
}

/// Goertzel level (dB) of `hz` in 5 ms windows.
fn level(s: &[f32], hz: f64) -> Vec<f64> {
    let k = 2.0 * (2.0 * std::f64::consts::PI * hz / 48000.0).cos();
    s.chunks(240)
        .map(|c| {
            let (mut s1, mut s2) = (0f64, 0f64);
            for &x in c {
                let s0 = x as f64 + k * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            10.0 * ((s1 * s1 + s2 * s2 - k * s1 * s2).max(1e-20) / (c.len() as f64).powi(2)).log10()
        })
        .collect()
}

/// Seconds where the tone is on (within 20 dB of its loudest window): (first, last).
fn on_span(s: &[f32], hz: f64) -> (f64, f64) {
    let l = level(s, hz);
    let top = l.iter().cloned().fold(f64::MIN, f64::max);
    let on: Vec<usize> = (0..l.len()).filter(|i| l[*i] > top - 20.0).collect();
    (on[0] as f64 * 0.005, (on[on.len() - 1] + 1) as f64 * 0.005)
}

fn render(d: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let p = d.join(format!("{name}.json5"));
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: '{name}', target: 'custom', width: 320, height: 180, fps: 30, brand_bumpers: false, watermark: false,
              assets: {{a: {{path: 'a.mp4'}}, b: {{path: 'b.mp4'}}}},
              audio: {{voice_chain: 'raw'}},
              {body}}}"
        ),
    )
    .unwrap();
    let l = pipeline::load(&p).unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    pipeline::render(&l, &pr, &RenderOptions { jobs: 3, ..Default::default() }).unwrap().0.output
}

fn check(tc: &cutline_deps::Toolchain, out: &Path, a_until: f64, b_from: f64) {
    // the picture switches at 2.0 s (frame 60) within a frame
    let f = read_frames(tc, out, &[58, 59, 60, 61]).unwrap();
    let red = |i: usize| f[i].get_pixel(160, 90)[0] > 150;
    assert!(red(0) && red(1) && !red(2) && !red(3), "picture cut at frame 60");
    let s = read_audio(tc, out, 0.0, None, 1).unwrap();
    let frame = 1.0 / 30.0;
    let (a0, a1) = on_span(&s, 400.0);
    let (b0, b1) = on_span(&s, 1000.0);
    assert!(a0 < frame && (a1 - a_until).abs() <= frame, "A's sound {a0:.3}–{a1:.3}, want to {a_until}");
    assert!((b0 - b_from).abs() <= frame && b1 > 3.9, "B's sound {b0:.3}–{b1:.3}, want from {b_from}");
}

#[test]
fn j_cut_and_l_cut() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    shot(tc, &d.path().join("a.mp4"), "red", 400);
    shot(tc, &d.path().join("b.mp4"), "blue", 1000);
    // J-cut: B is heard from 1.5 s, half a second before it is seen
    let j = render(
        d.path(),
        "jcut",
        "main: [{id: 'a', clip: 'a', in: 1, out: 3, audio: 'mute'}, {id: 'b', clip: 'b', in: 1, out: 3, audio: 'mute'}],
         layers: [{type: 'audio', asset: 'a', role: 'dialogue', src_in: 1, start: 0, end: 1.5},
                  {type: 'audio', asset: 'b', role: 'dialogue', src_in: 0.5, start: 1.5, end: 4}]",
    );
    check(tc, &j, 1.5, 1.5);
    // L-cut: A is heard to 2.5 s, half a second after the cut
    let l = render(
        d.path(),
        "lcut",
        "main: [{id: 'a', clip: 'a', in: 1, out: 3, audio: 'mute'}, {id: 'b', clip: 'b', in: 1, out: 3, audio: 'mute'}],
         layers: [{type: 'audio', asset: 'a', role: 'dialogue', src_in: 1, start: 0, end: 2.5},
                  {type: 'audio', asset: 'b', role: 'dialogue', src_in: 1.5, start: 2.5, end: 4}]",
    );
    check(tc, &l, 2.5, 2.5);
}
