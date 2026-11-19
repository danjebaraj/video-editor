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

/// Bounds (x0, x1, y0, y1) of the red block in a frame, scanning the row and column through `at`.
fn red_bounds(f: &image::RgbImage, at: (u32, u32)) -> (u32, u32, u32, u32) {
    let is_red = |x: u32, y: u32| {
        let p = f.get_pixel(x, y).0;
        p[0] > 140 && p[1] < 90
    };
    let xs: Vec<u32> = (0..f.width()).filter(|x| is_red(*x, at.1)).collect();
    let ys: Vec<u32> = (0..f.height()).filter(|y| is_red(at.0, *y)).collect();
    assert!(!xs.is_empty() && !ys.is_empty(), "no red at {at:?}");
    (xs[0], *xs.last().unwrap() + 1, ys[0], *ys.last().unwrap() + 1)
}

#[test]
fn a_zoom_near_the_edge_of_a_framed_recording_shows_no_background() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path());
    // the top-right corner of the recording; `duration: 0` zooms at once.
    // A translucent blue highlight sits on the red block (x 0.75..0.9375, y 0.1..0.3).
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, name: 'f', target: 'custom', width: 640, height: 360, fps: 30,
          default_frame: {style: 'none', background: '#0000FF', shadow: 0, radius: 0},
          assets: {screen: {path: 'screen.mp4'}}, audio: {voice_chain: 'raw'},
          main: [{id: 'a', clip: 'screen', effects: [
            {type: 'zoom_to', rect: {x: 0.78, y: 0.12, w: 0.22, h: 0.34}, start: 0.5, duration: 0}]}],
          layers: [{type: 'highlight', rect: {x: 0.75, y: 0.1, w: 0.1875, h: 0.2}, style: 'fill', color: '#0000FF',
                    radius: 0, enter: 'none', anchor: 'a', duration: 3}]}",
    )
    .unwrap();
    let (out, tl) = render(&pp);
    let f = read_frames(tc, &out, &[14, 15, 60]).unwrap();
    // before the zoom: the frame's background all round
    assert!(blue_bg(mean_rgb(&f[0], 4, 100, 12, 100)) && blue_bg(mean_rgb(&f[0], 624, 100, 12, 100)));
    for (i, fr) in f[1..].iter().enumerate() {
        // zoomed (already on the first frame at `start`): recording from edge to edge
        for (x, y) in [(2, 40), (2, 300), (626, 20), (626, 180), (626, 340), (300, 2), (300, 346)] {
            let c = mean_rgb(fr, x, y, 12, 12);
            assert!(!blue_bg(c) && c[0] > 200.0, "frame {i}: background in view at ({x}, {y}): {c:?}");
        }
    }
    // the view ends at the recording's right edge, and the highlight is still on the block
    let c = &tl.clips[0];
    let ct = c.content.unwrap();
    let map = cutline_render::annot::zoom_map(c, &tl, 60, 640.0, 360.0);
    let (right, _) = map.pt((ct.x + ct.w) * 640.0, 0.0);
    assert!((right - 640.0).abs() < 1.0, "recording's right edge on screen: {right}");
    let (bx0, by0) = map.pt((ct.x + 0.75 * ct.w) * 640.0, (ct.y + 0.1 * ct.h) * 360.0);
    let (bx1, by1) = map.pt((ct.x + 0.9375 * ct.w) * 640.0, (ct.y + 0.3 * ct.h) * 360.0);
    let mid = (((bx0 + bx1) / 2.0) as u32, ((by0 + by1) / 2.0) as u32);
    let (x0, x1, y0, y1) = red_bounds(&f[2], mid);
    for (got, want) in [(x0, bx0), (x1, bx1), (y0, by0), (y1, by1)] {
        assert!(
            (got as f64 - want).abs() <= 3.0,
            "block at {x0}..{x1} × {y0}..{y1}, mapped {bx0}..{bx1} × {by0}..{by1}"
        );
    }
    let inside = mean_rgb(&f[2], mid.0 - 10, mid.1 - 10, 20, 20);
    assert!(inside[0] > 140.0 && inside[2] > 40.0, "block tinted by the highlight: {inside:?}");
    let left = mean_rgb(&f[2], x0 - 16, mid.1 - 10, 8, 20);
    assert!(white(left), "no tint beside the block: {left:?}");
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

/// Mean absolute luma step between neighbouring pixels (both directions) over a rectangle: how
/// crisp fine detail such as UI text is after scaling.
fn edge_contrast(f: &image::RgbImage, x: u32, y: u32, w: u32, h: u32) -> f64 {
    let luma = |px: u32, py: u32| {
        let p = f.get_pixel(px, py).0;
        0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
    };
    let mut sum = 0.0;
    let mut n = 0.0;
    for py in y..y + h - 1 {
        for px in x..x + w - 1 {
            sum += (luma(px + 1, py) - luma(px, py)).abs() + (luma(px, py + 1) - luma(px, py)).abs();
            n += 2.0;
        }
    }
    sum / n
}

/// F2 acceptance: a framed recording is as sharp as the same recording scaled to the same size
/// with a plain `fit_pad` (no extra softening from the window, padding or shadow).
#[test]
fn framing_keeps_text_as_sharp_as_fit_pad_at_the_same_scale() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // fine high-contrast detail, like small UI text
    gen::grid_video(tc, &d.path().join("ui.mp4"), 1280, 800, 30, 2.0, 3).unwrap();
    let fr = ScreenFrame { style: FrameStyle::Browser, ..Default::default() };
    let l = cutline_core::frame::layout(&fr, (1280, 800), (1280, 720));
    let (cx, cy, cw, ch) = l.content;
    let write = |name: &str, w: u32, h: u32, clip: &str| {
        let p = d.path().join(name);
        std::fs::write(
            &p,
            format!(
                "{{version: 1, name: '{name}', target: 'custom', width: {w}, height: {h}, fps: 30,
                  assets: {{ui: {{path: 'ui.mp4'}}}}, audio: {{voice_chain: 'raw'}}, main: [{clip}]}}"
            ),
        )
        .unwrap();
        p
    };
    let framed = write(
        "framed.json5",
        1280,
        720,
        "{id: 'a', clip: 'ui', duration: 1, frame: {style: 'browser', background: '#101014', shadow: 0.5}}",
    );
    let (pw, ph) = (cw & !1, ch & !1);
    let plain = write("plain.json5", pw, ph, "{id: 'a', clip: 'ui', duration: 1, reframe: {mode: 'fit_pad'}}");
    let f = &read_frames(tc, &render(&framed).0, &[15]).unwrap()[0];
    let g = &read_frames(tc, &render(&plain).0, &[15]).unwrap()[0];
    // the middle of the recording in each (away from rounded corners and the bar)
    let (mw, mh) = (cw * 6 / 10, ch * 6 / 10);
    let in_frame = edge_contrast(f, cx + (cw - mw) / 2, cy + (ch - mh) / 2, mw, mh);
    let in_plain = edge_contrast(g, (pw - mw) / 2, (ph - mh) / 2, mw, mh);
    assert!(in_plain > 5.0, "the measure sees the detail: {in_plain}");
    assert!(in_frame >= in_plain * 0.97, "framed {in_frame:.2} vs fit_pad {in_plain:.2} at {cw}×{ch}");
}
