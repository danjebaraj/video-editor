//! `zoom_to` geometry: the scale and fixed point that make a rect fill the view.

use cutline_core::model::{zoom_to_params, Rect};

/// Visible window (normalized source coords) at scale `s` with fixed point `f`.
fn window(s: f64, f: f64) -> (f64, f64) {
    let start = (s - 1.0) * f / s;
    (start, start + 1.0 / s)
}

#[test]
fn centered_rect_fills_the_view() {
    let r = Rect { x: 0.5, y: 0.3, w: 0.4, h: 0.4 };
    let (s, p) = zoom_to_params(&r, 0.0, 3.0);
    assert!((s - 2.5).abs() < 1e-9);
    let (x0, x1) = window(s, p.x);
    let (y0, y1) = window(s, p.y);
    assert!((x0 - 0.5).abs() < 1e-9 && (x1 - 0.9).abs() < 1e-9, "x window {x0}..{x1}");
    assert!((y0 - 0.3).abs() < 1e-9 && (y1 - 0.7).abs() < 1e-9, "y window {y0}..{y1}");
}

#[test]
fn wide_rect_fits_its_width_and_stays_centered_vertically() {
    let r = Rect { x: 0.1, y: 0.45, w: 0.5, h: 0.1 };
    let (s, p) = zoom_to_params(&r, 0.0, 3.0);
    assert!((s - 2.0).abs() < 1e-9, "fits the width: {s}");
    let (y0, y1) = window(s, p.y);
    assert!(y0 <= 0.45 && y1 >= 0.55 && ((y0 + y1) / 2.0 - 0.5).abs() < 1e-9);
}

#[test]
fn corner_rect_clamps_inside_the_frame() {
    let r = Rect { x: 0.9, y: 0.0, w: 0.1, h: 0.1 };
    let (s, p) = zoom_to_params(&r, 0.0, 3.0);
    assert!((s - 3.0).abs() < 1e-9, "capped");
    let (x0, x1) = window(s, p.x);
    let (y0, _) = window(s, p.y);
    assert!((x1 - 1.0).abs() < 1e-9 && x0 >= 0.0, "window hugs the right edge: {x0}..{x1}");
    assert!(y0.abs() < 1e-9, "and the top edge");
}

#[test]
fn padding_and_whole_frame() {
    let (s, _) = zoom_to_params(&Rect { x: 0.3, y: 0.3, w: 0.4, h: 0.4 }, 0.05, 3.0);
    assert!((s - 2.0).abs() < 1e-9, "0.4 + 2×0.05 = 0.5 → 2×");
    let (s, p) = zoom_to_params(&Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }, 0.0, 3.0);
    assert_eq!((s, p.x, p.y), (1.0, 0.5, 0.5));
}

// ---- a clip inside a screen frame: the view stays on the recording

use cutline_core::model::{clamp_zoom_focus, Point};

/// The recording of a 16:10 capture in a browser frame on 1920×1080, roughly.
const CONTENT: Rect = Rect { x: 0.12, y: 0.11, w: 0.76, h: 0.83 };

fn placed(r: &Rect, ct: &Rect) -> Rect {
    Rect { x: ct.x + r.x * ct.w, y: ct.y + r.y * ct.h, w: r.w * ct.w, h: r.h * ct.h }
}

#[test]
fn framed_zoom_near_the_right_edge_stays_on_the_recording() {
    // the rect from the bug report: needs more than 3× and hugs the recording's right edge
    let r = placed(&Rect { x: 0.78, y: 0.12, w: 0.22, h: 0.34 }, &CONTENT);
    let (s, p) = zoom_to_params(&r, 0.03, 3.0);
    assert!(s > 2.9 && s <= 3.0, "a strong zoom: {s}");
    let (_, x1) = window(s, p.x);
    assert!(x1 > CONTENT.x + CONTENT.w + 0.03, "before: the view shows the padding right of the recording ({x1})");
    let q = clamp_zoom_focus(s, p, &CONTENT);
    let (x0, x1) = window(s, q.x);
    let (y0, y1) = window(s, q.y);
    assert!((x1 - (CONTENT.x + CONTENT.w)).abs() < 1e-9, "the view ends at the recording's right edge: {x1}");
    assert!(x0 >= CONTENT.x && y0 >= CONTENT.y - 1e-9 && y1 <= CONTENT.y + CONTENT.h + 1e-9);
    // the rect is still in view (it is narrower than the view)
    assert!(x0 <= r.x && r.x + r.w <= x1 + 1e-9 && y0 <= r.y && r.y + r.h <= y1);
}

#[test]
fn a_view_that_is_already_inside_is_left_alone() {
    let r = placed(&Rect { x: 0.4, y: 0.4, w: 0.2, h: 0.2 }, &CONTENT);
    let (s, p) = zoom_to_params(&r, 0.03, 3.0);
    let q = clamp_zoom_focus(s, p, &CONTENT);
    assert!((q.x - p.x).abs() < 1e-12 && (q.y - p.y).abs() < 1e-12);
    // scale 1: nothing to move
    let p = Point { x: 0.9, y: 0.1 };
    assert_eq!(clamp_zoom_focus(1.0, p, &CONTENT), p);
}

#[test]
fn a_view_larger_than_the_recording_is_centred_on_it() {
    // vertical output: the recording is a band in the upper part, far shorter than a 2× view
    let band = Rect { x: 0.06, y: 0.25, w: 0.88, h: 0.3 };
    let q = clamp_zoom_focus(2.0, Point { x: 0.95, y: 0.0 }, &band);
    let (x0, x1) = window(2.0, q.x);
    let (y0, y1) = window(2.0, q.y);
    assert!((x1 - 0.94).abs() < 1e-9 && x0 >= 0.06, "x is shifted inward: {x0}..{x1}");
    assert!(((y0 + y1) / 2.0 - 0.4).abs() < 1e-9, "y is centred on the band: {y0}..{y1}");
    // … but never past the frame: a band at the very top keeps the view at the top
    let top = Rect { x: 0.0, y: 0.0, w: 1.0, h: 0.2 };
    let q = clamp_zoom_focus(2.0, Point { x: 0.5, y: 0.5 }, &top);
    assert!(window(2.0, q.y).0.abs() < 1e-9);
}

#[test]
fn a_picture_larger_than_the_frame_only_limits_where_it_ends() {
    // a rect-reframed clip: the source runs past the frame on the left and right
    let wide = Rect { x: -1.2, y: 0.0, w: 2.6, h: 1.0 };
    let p = Point { x: 0.0, y: 0.3 };
    assert_eq!(clamp_zoom_focus(2.0, p, &wide), p);
}

// ---- rect reframe: the crop of the target's aspect ratio around a rect

use cutline_core::model::rect_reframe;

const NINE_SIXTEEN: f64 = 1080.0 / 1920.0;

#[test]
fn rect_reframe_is_tight_centred_and_inside_the_source() {
    // a tall panel of a 1600×1000 recording, cut for 9:16: the height decides the crop
    let r = Rect { x: 0.5, y: 0.2, w: 0.2, h: 0.6 };
    let rc = rect_reframe(&r, 0.0, (1600, 1000), NINE_SIXTEEN);
    let (x, y, w, h) = rc.crop;
    assert_eq!((w, h, y), (338, 600, 200), "600 px tall → 337.5 px wide, rounded to even");
    assert_eq!(x, 792, "centred on the rect's middle (x = 960 − 169), on an even pixel");
    assert_eq!(rc.place, Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 });
    // padding (3 % of the source on each side: 48 px and 30 px) grows the box to 416×660; now
    // the width decides
    let rc = rect_reframe(&r, 0.03, (1600, 1000), NINE_SIXTEEN);
    assert_eq!(rc.crop, (960 - 208, 130, 416, 740));
    // near an edge the crop is shifted inward instead of leaving the source
    let rc = rect_reframe(&Rect { x: 0.9, y: 0.0, w: 0.1, h: 0.5 }, 0.0, (1600, 1000), NINE_SIXTEEN);
    assert_eq!(rc.crop, (1600 - 282, 0, 282, 500));
}

#[test]
fn rect_reframe_gives_up_padding_before_it_adds_bars() {
    // 0.34 of the width = 544 px; the largest 9:16 crop of 1600×1000 is 562 px wide
    let r = Rect { x: 0.6, y: 0.1, w: 0.34, h: 0.8 };
    let rc = rect_reframe(&r, 0.03, (1600, 1000), NINE_SIXTEEN);
    assert_eq!((rc.crop.2, rc.crop.3, rc.crop.1), (562, 1000, 0), "the largest crop, no bars");
    assert_eq!(rc.place, Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 });
    let (x0, x1) = (rc.crop.0 as f64, (rc.crop.0 + rc.crop.2) as f64);
    assert!(x0 <= 960.0 && x1 >= 1504.0, "the bare rect is inside: {x0}..{x1}");
}

#[test]
fn rect_reframe_fits_a_rect_too_wide_for_the_target_with_bars() {
    // half the width (800 px) cannot fit a 562 px crop: show it whole, with bars above and below
    let r = Rect { x: 0.25, y: 0.3, w: 0.5, h: 0.2 };
    let rc = rect_reframe(&r, 0.0, (1600, 1000), NINE_SIXTEEN);
    assert_eq!(rc.crop, (400, 0, 800, 1000), "the rect's width, the source's whole height");
    // 800 px wide at 9:16 would be 1422 px tall; the 1000 px of source take 70 % of the height
    assert!((rc.place.h - 1000.0 / (800.0 / NINE_SIXTEEN)).abs() < 1e-9 && rc.place.w == 1.0);
    assert!((rc.place.y - (1.0 - rc.place.h) / 2.0).abs() < 1e-12, "centred between the bars");
}

#[test]
fn rect_reframe_content_maps_source_fractions_to_the_output() {
    let r = Rect { x: 0.5, y: 0.2, w: 0.2, h: 0.6 };
    for rc in [
        rect_reframe(&r, 0.0, (1600, 1000), NINE_SIXTEEN),
        rect_reframe(&Rect { x: 0.25, y: 0.3, w: 0.5, h: 0.2 }, 0.0, (1600, 1000), NINE_SIXTEEN),
    ] {
        let ct = rc.content((1600, 1000));
        // the crop's corners land on the corners of its place in the frame
        let (cx, cy, cw, ch) = rc.crop;
        let to_out = |px: f64, py: f64| (ct.x + px / 1600.0 * ct.w, ct.y + py / 1000.0 * ct.h);
        let (x0, y0) = to_out(cx as f64, cy as f64);
        let (x1, y1) = to_out((cx + cw) as f64, (cy + ch) as f64);
        assert!((x0 - rc.place.x).abs() < 1e-9 && (y0 - rc.place.y).abs() < 1e-9, "{x0} {y0}");
        assert!((x1 - rc.place.x - rc.place.w).abs() < 1e-9 && (y1 - rc.place.y - rc.place.h).abs() < 1e-9);
    }
    // the first crop is as tall as the rect and a little wider: on screen the rect fills the
    // height and sits in the middle
    let ct = rect_reframe(&r, 0.0, (1600, 1000), NINE_SIXTEEN).content((1600, 1000));
    let p = placed(&r, &ct);
    assert!(p.y.abs() < 1e-9 && (p.h - 1.0).abs() < 1e-9, "{p:?}");
    assert!((p.w - 320.0 / 338.0).abs() < 1e-9 && (p.x + p.w / 2.0 - 0.5).abs() < 0.005, "{p:?}");
}
