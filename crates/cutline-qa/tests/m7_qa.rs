//! M7 acceptance: fault injection (each broken output triggers the expected QA finding) and
//! review-pack image limits.

use cutline_core::model::TargetName;
use cutline_core::target::resolve;
use cutline_qa::qa::{run_qa, CaptionBox, QaContext, QaReport};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

fn encode(tc: &cutline_deps::Toolchain, out: &Path, vf_extra: &str, fps: u32, af: &str, faststart: bool) {
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc2=s=1080x1920:r={fps}:d=6"),
        "-f",
        "lavfi",
        "-i",
        "anoisesrc=color=pink:amplitude=0.25:d=6:seed=3",
    ]);
    c.args([
        "-vf",
        &format!("{vf_extra}scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv"),
        "-af",
        &format!("{af}aformat=channel_layouts=stereo"),
    ]);
    c.args(["-c:v", "libx264", "-preset", "veryfast", "-profile:v", "high", "-c:a", "aac", "-ar", "48000"]);
    c.args([
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
        "-shortest",
    ]);
    if faststart {
        c.args(["-movflags", "+faststart"]);
    }
    c.arg(out);
    gen::run(c).unwrap();
}

fn ctx() -> QaContext {
    QaContext {
        expected_duration: Some(6.0),
        loudness_target: Some((-14.0, -1.0)),
        is_shorts: true,
        max_cps: 20.0,
        first_text_at: Some(0.2),
        ..Default::default()
    }
}

fn finding<'a>(r: &'a QaReport, check: &str) -> &'a cutline_qa::qa::Finding {
    r.findings.iter().find(|f| f.check == check).unwrap_or_else(|| panic!("no finding {check}: {r:?}"))
}

/// Loudness-normalized pink noise filter (two passes are overkill for the fixture).
const NORM: &str = "loudnorm=I=-14:TP=-1.5:LRA=7,aresample=48000,";

#[test]
fn fault_injection() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let t = resolve(TargetName::Shorts, None, None, None, None);
    let src = "";

    // baseline passes
    let good = d.path().join("good.mp4");
    encode(tc, &good, src, 30, NORM, true);
    let r = run_qa(tc, &good, &t, &ctx()).unwrap();
    assert!(r.ok, "baseline should pass: {:#?}", r.findings.iter().filter(|f| !f.ok).collect::<Vec<_>>());
    for chk in ["container_codec", "geometry_fps", "loudness", "black_frames"] {
        assert!(finding(&r, chk).ok, "baseline {chk}");
    }

    // wrong fps
    let p = d.path().join("fps.mp4");
    encode(tc, &p, src, 25, NORM, true);
    let r = run_qa(tc, &p, &t, &ctx()).unwrap();
    assert!(!finding(&r, "geometry_fps").ok && !r.ok);

    // loud audio
    let p = d.path().join("loud.mp4");
    encode(tc, &p, src, 30, "volume=18dB,alimiter=limit=0.99,", true);
    let r = run_qa(tc, &p, &t, &ctx()).unwrap();
    let f = finding(&r, "loudness");
    assert!(!f.ok, "{f:?}");
    assert!(!r.ok);

    // 2 s black gap in the middle
    let p = d.path().join("black.mp4");
    encode(tc, &p, "drawbox=enable='between(t,2,4)':color=black:t=fill,", 30, NORM, true);
    let r = run_qa(tc, &p, &t, &ctx()).unwrap();
    let f = finding(&r, "black_frames");
    assert!(!f.ok, "{f:?}");
    let spans = f.measured.as_array().unwrap();
    let (a, b) = (spans[0][0].as_f64().unwrap(), spans[0][1].as_f64().unwrap());
    assert!((a - 2.0).abs() < 0.1 && (b - 4.0).abs() < 0.1, "{spans:?}");
    // …but not when the black is intended (a fade)
    let mut c2 = ctx();
    c2.intended_black.push((1.9, 4.1));
    assert!(finding(&run_qa(tc, &p, &t, &c2).unwrap(), "black_frames").ok);

    // missing faststart
    let p = d.path().join("nofs.mp4");
    encode(tc, &p, src, 30, NORM, false);
    let r = run_qa(tc, &p, &t, &ctx()).unwrap();
    assert!(!finding(&r, "container_codec").ok && !r.ok);

    // captions outside the safe zone + too fast
    let mut c3 = ctx();
    c3.caption_boxes = vec![
        CaptionBox {
            start: 1.0,
            end: 2.0,
            x: 100.0,
            y: 1700.0,
            w: 600.0,
            h: 120.0,
            text: "IN THE UI ZONE".into(),
            kind: "caption".into(),
        },
        CaptionBox {
            start: 2.0,
            end: 2.4,
            x: 200.0,
            y: 900.0,
            w: 600.0,
            h: 120.0,
            text: "this is far too much text to read".into(),
            kind: "caption".into(),
        },
    ];
    let r = run_qa(tc, &good, &t, &c3).unwrap();
    assert!(!finding(&r, "captions_safe_zone").ok);
    assert!(!finding(&r, "captions_cps").ok);
    assert!(r.ok, "caption findings are warnings");

    // no hook within 1 s on Shorts
    let mut c4 = ctx();
    c4.first_text_at = Some(2.5);
    assert!(!finding(&run_qa(tc, &good, &t, &c4).unwrap(), "hook").ok);
}

#[test]
fn review_pack_respects_limits() {
    let Some(_tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    cutline_qa::fixtures::sources(d.path()).unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(&pp, cutline_qa::suite::SHORTS_PROJECT).unwrap();
    let l = cutline_render::pipeline::load(&pp).unwrap();
    let out = d.path().join("review");
    let r = cutline_qa::review::review(&l, &out).unwrap();
    assert!(r.images.len() <= cutline_qa::review::MAX_IMAGES);
    assert!(r.images.iter().any(|p| p.ends_with("contact_sheet.jpg")));
    assert!(r.images.iter().any(|p| p.ends_with("audio.png")));
    assert!(r.images.iter().any(|p| p.to_string_lossy().contains("/cuts/")));
    assert!(r.images.iter().any(|p| p.to_string_lossy().contains("/captions/")));
    for p in &r.images {
        let (w, h) = image::image_dimensions(p).unwrap();
        assert!(w.max(h) <= cutline_qa::review::MAX_SIDE, "{} is {w}x{h}", p.display());
    }
    for f in ["timeline.txt", "summary.md", "qa.json", "preview_540p.mp4"] {
        assert!(out.join(f).exists(), "{f}");
    }
    let tl = std::fs::read_to_string(out.join("timeline.txt")).unwrap();
    assert!(tl.contains("hook") && tl.contains("CAPTIONS"), "{tl}");
}
