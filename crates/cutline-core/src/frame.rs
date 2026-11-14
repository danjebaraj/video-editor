//! Screen frames: a recording drawn inside a window on a background. This module holds the
//! layout arithmetic (shared by validation, zooms, annotations and the renderer); the pixels
//! are drawn by `cutline-render`.

use crate::model::{FrameStyle, Rect, ScreenFrame};

/// Where the pieces of a framed clip sit, in pixels of the output frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameLayout {
    /// The window (title bar or bezel included): x, y, w, h.
    pub window: (f64, f64, f64, f64),
    /// The recording inside it: x, y, w, h (even numbers).
    pub content: (u32, u32, u32, u32),
    /// Title-bar height (browser, window).
    pub bar: f64,
    /// Bezel thickness (phone, tablet).
    pub bezel: f64,
    /// Outer corner radius of the window.
    pub radius: f64,
}

impl FrameLayout {
    /// The recording's place as fractions of the output frame.
    pub fn content_rect(&self, out: (u32, u32)) -> Rect {
        let (x, y, w, h) = self.content;
        Rect {
            x: x as f64 / out.0 as f64,
            y: y as f64 / out.1 as f64,
            w: w as f64 / out.0 as f64,
            h: h as f64 / out.1 as f64,
        }
    }
}

/// Lay out a `src` (w, h) recording in an `out` (w, h) frame. Sizes given in px are for a frame
/// whose shorter side is 1080 and scale with it.
pub fn layout(fr: &ScreenFrame, src: (u32, u32), out: (u32, u32)) -> FrameLayout {
    let (ow, oh) = (out.0 as f64, out.1 as f64);
    let u = ow.min(oh) / 1080.0;
    let aspect = src.0.max(1) as f64 / src.1.max(1) as f64;
    let pad = fr.padding.unwrap_or(0.06).clamp(0.0, 0.4) * ow.min(oh);
    // whole pixels, so the recording meets the chrome on a pixel boundary
    let (bar, bezel) = match fr.style {
        FrameStyle::Browser => ((52.0 * u).round(), 0.0),
        FrameStyle::Window => ((40.0 * u).round(), 0.0),
        FrameStyle::Phone => (0.0, (16.0 * u).round()),
        FrameStyle::Tablet => (0.0, (22.0 * u).round()),
        FrameStyle::None | FrameStyle::Off => (0.0, 0.0),
    };
    let (aw, ah) = ((ow - 2.0 * pad).max(8.0), (oh - 2.0 * pad).max(8.0));
    // the largest recording whose window still fits
    let cw = (aw - 2.0 * bezel).min((ah - bar - 2.0 * bezel) * aspect).max(4.0);
    let even = |v: f64| ((v.round() as u32).max(2)) & !1;
    let (cw, ch) = (even(cw), even(cw / aspect));
    let (ww, wh) = (cw as f64 + 2.0 * bezel, ch as f64 + bar + 2.0 * bezel);
    let wx = ((ow - ww) / 2.0).round();
    // vertical frames leave the lower part for captions
    let cy = if oh > ow { 0.40 * oh } else { 0.5 * oh };
    let wy =
        (cy - wh / 2.0).clamp(pad.min((oh - wh).max(0.0)), (oh - wh - pad).max(pad.min((oh - wh).max(0.0)))).round();
    let radius = match fr.style {
        FrameStyle::Phone => fr.radius.map(|r| r * u).unwrap_or(0.11 * ww),
        FrameStyle::Tablet => fr.radius.map(|r| r * u).unwrap_or(0.045 * ww),
        _ => fr.radius.unwrap_or(14.0) * u,
    };
    FrameLayout {
        window: (wx, wy, ww, wh),
        content: ((wx + bezel) as u32, (wy + bar + bezel) as u32, cw, ch),
        bar,
        bezel,
        radius,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(style: FrameStyle) -> ScreenFrame {
        ScreenFrame { style, ..Default::default() }
    }

    #[test]
    fn browser_frame_fits_and_keeps_the_aspect() {
        let l = layout(&frame(FrameStyle::Browser), (1600, 1000), (1920, 1080));
        let (x, y, w, h) = l.content;
        assert_eq!((w % 2, h % 2), (0, 0));
        assert!((w as f64 / h as f64 - 1.6).abs() < 0.01, "{w}x{h}");
        // 6 % padding of 1080 on each side; bar + content fill the height
        assert!((l.window.1 - 64.8).abs() <= 1.0 && (l.window.3 - (1080.0 - 129.6)).abs() <= 2.0, "{l:?}");
        assert_eq!(y as f64, l.window.1 + l.bar);
        assert!((x as f64 + w as f64 / 2.0 - 960.0).abs() <= 1.0, "centred");
    }

    #[test]
    fn vertical_output_places_the_window_in_the_upper_part() {
        let l = layout(&frame(FrameStyle::Browser), (1920, 1080), (1080, 1920));
        let mid = l.window.1 + l.window.3 / 2.0;
        assert!((mid - 768.0).abs() < 2.0, "{l:?}");
        assert!((l.window.2 - (1080.0 - 129.6)).abs() < 2.0, "width-limited: {l:?}");
    }

    #[test]
    fn phone_frame_has_a_bezel_all_round() {
        let l = layout(&frame(FrameStyle::Phone), (1170, 2532), (1920, 1080));
        assert!(l.bezel > 10.0 && l.bar == 0.0);
        assert_eq!(l.content.0 as f64, l.window.0 + l.bezel);
        assert!((l.window.3 - (1080.0 - 129.6)).abs() <= 2.0, "height-limited: {l:?}");
    }
}
