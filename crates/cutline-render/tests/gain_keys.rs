//! FEAT-magic-alpaca-JPD6: an editable gain curve on an audio layer (`gain_keys`).
//!
//! A constant 440 Hz tone on an `sfx` layer (no role chain, not ducked) is measured in its role
//! stem, before ducking and loudness, against the same layer without keys.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{read_audio, rms_db};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

fn project(d: &Path, name: &str, layer: &str) -> PathBuf {
    let p = d.join(format!("{name}.json5"));
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: '{name}', target: 'custom', width: 320, height: 180, fps: 30,
              assets: {{bg: {{path: 'bg.png'}}, tone: {{path: 'tone.wav'}}}},
              main: [{{id: 'm', clip: 'bg', duration: 6}}],
              layers: [{{type: 'audio', asset: 'tone', role: 'sfx', {layer}}}],
              audio: {{voice_chain: 'raw'}}}}"
        ),
    )
    .unwrap();
    p
}

/// Renders and returns the sfx stem (before ducking and loudness) and the stems job's filter.
fn stem(p: &Path) -> (PathBuf, String) {
    let l = pipeline::load(p).unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let (plan, _) = pipeline::render(&l, &pr, &RenderOptions { jobs: 3, ..Default::default() }).unwrap();
    let filter = std::fs::read_to_string(plan.work_dir.join("audio_stems.filter"))
        .unwrap_or_else(|_| format!("{:?}", plan.audio.stems_job));
    // renders in one process share a work folder: keep this render's stem
    let kept = p.with_extension("sfx.wav");
    std::fs::copy(&plan.audio.stems["sfx"], &kept).unwrap();
    (kept, filter)
}

fn level_at(tc: &cutline_deps::Toolchain, wav: &Path, t: f64) -> f64 {
    rms_db(&read_audio(tc, wav, t - 0.1, Some(0.2), 1).unwrap())
}

#[test]
fn gain_keys_shape_the_layer_level() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("bg.png"), 320, 180, "0x223344").unwrap();
    gen::tone(tc, &d.path().join("tone.wav"), 440.0, 6.0, -12.0).unwrap();

    for start in [0.0, 1.5] {
        let (control, plain_filter) = stem(&project(
            d.path(),
            &format!("control{}", start as i32),
            &format!("start: {start}, end: {}", start + 4.0),
        ));
        let (keyed, _) = stem(&project(
            d.path(),
            &format!("keyed{}", start as i32),
            &format!("start: {start}, end: {}, gain_keys: [{{at: 1, db: 0}}, {{at: 3, db: -12}}]", start + 4.0),
        ));
        assert!(!plain_filter.contains("asetnsamples"), "no keys, no change to the graph");
        // layer-local times 0.5, 2.0 and 3.5 s: 0, −6 and −12 dB relative to the control
        for (local, want) in [(0.5, 0.0), (2.0, -6.0), (3.5, -12.0)] {
            let t = start + local;
            let diff = level_at(tc, &keyed, t) - level_at(tc, &control, t);
            assert!((diff - want).abs() <= 0.5, "start {start}, at {local}s: {diff:.2} dB, want {want}");
        }
    }
}

#[test]
fn a_single_key_is_a_constant_offset() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("bg.png"), 320, 180, "0x223344").unwrap();
    gen::tone(tc, &d.path().join("tone.wav"), 440.0, 6.0, -12.0).unwrap();
    let (control, _) = stem(&project(d.path(), "c", "start: 0, end: 4"));
    let (keyed, _) = stem(&project(d.path(), "k", "start: 0, end: 4, gain_keys: [{at: 2, db: -6}]"));
    for t in [0.5, 2.0, 3.5] {
        let diff = level_at(tc, &keyed, t) - level_at(tc, &control, t);
        assert!((diff + 6.0).abs() <= 0.5, "at {t}: {diff:.2}");
    }
}
