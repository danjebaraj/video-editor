//! `reframe: {mode: "rect"}`: a crop of the target's aspect ratio around a rect of the source,
//! with annotation rects still written in fractions of the source.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{mean_rgb, read_frames};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

fn render(project: &Path) -> (PathBuf, cutline_core::Timeline) {
    let l = pipeline::load(project).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let opts = RenderOptions { jobs: 3, ..Default::default() };
    (pipeline::render(&l, &pr, &opts).unwrap().0.output, pr.timeline)
}

/// A 320×200 "screen": white, with a red block at x 240..300, y 20..60
/// (fractions 0.75..0.9375 × 0.1..0.3).
const BLOCK: &str = "{x: 0.75, y: 0.1, w: 0.1875, h: 0.2}";

fn project(d: &Path, top: &str, reframe: &str, layers: &str) -> PathBuf {
    let tc = test_toolchain().unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "color=c=white:s=320x200:d=2:r=30,drawbox=x=240:y=20:w=60:h=40:c=red:t=fill"])
        .args(["-pix_fmt", "yuv420p"])
        .arg(d.join("screen.mp4"));
    gen::run(c).unwrap();
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'r', target: 'custom', width: 180, height: 320, fps: 30, {top}
              assets: {{screen: {{path: 'screen.mp4'}}}}, audio: {{voice_chain: 'raw'}},
              main: [{{id: 'a', clip: 'screen', reframe: {reframe}}}], layers: [{layers}]}}"
        ),
    )
    .unwrap();
    p
}

fn white(c: [f64; 3]) -> bool {
    c.iter().all(|v| *v > 235.0)
}
fn red(c: [f64; 3]) -> bool {
    c[0] > 200.0 && c[1] < 50.0 && c[2] < 50.0
}

#[test]
fn rect_reframe_crops_to_the_rect_and_annotations_keep_source_fractions() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // source px 224..304 × 10..110 (80×100): for 9:16 the width decides, 80×142 from the top
    let pp = project(
        d.path(),
        // a clip with its own reframe does not take the default frame
        "default_frame: {style: 'browser'},",
        "{mode: 'rect', rect: {x: 0.7, y: 0.05, w: 0.25, h: 0.5}, padding: 0}",
        &format!(
            "{{type: 'highlight', rect: {BLOCK}, style: 'fill', color: '#0000FF', radius: 0, enter: 'none', start: 1}}"
        ),
    );
    let (out, tl) = render(&pp);
    let c = &tl.clips[0];
    assert!(c.frame.is_none());
    let ct = c.content.expect("the source's place in the output");
    // 180 px show 80 px of source: 2.25×; the source's left edge is 224 px (504 output px) off screen
    assert!((ct.w * 180.0 - 720.0).abs() < 0.01 && (ct.x * 180.0 + 504.0).abs() < 0.01, "{ct:?}");
    assert!(ct.y.abs() < 1e-9 && (ct.h * 320.0 - 200.0 * 320.0 / 142.0).abs() < 0.01, "{ct:?}");
    let f = read_frames(tc, &out, &[10, 45]).unwrap();
    // the block: x (240−224)·2.25 = 36..171, y 20·(320/142) = 45..135
    assert!(red(mean_rgb(&f[0], 50, 55, 100, 70)), "{:?}", mean_rgb(&f[0], 50, 55, 100, 70));
    assert!(white(mean_rgb(&f[0], 4, 60, 24, 60)) && white(mean_rgb(&f[0], 40, 150, 100, 150)));
    assert!(white(mean_rgb(&f[0], 60, 4, 80, 30)), "above the block");
    // the highlight, written for the source, lands on the block: tinted inside, clean outside
    let inside = mean_rgb(&f[1], 50, 55, 100, 70);
    assert!(inside[0] > 140.0 && inside[1] < 60.0 && inside[2] > 40.0, "tinted block: {inside:?}");
    assert!(white(mean_rgb(&f[1], 4, 60, 24, 60)), "left of it: {:?}", mean_rgb(&f[1], 4, 60, 24, 60));
    assert!(white(mean_rgb(&f[1], 40, 146, 100, 100)), "below it: {:?}", mean_rgb(&f[1], 40, 146, 100, 100));
    assert!(white(mean_rgb(&f[1], 60, 4, 80, 34)), "above it: {:?}", mean_rgb(&f[1], 60, 4, 80, 34));
}

#[test]
fn a_rect_too_wide_for_the_target_is_shown_whole_between_bars() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // source px 160..320 wide (160 px): a 9:16 window that wide is 284 px tall, the source 200
    let pp = project(
        d.path(),
        "",
        "{mode: 'rect', rect: {x: 0.5, y: 0.3, w: 0.5, h: 0.2}, padding: 0.03, pad_color: '#00FF00'}",
        "",
    );
    let (out, tl) = render(&pp);
    let ct = tl.clips[0].content.unwrap();
    let (top, height) = (ct.y * 320.0, ct.h * 320.0);
    assert!((height - 225.0).abs() < 0.01 && (top - 47.5).abs() < 0.01, "200 px of source at 1.125×: {ct:?}");
    let f = &read_frames(tc, &out, &[10]).unwrap()[0];
    let green = |c: [f64; 3]| c[1] > 200.0 && c[0] < 50.0 && c[2] < 50.0;
    assert!(green(mean_rgb(f, 10, 4, 160, 36)), "bar above: {:?}", mean_rgb(f, 10, 4, 160, 36));
    assert!(green(mean_rgb(f, 10, 280, 160, 36)), "bar below: {:?}", mean_rgb(f, 10, 280, 160, 36));
    // the block: x (240−160)·1.125 = 90..157, y 48 + 20·1.125 = 70..115
    assert!(red(mean_rgb(f, 96, 76, 56, 34)), "{:?}", mean_rgb(f, 96, 76, 56, 34));
    assert!(white(mean_rgb(f, 10, 140, 160, 100)));
}
