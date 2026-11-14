//! Screen frames: a recording inside generated window chrome on a background.

use cutline_core::model::{FrameStyle, Rect, ScreenFrame};
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

/// A 320×200 "screen": white, with a red block at x 240..300, y 20..60 (fractions 0.75..0.9375 × 0.1..0.3).
fn setup(d: &Path) {
    let tc = test_toolchain().unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "color=c=white:s=320x200:d=3:r=30,drawbox=x=240:y=20:w=60:h=40:c=red:t=fill"])
        .args(["-pix_fmt", "yuv420p"])
        .arg(d.join("screen.mp4"));
    gen::run(c).unwrap();
    gen::still(tc, &d.join("title.png"), 640, 360, "0x00FF00").unwrap();
}

fn project(d: &Path, top: &str, main: &str) -> PathBuf {
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'f', target: 'custom', width: 640, height: 360, fps: 30, {top}
              assets: {{screen: {{path: 'screen.mp4'}}, title: {{path: 'title.png'}}}}, audio: {{voice_chain: 'raw'}},
              main: [{main}]}}"
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
fn blue_bg(c: [f64; 3]) -> bool {
    c[2] > 200.0 && c[0] < 40.0 && c[1] < 40.0
}

#[test]
fn browser_frame_places_the_recording_in_a_window_on_the_background() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path());
    let frame = "frame: {style: 'browser', url: 'app.example.com', background: '#0000FF', shadow: 0}";
    let pp = project(d.path(), "", &format!("{{id: 'a', clip: 'screen', {frame}}}"));
    let (out, tl) = render(&pp);
    let fr = ScreenFrame { style: FrameStyle::Browser, ..Default::default() };
    let l = cutline_core::frame::layout(&fr, (320, 200), (640, 360));
    let (cx, cy, cw, ch) = l.content;
    assert_eq!(tl.clips[0].content, Some(l.content_rect((640, 360))));
    let f = &read_frames(tc, &out, &[30]).unwrap()[0];
    // background in the padding
    assert!(blue_bg(mean_rgb(f, 4, 4, 12, 12)), "{:?}", mean_rgb(f, 4, 4, 12, 12));
    assert!(blue_bg(mean_rgb(f, 620, 340, 12, 12)));
    // the recording fills the content rect: white in its middle, the red block where it belongs
    assert!(white(mean_rgb(f, cx + cw / 2 - 10, cy + ch / 2, 20, 20)));
    let (bx, by) = (cx + (0.75 * cw as f64) as u32, cy + (0.1 * ch as f64) as u32);
    assert!(red(mean_rgb(f, bx + 8, by + 8, 40, 30)), "{:?}", mean_rgb(f, bx + 8, by + 8, 40, 30));
    // the title bar above it: dark chrome, neither background nor recording
    let bar = mean_rgb(f, cx + 100, cy - (l.bar as u32) + 3, 40, (l.bar as u32).saturating_sub(8).max(2));
    assert!(bar.iter().all(|v| *v > 20.0 && *v < 90.0), "bar {bar:?}");
    // rounded bottom corners: the very corner pixel shows the background, not the recording
    let corner = mean_rgb(f, cx, cy + ch - 1, 1, 1);
    assert!(corner[2] > 150.0 && corner[0] < 120.0, "bottom-left corner is cut: {corner:?}");
    assert!(white(mean_rgb(f, cx + 8, cy + ch - 9, 3, 3)), "just inside the corner");
}

#[test]
fn zoom_to_rects_are_relative_to_the_recording() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path());
    // zoom to the red block, given in fractions of the recording
    let rect = "{x: 0.75, y: 0.1, w: 0.1875, h: 0.2}";
    let pp = project(
        d.path(),
        "default_frame: {style: 'none', background: '#0000FF', shadow: 0, radius: 0},",
        &format!(
            "{{id: 't', clip: 'title', duration: 1}},
             {{id: 'a', clip: 'screen', effects: [{{type: 'zoom_to', rect: {rect}, start: 0.2, duration: 0.4, padding: 0}}]}},
             {{id: 'b', clip: 'screen', frame: {{style: 'off'}}}}"
        ),
    );
    let (out, tl) = render(&pp);
    // default_frame applies to recordings only, and `off` opts out
    assert!(tl.clips[0].frame.is_none() && tl.clips[0].content.is_none(), "stills are not framed");
    assert!(tl.clips[1].frame.is_some());
    assert!(tl.clips[2].frame.is_none());
    let a = tl.clips[1].start as u64;
    let b = tl.clips[2].start as u64;
    let f = read_frames(tc, &out, &[10, a + 45, b + 10]).unwrap();
    assert!(mean_rgb(&f[0], 300, 170, 40, 20)[1] > 200.0, "title is full-bleed green");
    // zoomed: the red block fills the view (it would be a small block off-centre otherwise)
    let content = tl.clips[1].content.unwrap();
    let placed = Rect {
        x: content.x + 0.75 * content.w,
        y: content.y + 0.1 * content.h,
        w: 0.1875 * content.w,
        h: 0.2 * content.h,
    };
    let (s, _) = cutline_core::model::zoom_to_params(&placed, 0.0, 3.0);
    assert!(s > 2.5, "needs a strong zoom: {s}");
    assert!(red(mean_rgb(&f[1], 300, 160, 40, 40)), "red block centred: {:?}", mean_rgb(&f[1], 300, 160, 40, 40));
    let (bw, bh) = (placed.w * s * 640.0, placed.h * s * 360.0);
    // (the zoom is capped at 3×, so the block is about 285×190 px rather than filling the view)
    assert!(bw > 250.0 && bh > 170.0, "block size on screen: {bw}x{bh}");
    assert!(white(mean_rgb(&f[1], 40, 20, 20, 20)), "recording around the block, no background in view");
    // unframed clip: the recording covers the frame
    assert!(white(mean_rgb(&f[2], 4, 150, 12, 12)), "full-bleed again: {:?}", mean_rgb(&f[2], 4, 150, 12, 12));
}

#[test]
fn frame_validation() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path());
    let pp = project(
        d.path(),
        "",
        "{id: 'a', clip: 'screen', frame: {style: 'browser', padding: 0.9}, reframe: {mode: 'cover'}},
         {id: 'b', clip: 'screen', frame: {background: 'nope'}},
         {id: 'c', clip: 'screen', frame: {background: {gradient: ['#000000']}}}",
    );
    let l = pipeline::load(&pp).unwrap();
    let has = |code: &str, path: &str| l.diags.iter().any(|x| x.code == code && x.path == path);
    assert!(has("E_FIELD_CONFLICT", "/main/0/frame"), "{:?}", l.diags);
    assert!(has("E_BAD_VALUE", "/main/0/frame/padding"), "{:?}", l.diags);
    assert!(has("E_UNKNOWN_ASSET", "/main/1/frame/background"), "{:?}", l.diags);
    assert!(has("E_BAD_VALUE", "/main/2/frame/background/gradient"), "{:?}", l.diags);
}
