//! M6 acceptance: face tracking on a panned face fixture (tracking error ≤ 8 % of width after
//! settling, ≤ 2 direction changes per second, pan speed ≤ max), fit_blur fallback + warning.
//! Runs with an injected mock detector; with `--features onnx` and the model installed the
//! real YuNet detector is exercised too.

use cutline_reframe::{Detector, Face, FnDetector};
use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static SERIAL: Mutex<()> = Mutex::new(());

const DUR: f64 = 8.0;
const FACE_W: f64 = 360.0;
const X0: f64 = 150.0;
const X1: f64 = 1400.0;
/// Face center inside the fixture image (normalized), measured with YuNet.
const FACE_CX: f64 = 0.615;
const FACE_CY: f64 = 0.274;

fn truth_x(t: f64) -> f64 {
    (X0 + (X1 - X0) * t / DUR + FACE_CX * FACE_W) / 1920.0
}

fn panned_face(dir: &Path) -> PathBuf {
    let tc = test_toolchain().unwrap();
    let out = dir.join("pan.mp4");
    let face = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/face_armstrong.jpg");
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &format!("color=c=0x505860:s=1920x1080:r=30:d={DUR}"),
        "-loop",
        "1",
        "-framerate",
        "30",
        "-t",
        &DUR.to_string(),
        "-i",
    ])
    .arg(&face);
    c.args(["-f", "lavfi", "-i", &format!("sine=f=200:d={DUR}")]);
    c.args([
        "-filter_complex",
        &format!("[1:v]scale={FACE_W}:-2[f];[0:v][f]overlay=x='{X0}+({X1}-{X0})*t/{DUR}':y=180:shortest=1,scale=out_color_matrix=bt709:out_range=tv,format=yuv420p[v]"),
        "-map",
        "[v]",
        "-map",
        "2:a",
        "-colorspace",
        "bt709",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "16",
        "-c:a",
        "aac",
    ])
    .arg(&out);
    gen::run(c).unwrap();
    out
}

fn project(dir: &Path, asset: &str) -> PathBuf {
    let p = dir.join("p.json5");
    std::fs::write(&p, format!("{{version: 1, name: 'rf', target: 'shorts', assets: {{v: {{path: '{asset}'}}}}, main: [{{id: 'c', clip: 'v', reframe: {{mode: 'track_face', max_pan_speed: 0.5}}}}]}}")).unwrap();
    p
}

fn check_path(path: &cutline_reframe::CropPath, max_speed: f64) {
    let fps = path.fps;
    let n = path.len();
    let settle = (1.0 * fps) as usize;
    let mut worst = 0.0f64;
    for i in settle..n {
        let t = i as f64 / fps;
        worst = worst.max((path.center_x(i) - truth_x(t)).abs());
    }
    assert!(worst <= 0.08, "tracking error {worst:.3} of width");
    // direction changes per second while the subject moves monotonically
    let dx: Vec<i64> = path.x.windows(2).map(|w| w[1] as i64 - w[0] as i64).filter(|d| *d != 0).collect();
    let flips = dx.windows(2).filter(|w| w[0].signum() != w[1].signum()).count();
    let per_s = flips as f64 / (n as f64 / fps);
    assert!(per_s <= 2.0, "{flips} direction changes ({per_s:.2}/s)");
    let maxv =
        path.x.windows(2).map(|w| (w[1] as f64 - w[0] as f64).abs() * fps / path.src_w as f64).fold(0.0, f64::max);
    assert!(maxv <= max_speed + 2.0 * fps / path.src_w as f64, "pan speed {maxv:.3} widths/s > {max_speed}");
}

#[test]
fn tracks_panned_face_with_mock_detector() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(_tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    panned_face(d.path());
    cutline_reframe::set_detector_factory(Some(Box::new(|| {
        Box::new(FnDetector::new("mock-truth", |i, _rgb, _w, _h| {
            let t = i as f64 / 6.0;
            let cx = truth_x(t);
            vec![Face { x: cx - 0.04, y: FACE_CY * 0.5, w: 0.08, h: 0.14, score: 0.95 }]
        })) as Box<dyn Detector>
    })));
    let pp = project(d.path(), "pan.mp4");
    let l = pipeline::load(&pp).unwrap();
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    cutline_reframe::set_detector_factory(None);
    let path = pr.extras.crop_paths.get("c").expect("crop path").clone();
    check_path(&path, 0.5);
    // the pan moves in fractions of a source pixel: during the steady part of the move the
    // window advances by nearly the same amount every frame (whole-pixel crops alternate steps)
    let steps: Vec<f64> = (100..200).map(|i| path.x[i + 1] - path.x[i]).collect();
    let mean = steps.iter().sum::<f64>() / steps.len() as f64;
    assert!(mean > 1.0, "the window pans ({mean:.2} px/frame)");
    assert!(steps.iter().all(|s| (s - mean).abs() < 0.25), "uneven pan steps: {steps:.2?}");
    // renders, and the segment resamples the moving window sub-pixel
    let (plan, _) = pipeline::render(&l, &pr, &RenderOptions::default()).unwrap();
    assert!(plan.segments[0].spec.filter.contains("perspective="), "{}", plan.segments[0].spec.filter);
    assert!(plan.output.exists());
}

#[test]
fn falls_back_to_fit_blur_without_faces() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=1280x720:r=30:d=2",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
    ])
    .arg(d.path().join("nf.mp4"));
    gen::run(c).unwrap();
    cutline_reframe::set_detector_factory(Some(Box::new(|| {
        Box::new(FnDetector::new("mock-none", |_, _, _, _| vec![])) as Box<dyn Detector>
    })));
    let pp = project(d.path(), "nf.mp4");
    let l = pipeline::load(&pp).unwrap();
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    cutline_reframe::set_detector_factory(None);
    assert!(pr.diags.iter().any(|x| x.code == "W_REFRAME_NO_FACES"), "{:?}", pr.diags);
    let plan = pipeline::plan_for(&l, &pr, &RenderOptions::default()).unwrap();
    assert!(plan.segments[0].spec.filter.contains("boxblur"), "fit_blur expected: {}", plan.segments[0].spec.filter);
}

#[cfg(feature = "onnx")]
#[test]
fn tracks_panned_face_with_yunet() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(tc) = test_toolchain() else { return };
    if !cutline_reframe::model_path().exists() && std::env::var_os("CUTLINE_FACE_MODEL").is_none() {
        eprintln!("SKIP: face model not installed");
        return;
    }
    cutline_reframe::set_detector_factory(None);
    let d = tempfile::tempdir().unwrap();
    panned_face(d.path());
    let pp = project(d.path(), "pan.mp4");
    let l = pipeline::load(&pp).unwrap();
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let path = pr.extras.crop_paths.get("c").expect("faces found").clone();
    check_path(&path, 0.5);
    // the face is near the horizontal center of the rendered 9:16 frames
    let (plan, _) = pipeline::render(&l, &pr, &RenderOptions::default()).unwrap();
    let mut det = cutline_reframe::default_detector().unwrap();
    let frames = cutline_testkit::analyze::read_frames(tc, &plan.output, &[60, 120, 180, 220]).unwrap();
    for f in frames {
        let faces = det.detect(f.as_raw(), f.width(), f.height()).unwrap();
        assert!(!faces.is_empty());
        assert!((faces[0].cx() - 0.5).abs() < 0.15, "face at {:.2} of output width", faces[0].cx());
    }
}

/// A still image clip with `track_face`: the analysis must loop the single frame (ffmpeg
/// yields no frames for `-ss`/`-t` on an image input), so faces are found and no fallback
/// warning is emitted.
#[test]
fn tracks_face_on_still_image() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(_tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let face = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/face_armstrong.jpg");
    std::fs::copy(&face, d.path().join("face.jpg")).unwrap();
    let frames_seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fs2 = frames_seen.clone();
    cutline_reframe::set_detector_factory(Some(Box::new(move || {
        let fs3 = fs2.clone();
        Box::new(FnDetector::new("mock-still", move |_, _rgb, _w, _h| {
            fs3.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            vec![Face { x: FACE_CX - 0.15, y: FACE_CY - 0.1, w: 0.3, h: 0.2, score: 0.95 }]
        })) as Box<dyn Detector>
    })));
    let p = d.path().join("p.json5");
    std::fs::write(&p, "{version: 1, name: 'st', target: 'shorts', assets: {img: {path: 'face.jpg'}}, main: [{id: 'c', clip: 'img', duration: 2, reframe: {mode: 'track_face'}}]}").unwrap();
    let l = pipeline::load(&p).unwrap();
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    cutline_reframe::set_detector_factory(None);
    assert!(frames_seen.load(std::sync::atomic::Ordering::SeqCst) >= 6, "analysis decoded no frames");
    assert!(!pr.diags.iter().any(|x| x.code == "W_REFRAME_NO_FACES"), "{:?}", pr.diags);
    assert!(pr.extras.crop_paths.contains_key("c"), "no crop path for the still");
}

/// Centroid and pixel count of the red marker in a frame.
fn red_marker(f: &image::RgbImage) -> Option<(f64, f64, f64)> {
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
    for (x, y, p) in f.enumerate_pixels() {
        if p[0] > 160 && p[1] < 90 && p[2] < 90 {
            sx += x as f64;
            sy += y as f64;
            n += 1.0;
        }
    }
    (n > 0.0).then(|| (sx / n, sy / n, n))
}

/// Manual keys frame the crop on their (x, y) center, not the frame center (the post-crop
/// zoom used to be centered whatever the keys said). Same-aspect source, so the zoom does all
/// the framing. A red marker sits at source (0.3, 0.3):
/// - key 1 centers on it at zoom 2 → the marker is at the output center, 2× its size;
/// - key 2 asks for (0.1, 0.1) at zoom 2 → the view clamps to the top-left half [0, 0.5], so
///   the marker lands at 0.6 of the output on both axes;
/// - in between the marker moves monotonically (eased interpolation, no jumps).
#[test]
fn manual_keys_frame_on_key_center() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let (w, h) = (1280u32, 720u32);
    let (mx, my) = (384u32, 216u32);
    let g = format!(
        "color=c=0x808080:s={w}x{h}:r=30:d=2,drawbox=x={}:y={}:w=24:h=24:color=0xff0000:t=fill,format=yuv420p",
        mx - 12,
        my - 12
    );
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &g, "-c:v", "libx264", "-crf", "8", "-preset", "veryfast"])
        .arg(d.path().join("m.mp4"));
    gen::run(c).unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        format!(
            "{{version: 1, name: 'mk', target: 'custom', width: {w}, height: {h}, fps: 30, assets: {{m: {{path: 'm.mp4'}}}}, main: [{{id: 'c', clip: 'm', in: 0, out: 1.8, reframe: {{mode: 'manual', keys: [{{t: 0, x: 0.3, y: 0.3, zoom: 2}}, {{t: 0.4, x: 0.3, y: 0.3, zoom: 2}}, {{t: 1.2, x: 0.1, y: 0.1, zoom: 2}}]}}}}], audio: {{voice_chain: 'raw'}}}}"
        ),
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let (plan, _) = pipeline::render(&l, &pr, &RenderOptions::default()).unwrap();
    let frames = cutline_testkit::analyze::read_all_frames(tc, &plan.output, None).unwrap();
    let a_src = 24.0 * 24.0;
    let mut prev = (0.0f64, 0.0f64);
    for (n, f) in frames.iter().enumerate() {
        let t = n as f64 / 30.0;
        let (cx, cy, a) = red_marker(f).unwrap_or_else(|| panic!("marker left the frame at {n}"));
        let want = if t <= 0.4 {
            Some((640.0, 360.0))
        } else if t >= 1.2 {
            Some((0.6 * w as f64, 0.6 * h as f64))
        } else {
            None
        };
        if let Some((wx, wy)) = want {
            assert!(
                (cx - wx).abs() < 3.0 && (cy - wy).abs() < 3.0,
                "frame {n} (t={t:.2}): marker at ({cx:.1}, {cy:.1}), want ({wx}, {wy})"
            );
            let grown = (a / a_src).sqrt();
            assert!((grown - 2.0).abs() < 0.15, "frame {n}: marker grew {grown:.2}×, want 2×");
        }
        if n > 0 {
            assert!(cx >= prev.0 - 1.0 && cy >= prev.1 - 1.0, "frame {n}: marker moved backwards");
        }
        prev = (cx, cy);
    }
}
