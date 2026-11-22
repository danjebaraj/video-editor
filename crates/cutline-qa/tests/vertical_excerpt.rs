//! FEAT-magic-alpaca-JPD6: a vertical excerpt with a moving subject, timed captions, voice and
//! music, made with existing controls (manual reframe keys, transcript captions, music under the
//! voice). The crop keeps the subject at the caption times, the caption text is the fixture's,
//! and loudness and true peak pass QA.

use cutline_qa::deliver::render_all;
use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::read_frames;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;

const WORDS: &[(&str, f64, f64)] = &[
    ("Follow", 0.5, 0.9),
    ("the", 0.9, 1.1),
    ("red", 1.1, 1.5),
    ("square", 1.5, 2.1),
    ("across", 3.0, 3.5),
    ("the", 3.5, 3.7),
    ("frame.", 3.7, 4.4),
];

#[test]
fn vertical_excerpt_keeps_the_subject_and_passes_audio_qa() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // a 640×360 shot: a red 40 px square moving left to right over 6 s
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        "color=c=0x334455:s=640x360:r=30:d=6",
        "-f",
        "lavfi",
        "-i",
        "color=c=red:s=40x40:r=30:d=6",
    ])
    .args(["-filter_complex", "[0][1]overlay=x='128+384*t/6':y=160", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
    .arg(d.path().join("shot.mp4"));
    gen::run(c).unwrap();
    gen::tone(tc, &d.path().join("vo.wav"), 220.0, 5.0, -16.0).unwrap();
    gen::noise_bed(tc, &d.path().join("music.wav"), 6.0, 0.1).unwrap();
    let words: Vec<_> = WORDS.iter().map(|(w, s, e)| serde_json::json!({"w": w, "start": s, "end": e})).collect();
    std::fs::write(d.path().join("vo.json"), serde_json::json!({"asset": "vo", "words": words}).to_string()).unwrap();
    // the square's centre is at x = (148 + 384 t / 6) / 640: one key a second follows it
    let keys: Vec<String> =
        (0..=6).map(|t| format!("{{t: {t}, x: {:.4}, y: 0.5, zoom: 1}}", (148.0 + 64.0 * t as f64) / 640.0)).collect();
    let pp = d.path().join("v.json5");
    std::fs::write(
        &pp,
        format!(
            "{{version: 1, name: 'v', target: 'custom', width: 360, height: 640, fps: 30, brand_bumpers: false, watermark: false,
              assets: {{shot: {{path: 'shot.mp4'}}, vo: {{path: 'vo.wav', role: 'voice', transcript: 'vo.json'}}, music: {{path: 'music.wav', role: 'music'}}}},
              main: [{{id: 's', clip: 'shot', audio: 'mute', reframe: {{mode: 'manual', keys: [{}]}}}}],
              layers: [{{type: 'audio', asset: 'vo', role: 'voice', start: 0}}, {{type: 'audio', asset: 'music', role: 'music', start: 0}}],
              captions: {{style: 'clean_sentence'}},
              audio: {{music_level: 18}}}}",
            keys.join(", ")
        ),
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let r = render_all(&l, &RenderOptions { jobs: 3, ..Default::default() }, &[]).unwrap().remove(0);

    // captions: the fixture's words
    let srt = std::fs::read_to_string(&r.srt_path).unwrap();
    assert!(srt.contains("Follow the red square") && srt.contains("across the frame."), "{srt}");
    // the subject is in the crop at each caption's middle
    let mids = [1.3, 3.7];
    let frames = read_frames(tc, &r.output, &mids.map(|t| (t * 30.0) as u64)).unwrap();
    for (t, f) in mids.iter().zip(&frames) {
        let red = f.pixels().filter(|p| p[0] > 180 && p[1] < 80 && p[2] < 80).count();
        assert!(red > 400, "the square is in the crop at {t}s ({red} red pixels)");
    }
    // loudness and true peak checks pass
    // loudness and true peak (one finding) pass
    let loud = r.qa.findings.iter().find(|f| f.check == "loudness").unwrap();
    assert!(loud.ok, "{loud:?}");
    eprintln!("srt:\n{srt}\nloudness: {:?}", r.qa.findings.iter().find(|f| f.check == "loudness").map(|f| &f.message));
}
