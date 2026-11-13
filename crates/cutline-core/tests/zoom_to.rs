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
