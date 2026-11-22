//! Review on sparse, variable-rate screen recordings (TASK-silver-cherry-R3MZ,
//! TASK-honest-cedar-A73E). A recorder that writes a frame only when the screen changes leaves
//! long gaps: a seek can land after the last stored frame while still inside the file, where
//! ffmpeg writes nothing and prints nothing ("frame grab at 11.433s failed: "), and the first frame
//! after a seek is the next change, not the frame on screen.
//!
//! The fixtures are generated here: a CFR source with four solid colors (red from 0 s, green from
//! 4 s, blue from 8 s, white from 11.9 s), of which only a few frames are kept with their original
//! timestamps (`select` + `-fps_mode passthrough`, no B-frames, like a screen recorder).

use cutline_qa::review::{grab, grab_held, grab_source};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

/// Writes a sparse recording of `dur` seconds keeping the 30 fps source frames `keep`.
fn sparse(tc: &cutline_deps::Toolchain, out: &Path, w: u32, h: u32, dur: f64, keep: &[u32]) {
    let sel: Vec<String> = keep.iter().map(|n| format!("eq(n,{n})")).collect();
    let graph = format!(
        "color=c=red:s={w}x{h}:r=30:d={dur}[a];color=c=green:s={w}x{h}:r=30:d={dur}[g];\
         color=c=blue:s={w}x{h}:r=30:d={dur}[b];color=c=white:s={w}x{h}:r=30:d={dur}[x];\
         [a][g]overlay=enable='gte(t,4)'[p];[p][b]overlay=enable='gte(t,8)'[q];[q][x]overlay=enable='gte(t,11.9)',\
         drawbox=x=0:y=0:w=8:h=8:c=black:t=fill:enable='mod(n,2)'"
    );
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &graph, "-vf", &format!("select='{}'", sel.join("+"))])
        .args(["-fps_mode", "passthrough", "-c:v", "libx264", "-bf", "0", "-pix_fmt", "yuv420p"])
        .args(["-video_track_timescale", "600"])
        .arg(out);
    gen::run(c).unwrap();
}

/// Stored frame times, via ffprobe.
fn frame_times(tc: &cutline_deps::Toolchain, p: &Path) -> Vec<f64> {
    let o = std::process::Command::new(&tc.ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time", "-of", "csv=p=0"])
        .arg(p)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect()
}

fn center(img: &image::RgbImage) -> [u8; 3] {
    img.get_pixel(img.width() / 2, img.height() / 2).0
}

fn is_color(px: [u8; 3], want: [u8; 3]) -> bool {
    px.iter().zip(want).all(|(a, b)| (*a as i32 - b as i32).abs() < 40)
}

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];

#[test]
fn grabs_on_a_sparse_capture_show_the_frame_on_screen() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let rec = d.path().join("sparse-12s.mov");
    // 0, 0.5, 1.7, 4.0, 4.3, 8.0, 9.1, 11.9 and 11.967 s
    sparse(tc, &rec, 270, 480, 12.0, &[0, 15, 51, 120, 129, 240, 273, 357, 359]);
    let times = frame_times(tc, &rec);
    assert_eq!(times.len(), 9, "{times:?}");
    assert!((times[7] - 11.9).abs() < 0.01 && (times[3] - 4.0).abs() < 0.01, "{times:?}");

    // the frame on screen at 11.433 s is the blue one stored at 9.1 s, not the white change at 11.9
    assert!(is_color(center(&grab_held(tc, &rec, 11.433).unwrap()), BLUE));
    assert!(is_color(center(&grab_source(tc, &rec, 11.433, true).unwrap()), BLUE));
    assert!(is_color(center(&grab_source(tc, &rec, 2.0, true).unwrap()), RED));
    assert!(is_color(center(&grab_source(tc, &rec, 6.5, true).unwrap()), GREEN));
    // after the last stored frame but inside the file: used to be an empty image and an empty error
    assert!(is_color(center(&grab(tc, &rec, 11.99).unwrap()), WHITE));

    // control: the same picture at a constant 30 fps
    let cfr = d.path().join("cfr-12s.mov");
    let mut c = tc.ffmpeg_cmd();
    c.arg("-i").arg(&rec).args(["-vf", "fps=30", "-c:v", "libx264", "-pix_fmt", "yuv420p"]).arg(&cfr);
    gen::run(c).unwrap();
    assert!(is_color(center(&grab(tc, &cfr, 11.433).unwrap()), BLUE));
    assert!(is_color(center(&grab_source(tc, &cfr, 11.433, false).unwrap()), BLUE));
}

#[test]
fn a_failed_grab_says_which_file_and_why() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let bad = d.path().join("broken.mov");
    std::fs::write(&bad, b"not a video").unwrap();
    let e = grab(tc, &bad, 0.9).unwrap_err().to_string();
    assert!(e.starts_with("frame grab at 0.900s failed: "), "{e}");
    assert!(e.contains("broken.mov") && e.contains("duration") && e.contains("frames"), "{e}");
    assert!(!e.trim_end().ends_with("failed:"), "the reason is never empty: {e}");
}

fn review_project(dir: &Path, name: &str, file: &str, reframe: &str) -> cutline_qa::review::ReviewReport {
    let pp = dir.join(format!("{name}.json5"));
    std::fs::write(
        &pp,
        format!(
            "{{version: 1, name: '{name}', target: 'shorts', assets: {{rec: {{path: '{file}'}}}},
              main: [{{id: 'rec', clip: 'rec' {reframe}}}], audio: {{voice_chain: 'raw'}}}}"
        ),
    )
    .unwrap();
    let l = cutline_render::pipeline::load(&pp).unwrap();
    cutline_qa::review::review_with(&l, &dir.join("review").join(name), Some("off")).unwrap()
}

/// TASK-honest-cedar-A73E step A: a 7 s capture with 25 frames, a hold across 0.9 s and a
/// rectangular crop, on a 30 fps vertical timeline. Review must finish and write the cut pairs and
/// the contact sheet. The whole 12 s sparse clip (TASK-silver-cherry-R3MZ) and its CFR control
/// must review too.
#[test]
fn review_finishes_on_sparse_captures() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("tests/fixtures/vfr")).unwrap();
    let rec7 = "tests/fixtures/vfr/sparse-7s.mov";
    let keep7 =
        [0, 10, 13, 15, 16, 18, 39, 43, 44, 45, 59, 75, 82, 103, 114, 122, 128, 129, 132, 150, 158, 162, 170, 187, 209];
    sparse(tc, &d.path().join(rec7), 574, 950, 7.0, &keep7);
    let times = frame_times(tc, &d.path().join(rec7));
    assert_eq!(times.len(), 25);
    assert!(times.windows(2).any(|w| w[0] < 0.9 && w[1] > 0.9 && w[1] - w[0] > 0.5), "a hold across 0.9 s: {times:?}");

    let rect = ", reframe: {mode: 'rect', rect: {x: 0.03, y: 0.055, w: 0.94, h: 0.945}}";
    let r = review_project(d.path(), "rect", rec7, rect);
    let out = d.path().join("review/rect");
    assert!(out.join("contact_sheet.jpg").exists());
    assert!(out.join("cuts").is_dir());
    assert!(r.images.iter().any(|p| p.ends_with("contact_sheet.jpg")));
    // the review's own picture at 0.9 s holds the frame stored at 0.6 s
    let preview = out.join("preview_540p.mp4");
    assert!(grab(tc, &preview, 0.9).is_ok());

    sparse(tc, &d.path().join("sparse-12s.mov"), 270, 480, 12.0, &[0, 15, 51, 120, 129, 240, 273, 357, 359]);
    review_project(d.path(), "whole", "sparse-12s.mov", "");
    let whole = d.path().join("review/whole/preview_540p.mp4");
    assert!(is_color(center(&grab(tc, &whole, 11.433).unwrap()), BLUE), "the held frame at 11.433 s");
    let mut c = tc.ffmpeg_cmd();
    c.arg("-i").arg(d.path().join("sparse-12s.mov")).args(["-vf", "fps=30", "-c:v", "libx264", "-pix_fmt", "yuv420p"]);
    c.arg(d.path().join("cfr-12s.mov"));
    gen::run(c).unwrap();
    review_project(d.path(), "control", "cfr-12s.mov", "");
    let control = d.path().join("review/control/preview_540p.mp4");
    assert!(is_color(center(&grab(tc, &control, 11.433).unwrap()), BLUE));
}
