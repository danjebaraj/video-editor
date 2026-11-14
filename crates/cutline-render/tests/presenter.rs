//! Presenter layer: the camera as a bubble, a side panel or the full frame, switching over time.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{mean_rgb, read_frames};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;

#[test]
fn presenter_layouts_switch_over_time_and_centre_on_the_face() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("screen.png"), 640, 360, "0xFFFFFF").unwrap();
    // camera: green, with a red 80×80 "face" centred at (200, 100) of 320×240, and a tone
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "color=c=0x00FF00:s=320x240:d=5:r=30,drawbox=x=160:y=60:w=80:h=80:c=red:t=fill"])
        .args(["-f", "lavfi", "-i", "sine=f=440:d=5", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(d.path().join("cam.mp4"));
    gen::run(c).unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, name: 'p', target: 'custom', width: 640, height: 360, fps: 30,
          assets: {screen: {path: 'screen.png'}, cam: {path: 'cam.mp4'}}, audio: {voice_chain: 'raw'},
          main: [{id: 'a', clip: 'screen', duration: 4}],
          layers: [{type: 'presenter', asset: 'cam', focus: {x: 0.625, y: 0.4167}, audio: true, shadow: false, border: 0,
                    layouts: [{at: 1, layout: 'presenter_full'}, {at: 2, layout: 'screen_full'}, {at: 3, layout: 'side_by_side'}]}]}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let (plan, _) = pipeline::render(&l, &pr, &RenderOptions { jobs: 3, ..Default::default() }).unwrap();
    let f = read_frames(tc, &plan.output, &[15, 45, 75, 105]).unwrap();
    let red = |c: [f64; 3]| c[0] > 200.0 && c[1] < 60.0 && c[2] < 60.0;
    let green = |c: [f64; 3]| c[1] > 200.0 && c[0] < 60.0;
    let white = |c: [f64; 3]| c.iter().all(|v| *v > 235.0);

    // bubble: a 116 px circle in the bottom-right of the safe zone, the face in its middle
    let (bx, by) = (608 - 116, 18 + 313 - 116);
    assert!(red(mean_rgb(&f[0], bx + 50, by + 50, 16, 16)), "{:?}", mean_rgb(&f[0], bx + 50, by + 50, 16, 16));
    assert!(white(mean_rgb(&f[0], bx + 1, by + 1, 6, 6)), "outside the circle, inside its square");
    assert!(green(mean_rgb(&f[0], bx + 52, by + 6, 12, 8)), "camera background around the face");
    assert!(white(mean_rgb(&f[0], 100, 100, 40, 40)), "the screen elsewhere");
    // presenter_full: the camera covers the frame; the 16:9 crop of the 4:3 picture is taken
    // around the face's height (the full width is used, so the face stays right of centre)
    assert!(red(mean_rgb(&f[1], 390, 170, 20, 20)), "{:?}", mean_rgb(&f[1], 390, 170, 20, 20));
    assert!(green(mean_rgb(&f[1], 20, 20, 30, 30)) && green(mean_rgb(&f[1], 590, 320, 30, 30)));
    // screen_full: no camera
    assert!(white(mean_rgb(&f[2], bx + 50, by + 50, 16, 16)) && white(mean_rgb(&f[2], 310, 170, 20, 20)));
    // side_by_side: a tall panel on the right, the screen free on the left
    let px = 608 - 204;
    assert!(!white(mean_rgb(&f[3], px + 40, 100, 120, 160)), "panel on the right");
    assert!(white(mean_rgb(&f[3], 100, 100, 200, 160)), "screen on the left");
    assert!(white(mean_rgb(&f[3], px - 30, 170, 20, 20)), "left of the panel");

    // its sound is in the mix
    let a = cutline_testkit::analyze::read_audio(tc, &plan.output, 1.0, Some(1.0), 1).unwrap();
    assert!((cutline_testkit::analyze::dominant_freq(&a) - 440.0).abs() < 12.0);
}

#[test]
fn presenter_layout_times_must_increase() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("s.png"), 64, 64, "0xFFFFFF").unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, assets: {s: {path: 's.png'}}, main: [{id: 'a', clip: 's', duration: 4}],
          layers: [{type: 'presenter', asset: 's', scale: 0.9, layouts: [{at: 2, layout: 'bubble'}, {at: 1, layout: 'screen_full'}]}]}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    let has = |code: &str, path: &str| l.diags.iter().any(|x| x.code == code && x.path == path);
    assert!(has("E_BAD_VALUE", "/layers/0/layouts/1/at") && has("E_BAD_VALUE", "/layers/0/scale"), "{:?}", l.diags);
}
