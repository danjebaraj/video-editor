//! FEAT-magic-alpaca-JPD6: `cutline review --compare-shots LEFT,RIGHT` puts two shots' graded
//! frames side by side with their mean BT.709 luma, so an editor can match exposure.

use cutline_qa::review::{review_opts, ShotComparison};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

fn shot(tc: &cutline_deps::Toolchain, out: &Path, color: &str) {
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &format!("color=c={color}:s=320x180:r=30:d=2"),
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
    ])
    .arg(out);
    gen::run(c).unwrap();
}

fn compare(d: &Path, name: &str, dark_grade: &str) -> (ShotComparison, cutline_qa::review::ReviewReport) {
    let p = d.join(format!("{name}.json5"));
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: '{name}', target: 'custom', width: 320, height: 180, fps: 30,
              assets: {{dark: {{path: 'dark.mp4'}}, light: {{path: 'light.mp4'}}}},
              main: [{{id: 'dark', clip: 'dark' {dark_grade}}}, {{id: 'light', clip: 'light'}}]}}"
        ),
    )
    .unwrap();
    let l = cutline_render::pipeline::load(&p).unwrap();
    let out = d.join("review").join(name);
    let r = review_opts(&l, &out, Some("off"), Some(("dark", "light"))).unwrap();
    let j: ShotComparison = serde_json::from_slice(&std::fs::read(out.join("shots/compare_dark_light.json")).unwrap())
        .unwrap_or_else(|e| panic!("{e}"));
    (j, r)
}

#[test]
fn shots_are_compared_and_grading_closes_the_gap() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    shot(tc, &d.path().join("dark.mp4"), "0x505050");
    shot(tc, &d.path().join("light.mp4"), "0x909090");

    let (before, r) = compare(d.path(), "before", "");
    assert!(r.images.iter().any(|p| p.ends_with("shots/compare_dark_light.png")));
    assert_eq!((before.left.clip.as_str(), before.right.clip.as_str()), ("dark", "light"));
    // the middles of the two 2 s shots
    assert!((before.left.at - 1.0).abs() < 0.05 && (before.right.at - 3.0).abs() < 0.05, "{before:?}");
    assert!(before.difference > 0.15, "{before:?}");
    // the picture: dark on the left, light on the right
    let img = image::open(&before.image).unwrap().to_rgb8();
    let (lp, rp) =
        (img.get_pixel(img.width() / 4, img.height() / 2)[0], img.get_pixel(3 * img.width() / 4, img.height() / 2)[0]);
    assert!(lp < rp, "{lp} {rp}");
    assert!(
        !r.qa_ok || r.suggestions.iter().all(|s| !s.contains("luma")),
        "QA and suggestions don't use the comparison"
    );

    // grade only the darker shot toward the reference
    let (after, _) = compare(d.path(), "after", ", color: {exposure: 0.8}");
    eprintln!("before: {before:?}\nafter: {after:?}");
    assert!(after.difference.abs() < before.difference.abs(), "{before:?} → {after:?}");
    assert!((after.right.mean_luma - before.right.mean_luma).abs() <= 1.0 / 255.0, "the reference stays put");
}

#[test]
fn an_unknown_clip_is_an_actionable_error() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    shot(tc, &d.path().join("dark.mp4"), "0x505050");
    shot(tc, &d.path().join("light.mp4"), "0x909090");
    let p = d.path().join("p.json5");
    std::fs::write(
        &p,
        "{version: 1, target: 'custom', width: 320, height: 180, fps: 30,
          assets: {dark: {path: 'dark.mp4'}, light: {path: 'light.mp4'}},
          main: [{id: 'dark', clip: 'dark'}, {transition: {type: 'fade', duration: 0.9}}, {id: 'light', clip: 'light', out: 1}, {transition: {type: 'fade', duration: 0.9}}, {id: 'end', clip: 'dark'}]}",
    )
    .unwrap();
    let l = cutline_render::pipeline::load(&p).unwrap();
    let e = review_opts(&l, &d.path().join("r"), Some("off"), Some(("dark", "lite"))).unwrap_err().to_string();
    assert!(e.contains("no main clip `lite`") && e.contains("clips: dark, light, end"), "{e}");
    let e = review_opts(&l, &d.path().join("r"), Some("off"), Some(("dark", "light"))).unwrap_err().to_string();
    assert!(e.contains("never on screen alone"), "{e}");
}
