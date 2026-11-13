//! M0 acceptance: probe on testkit media, frame-ID round-trip through x264 + scale.

use cutline_testkit::analyze::read_all_frames;
use cutline_testkit::gen::{self, AlphaKind, FrameIdSource};
use cutline_testkit::{decode_frame_id, test_toolchain, FrameIdLayout};

#[test]
fn probe_alpha_fixtures() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let pr = d.path().join("a.mov");
    gen::alpha_overlay(tc, &pr, AlphaKind::ProRes4444, 320, 240, 30, 2.0, 192).unwrap();
    let i = cutline_media::probe(tc, &pr, None).unwrap();
    let v = i.video.unwrap();
    assert!(v.alpha, "prores 4444 alpha");
    assert_eq!((v.width, v.height), (320, 240));
    assert!((v.fps_f64 - 30.0).abs() < 1e-6);
    assert!((i.duration - 2.0).abs() < 0.05, "{}", i.duration);

    let vp = d.path().join("a.webm");
    gen::alpha_overlay(tc, &vp, AlphaKind::Vp9Alpha, 320, 240, 25, 2.0, 192).unwrap();
    let i = cutline_media::probe(tc, &vp, None).unwrap();
    let v = i.video.unwrap();
    assert!(v.alpha && v.needs_libvpx_decoder, "vp9 alpha {v:?}");
    assert!((v.fps_f64 - 25.0).abs() < 1e-6);
    assert!((i.duration - 2.0).abs() < 0.05, "{}", i.duration);

    let seq = d.path().join("seq/%05d.png");
    gen::alpha_overlay(tc, &seq, AlphaKind::PngSequence, 160, 120, 30, 1.0, 192).unwrap();
    let i = cutline_media::probe(tc, &seq, Some(30.0)).unwrap();
    assert_eq!(i.kind, "image_sequence");
    assert!(i.video.unwrap().alpha);
}

#[test]
fn probe_frameid_video() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("src.mp4");
    gen::frameid_video(tc, &p, &FrameIdSource::new(1280, 720, 30, 3.0)).unwrap();
    let i = cutline_media::probe(tc, &p, None).unwrap();
    let v = i.video.as_ref().unwrap();
    assert_eq!((v.width, v.height), (1280, 720));
    assert!((v.fps_f64 - 30.0).abs() < 1e-6);
    assert!(!v.alpha && !v.vfr);
    assert!((i.duration - 3.0).abs() < 0.1);
    assert_eq!(i.audio.unwrap().sample_rate, 48000);
}

/// 300 frames, x264 CRF 23 then scale to 1080x1920: 100% decode accuracy.
#[test]
fn frameid_survives_x264_and_scale() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src.mp4");
    let mut s = FrameIdSource::new(1280, 720, 30, 10.0);
    s.tone_hz = None;
    gen::frameid_video(tc, &src, &s).unwrap();
    let out = d.path().join("out.mp4");
    let mut c = tc.ffmpeg_cmd();
    c.arg("-i")
        .arg(&src)
        .args([
            "-vf",
            "scale=1080:1920,setsar=1",
            "-c:v",
            "libx264",
            "-crf",
            "23",
            "-preset",
            "veryfast",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&out);
    gen::run(c).unwrap();
    // stretched scale: x * 1080/1280, y * 1920/720
    let l = FrameIdLayout::top_row().transform(1080.0 / 1280.0, 1920.0 / 720.0, 0.0, 0.0);
    let (_, _, bw, bh) = l.bbox();
    let frames = read_all_frames(tc, &out, Some((0, 0, bw.ceil() as u32, bh.ceil() as u32))).unwrap();
    assert_eq!(frames.len(), 300);
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(decode_frame_id(f, &l), Some(i as u16), "frame {i}");
    }
}

#[test]
#[ignore = "debug helper"]
fn dbg_counts() {
    let tc = test_toolchain().unwrap();
    let p = std::path::Path::new("/tmp/dbg_src.mp4");
    let mut s = FrameIdSource::new(1280, 720, 30, 10.0);
    s.tone_hz = None;
    gen::frameid_video(tc, p, &s).unwrap();
}
