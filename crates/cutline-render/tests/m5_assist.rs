//! M5 acceptance: jump cuts at expected points (±1 frame) with no long silences left; filler
//! removal from a transcript; caption style goldens; mapped captions drop removed words.

use cutline_core::model::MainItem;
use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{golden_compare, read_frames};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

fn video_with_audio(tc: &cutline_deps::Toolchain, out: &Path, audio: &Path, dur: f64) {
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &format!("testsrc2=s=640x360:r=30:d={dur}")]).arg("-i").arg(audio);
    c.args([
        "-vf",
        "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
    ]);
    c.args(["-c:v", "libx264", "-preset", "ultrafast", "-c:a", "pcm_s16le", "-shortest"]).arg(out);
    gen::run(c).unwrap();
}

fn clips(p: &cutline_core::Project) -> Vec<(String, f64, f64, Option<String>)> {
    p.main
        .iter()
        .filter_map(|m| match m {
            MainItem::Clip(c) => Some((
                c.id.clone(),
                c.in_.unwrap().seconds(cutline_core::time::Fps::new(30, 1), 0.0),
                c.out.unwrap().seconds(cutline_core::time::Fps::new(30, 1), 0.0),
                c.generated_by.clone(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn jump_cuts_cut_at_silences() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let speech = [(0.0, 1.0), (1.8, 3.0), (4.0, 4.6), (5.5, 7.0)];
    gen::bursts(tc, &d.path().join("a.wav"), 8.0, &speech, 440.0).unwrap();
    video_with_audio(tc, &d.path().join("v.mp4"), &d.path().join("a.wav"), 8.0);
    let (min_sil, pad) = (0.35, 0.08);
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        format!("{{version: 1, name: 'jc', target: 'long_1080p', fps: 30, assets: {{v: {{path: 'v.mp4'}}}}, main: [{{id: 'talk', clip: 'v'}}], audio: {{voice_chain: 'raw'}}, auto: {{jump_cuts: {{min_silence: {min_sil}, keep_padding: {pad}}}}}}}"),
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    let ex = cutline_render::extras::expand_project(&l).unwrap();
    let got = clips(&ex);
    let want = [(0.0, 1.0 + pad), (1.8 - pad, 3.0 + pad), (4.0 - pad, 4.6 + pad), (5.5 - pad, 7.0 + pad)];
    assert_eq!(got.len(), want.len(), "{got:?}");
    let frame = 1.0 / 30.0;
    for ((id, a, b, g), (wa, wb)) in got.iter().zip(want) {
        assert!((a - wa).abs() <= frame && (b - wb).abs() <= frame, "{id}: {a:.3}-{b:.3} want {wa:.3}-{wb:.3}");
        assert_eq!(g.as_deref(), Some("jump_cuts"));
    }
    assert_eq!(got[0].0, "talk", "first piece keeps the original id (anchors survive)");
    // render and verify no silence longer than min_silence remains
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let (plan, _) = pipeline::render(&l, &pr, &RenderOptions::default()).unwrap();
    let o = std::process::Command::new(&tc.ffmpeg)
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(&plan.output)
        .args(["-af", &format!("silencedetect=n=-35dB:d={min_sil}"), "-f", "null", "-"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&o.stderr);
    assert!(
        !log.contains("silence_start"),
        "silence remains: {}",
        log.lines().filter(|l| l.contains("silence")).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn fillers_are_removed_and_captions_follow() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "sine=f=300:d=6", "-ac", "2"]).arg(d.path().join("a.wav"));
    gen::run(c).unwrap();
    video_with_audio(tc, &d.path().join("v.mp4"), &d.path().join("a.wav"), 6.0);
    // cutline transcript with fillers at 1.0–1.4 ("um") and 3.2–3.5 ("uh,")
    let words = [
        ("So", 0.2, 0.5),
        ("today", 0.55, 0.95),
        ("um", 1.0, 1.4),
        ("we", 1.45, 1.6),
        ("sharpen", 1.65, 2.2),
        ("knives.", 2.25, 2.9),
        ("And", 3.0, 3.15),
        ("uh,", 3.2, 3.5),
        ("hone", 3.55, 3.9),
        ("them.", 3.95, 4.4),
    ];
    let t = serde_json::json!({"asset": "v", "lang": "en", "model": "fixture", "words": words.iter().map(|(w, a, b)| serde_json::json!({"w": w, "start": a, "end": b})).collect::<Vec<_>>()});
    std::fs::write(d.path().join("v.transcript.json"), serde_json::to_string(&t).unwrap()).unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, name: 'fill', target: 'long_1080p', fps: 30, assets: {v: {path: 'v.mp4', transcript: 'v.transcript.json'}}, main: [{id: 'talk', clip: 'v', in: 0, out: 5}], captions: {style: 'clean_sentence'}, auto: {remove_fillers: {}}}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    let ex = cutline_render::extras::expand_project(&l).unwrap();
    let got = clips(&ex);
    assert_eq!(got.len(), 3, "{got:?}");
    // cut ranges cover the filler words but not their neighbours
    assert!(got[0].2 <= 1.0 && got[0].2 >= 0.95, "{got:?}");
    assert!(got[1].1 >= 1.4 && got[1].1 <= 1.45, "{got:?}");
    assert!(got[1].2 <= 3.2 && got[1].2 >= 3.15, "{got:?}");
    assert!(got[2].1 >= 3.5 && got[2].1 <= 3.55, "{got:?}");
    assert!(got.iter().all(|c| c.3.as_deref() == Some("remove_fillers")));
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let srt = &pr.extras.srt;
    assert!(srt.contains("So today we sharpen knives."), "{srt}");
    assert!(!srt.to_lowercase().contains(" um") && !srt.to_lowercase().contains("uh,"), "{srt}");
    assert!(!pr.extras.speech.is_empty(), "transcript speech spans feed ducking");
}

#[test]
fn caption_style_goldens() {
    let Some(tc) = test_toolchain() else { return };
    let major = cutline_deps::tools::parse_version(&tc.version).0;
    let gold = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/golden/captions/ffmpeg{major}"));
    if !gold.exists() && std::env::var("CUTLINE_BLESS_MISSING").is_err() && std::env::var("CUTLINE_BLESS").is_err() {
        eprintln!("SKIP: no caption goldens for ffmpeg {major}");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        "color=c=0x30405a:s=1080x1920:r=30:d=4",
        "-f",
        "lavfi",
        "-i",
        "sine=f=300:d=4",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
    ])
    .arg(d.path().join("v.mp4"));
    gen::run(c).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../cutline-transcribe/tests/fixtures/scribe_sample.json"),
        d.path().join("vo.scribe.json"),
    )
    .unwrap();
    let mut failures = vec![];
    for style in cutline_core::catalog::CAPTION_STYLES {
        let pp = d.path().join(format!("{style}.json5"));
        std::fs::write(&pp, format!("{{version: 1, name: '{style}', target: 'shorts', assets: {{v: {{path: 'v.mp4', transcript: 'vo.scribe.json'}}}}, main: [{{id: 'c', clip: 'v'}}], captions: {{style: '{style}', emphasis: ['this'], emoji: {{dull: 'knife'}}}}}}")).unwrap();
        let l = pipeline::load(&pp).unwrap();
        let (name, e) = pipeline::exports(&l.project).remove(0);
        let pr = pipeline::prepare(&l, name, &e, false).unwrap();
        // safe zone check from layout
        let (sx, sy, sw, sh) = pr.timeline.target.safe_rect();
        assert!(
            pr.extras.text_boxes.iter().all(|b| b.x >= sx - 1.0
                && b.x + b.w <= sx + sw + 1.0
                && b.y >= sy - 1.0
                && b.y + b.h <= sy + sh + 1.0),
            "{style}: caption outside safe zone"
        );
        let (plan, _) = pipeline::render(&l, &pr, &RenderOptions::default()).unwrap();
        // 1.0 s ("dull" current) and 1.9 s ("this." emphasis)
        let frames = read_frames(tc, &plan.output, &[30, 57]).unwrap();
        for (k, img) in frames.iter().enumerate() {
            let small =
                image::imageops::resize(img, img.width() / 2, img.height() / 2, image::imageops::FilterType::Triangle);
            if let Err(e) = golden_compare(&small, &gold.join(format!("{style}_{k}.png")), 0.97) {
                failures.push(format!("{style}/{k}: {e}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
