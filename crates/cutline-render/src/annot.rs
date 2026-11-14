//! Annotation layers (highlight, spotlight, arrow, keys, cursor), rasterized here frame by frame
//! into an alpha overlay video that the final pass composites over the footage and overlays,
//! under text and captions.
//!
//! Drawing in Rust rather than with ffmpeg filters keeps every shape anti-aliased and lets an
//! annotation follow the zoom of the clip underneath exactly: the zoom's scale and window are
//! evaluated per frame with the same terms the zoom filter uses.

use crate::effects;
use crate::Geometry;
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use anyhow::{Context, Result};
use cutline_core::model::{AnnotEnter, CursorKey, Ease, HighlightStyle, Layer, Position, Rect};
use cutline_core::timeline::{TClip, TLayer, Timeline};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One rendered run of annotations: an alpha video covering `[start, end)` output seconds.
#[derive(Debug, Clone, Serialize)]
pub struct AnnotRender {
    pub path: PathBuf,
    pub start: f64,
    pub end: f64,
}

const FULL: Rect = Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

/// Look shared by every annotation in a render.
pub struct Style {
    pub accent: [u8; 3],
    /// Safe rectangle in pixels of the render geometry (x, y, w, h).
    pub safe: (f64, f64, f64, f64),
    /// Pixel sizes are given for the full output; `k` scales them to the render geometry.
    pub k: f64,
    pub font: Option<FontArc>,
    /// Fonts, colors and images for the business card templates.
    pub cards: Option<crate::cards::CardCtx>,
    /// Anything else that changes the pixels (brand, image fingerprints): part of the cache key.
    pub look_key: String,
}

// ------------------------------------------------------------------ canvas

/// Straight-alpha RGBA frame.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h, px: vec![0; (w * h * 4) as usize] }
    }
    pub fn clear(&mut self) {
        self.px.fill(0);
    }
    /// Source-over blend of one pixel.
    #[inline]
    pub fn blend(&mut self, x: i64, y: i64, rgb: [u8; 3], a: f64) {
        if a <= 0.003 || x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        let a = a.min(1.0);
        let da = self.px[i + 3] as f64 / 255.0;
        let oa = a + da * (1.0 - a);
        for (c, src) in rgb.iter().enumerate() {
            let v = (*src as f64 * a + self.px[i + c] as f64 * da * (1.0 - a)) / oa;
            self.px[i + c] = v.round() as u8;
        }
        self.px[i + 3] = (oa * 255.0).round() as u8;
    }
    /// Fill where `cov(x, y)` (pixel centers) is positive, inside `bbox` (x0, y0, x1, y1).
    pub fn shape(&mut self, bbox: (f64, f64, f64, f64), rgb: [u8; 3], alpha: f64, cov: impl Fn(f64, f64) -> f64) {
        if alpha <= 0.003 {
            return;
        }
        let x0 = bbox.0.floor().max(0.0) as i64;
        let y0 = bbox.1.floor().max(0.0) as i64;
        let x1 = (bbox.2.ceil() as i64).min(self.w as i64);
        let y1 = (bbox.3.ceil() as i64).min(self.h as i64);
        for y in y0..y1 {
            for x in x0..x1 {
                let c = cov(x as f64 + 0.5, y as f64 + 0.5);
                if c > 0.0 {
                    self.blend(x, y, rgb, c.min(1.0) * alpha);
                }
            }
        }
    }
    /// Draw text with its baseline at `y`; returns the advance width.
    pub fn text(&mut self, font: &FontArc, size: f64, x: f64, y: f64, text: &str, rgb: [u8; 3], alpha: f64) -> f64 {
        let scale = PxScale::from(size as f32);
        let sf = font.as_scaled(scale);
        let mut pen = x as f32;
        let mut prev = None;
        for ch in text.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                pen += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(scale, ab_glyph::point(pen, y as f32));
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, c| {
                    self.blend(b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64, rgb, c as f64 * alpha)
                });
            }
            pen += sf.h_advance(id);
            prev = Some(id);
        }
        pen as f64 - x
    }
}

pub fn text_width(font: &FontArc, size: f64, text: &str) -> f64 {
    let sf = font.as_scaled(PxScale::from(size as f32));
    let mut w = 0.0f32;
    let mut prev = None;
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w as f64
}

// ------------------------------------------------------------------ distance fields

/// Signed distance to a rounded rectangle (negative inside).
pub fn sd_rrect(px: f64, py: f64, x: f64, y: f64, w: f64, h: f64, r: f64) -> f64 {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let (qx, qy) = ((px - cx).abs() - (w / 2.0 - r), (py - cy).abs() - (h / 2.0 - r));
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

fn sd_segment(px: f64, py: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((px - a.0) * dx + (py - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    ((px - a.0 - t * dx).powi(2) + (py - a.1 - t * dy).powi(2)).sqrt()
}

/// Signed distance to a simple polygon (negative inside).
fn sd_polygon(px: f64, py: f64, pts: &[(f64, f64)]) -> f64 {
    let mut d = f64::INFINITY;
    let mut inside = false;
    let n = pts.len();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        d = d.min(sd_segment(px, py, a, b));
        if (a.1 > py) != (b.1 > py) && px < (b.0 - a.0) * (py - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    if inside {
        -d
    } else {
        d
    }
}

/// Anti-aliased coverage of a distance field.
#[inline]
fn aa(d: f64) -> f64 {
    (0.5 - d).clamp(0.0, 1.0)
}

// ------------------------------------------------------------------ following the zoom

/// Pixel mapping from the unzoomed output frame to what is on screen: `x' = x·s + ox`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Map {
    pub s: f64,
    pub ox: f64,
    pub oy: f64,
}

impl Map {
    pub const IDENTITY: Map = Map { s: 1.0, ox: 0.0, oy: 0.0 };
    pub fn pt(&self, x: f64, y: f64) -> (f64, f64) {
        (x * self.s + self.ox, y * self.s + self.oy)
    }
}

/// The zoom of clip `c` at timeline frame `fr`, as a pixel mapping in a `w×h` frame.
pub fn zoom_map(c: &TClip, tl: &Timeline, fr: i64, w: f64, h: f64) -> Map {
    let dur = tl.secs(c.frames);
    let t = tl.secs((fr - c.start).clamp(0, c.frames - 1));
    let (s, left, top) = effects::zoom_at(c, dur, t);
    Map { s, ox: -left * w * s, oy: -top * h * s }
}

/// The clip an annotation belongs to at frame `fr`: its anchor clip while that is on screen,
/// otherwise the clip under it.
fn clip_under<'a>(tl: &'a Timeline, lay: &TLayer, fr: i64) -> Option<&'a TClip> {
    let anchored = lay
        .layer
        .placement()
        .and_then(|p| p.anchor.as_deref())
        .and_then(|a| tl.clip_by_id(a))
        .filter(|c| fr >= c.start && fr < c.end());
    anchored.or_else(|| tl.clip_at(fr))
}

/// A rect given in fractions of a clip's picture, in fractions of the output frame.
pub fn place_rect(r: &Rect, content: &Rect) -> Rect {
    Rect { x: content.x + r.x * content.w, y: content.y + r.y * content.h, w: r.w * content.w, h: r.h * content.h }
}

// ------------------------------------------------------------------ drawing

fn color_of(c: Option<&str>, st: &Style) -> [u8; 3] {
    match c {
        None | Some("brand") => st.accent,
        Some(c) => crate::overlay::rgb_of(c),
    }
}

/// 0→1 over the first `d` seconds, 1→0 over the last `d`.
fn fade_env(t: f64, len: f64, d_in: f64, d_out: f64) -> f64 {
    let a = if d_in > 0.0 { (t / d_in).clamp(0.0, 1.0) } else { 1.0 };
    let b = if d_out > 0.0 { ((len - t) / d_out).clamp(0.0, 1.0) } else { 1.0 };
    a.min(b)
}

const ENTER: f64 = 0.35;
const FADE: f64 = 0.2;

/// Keycap label for a key name; symbols when the font has them.
pub fn key_label(name: &str, font: &FontArc) -> String {
    let n = name.trim();
    let (sym, word) = match n.to_ascii_lowercase().as_str() {
        "cmd" | "command" | "meta" => ("⌘", "cmd"),
        "shift" => ("⇧", "shift"),
        "alt" | "option" | "opt" => ("⌥", "alt"),
        "ctrl" | "control" => ("⌃", "ctrl"),
        "enter" | "return" => ("↵", "enter"),
        "tab" => ("⇥", "tab"),
        "backspace" | "delete" => ("⌫", "del"),
        "up" => ("↑", "up"),
        "down" => ("↓", "down"),
        "left" => ("←", "left"),
        "right" => ("→", "right"),
        "esc" | "escape" => ("esc", "esc"),
        "space" => ("space", "space"),
        _ => {
            return if n.chars().count() == 1 { n.to_uppercase() } else { n.to_string() };
        }
    };
    if sym.chars().all(|c| font.glyph_id(c).0 != 0) {
        sym.to_string()
    } else {
        word.to_string()
    }
}

/// Pointer outline, tip at (0, 0), 1 unit tall.
const POINTER: &[(f64, f64)] =
    &[(0.0, 0.0), (0.0, 0.80), (0.19, 0.63), (0.33, 0.95), (0.46, 0.89), (0.32, 0.58), (0.57, 0.58)];

/// Pointer position (frame fractions) at `t` seconds into the layer, and seconds since the
/// latest click (if any).
pub fn cursor_at(path: &[CursorKey], t: f64) -> ((f64, f64), Option<f64>) {
    let Some(first) = path.first() else { return ((0.5, 0.5), None) };
    let mut pos = (first.x, first.y);
    for w in path.windows(2) {
        if t >= w[1].t {
            pos = (w[1].x, w[1].y);
        } else if t > w[0].t {
            let u = effects::ease(Ease::InOutCubic, (t - w[0].t) / (w[1].t - w[0].t).max(1e-6));
            pos = (w[0].x + (w[1].x - w[0].x) * u, w[0].y + (w[1].y - w[0].y) * u);
            break;
        }
    }
    let click = path
        .iter()
        .filter(|k| k.click && k.t <= t)
        .map(|k| t - k.t)
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.min(v))));
    (pos, click)
}

/// Draw every annotation active at timeline frame `fr`.
pub fn draw_frame(cv: &mut Canvas, tl: &Timeline, layers: &[&TLayer], st: &Style, fr: i64) {
    let (w, h) = (cv.w as f64, cv.h as f64);
    let k = st.k;
    let active: Vec<&&TLayer> = layers.iter().filter(|l| fr >= l.start && fr < l.end).collect();
    let ctx = |lay: &TLayer, follow: Option<bool>| -> (Map, Rect) {
        let c = clip_under(tl, lay, fr);
        // screen coordinates with `follow: false`; otherwise relative to the clip's picture
        // (inside its screen frame, if any) and moving with its zoom
        let (map, content) = match c {
            Some(c) if follow != Some(false) => (zoom_map(c, tl, fr, w, h), c.content.unwrap_or(FULL)),
            _ => (Map::IDENTITY, FULL),
        };
        (map, content)
    };
    let times = |lay: &TLayer| -> (f64, f64) { (tl.secs(fr - lay.start), tl.secs(lay.end - lay.start)) };
    // a rect in clip fractions → on-screen pixels
    let px_rect = |r: &Rect, map: &Map, content: &Rect, grow: f64| -> (f64, f64, f64, f64) {
        let r = place_rect(r, content);
        let (x0, y0) = map.pt(r.x * w, r.y * h);
        (x0 - grow, y0 - grow, r.w * w * map.s + 2.0 * grow, r.h * h * map.s + 2.0 * grow)
    };

    // spotlights: one shared dim layer with a hole per spotlight
    let mut holes: Vec<((f64, f64, f64, f64), f64, f64, f64)> = vec![]; // rect px, radius, feather, env
    let mut dim: f64 = 0.0;
    for lay in &active {
        if let Layer::Spotlight { rect, dim: d, feather, radius, follow, .. } = &lay.layer {
            let (map, content) = ctx(lay, *follow);
            let (t, len) = times(lay);
            let env = fade_env(t, len, FADE, FADE);
            dim = dim.max(d.unwrap_or(0.6) * env);
            holes.push((
                px_rect(rect, &map, &content, 4.0 * k),
                radius.unwrap_or(8.0) * k * map.s,
                (feather.unwrap_or(12.0) * k).max(0.5),
                env,
            ));
        }
    }
    if dim > 0.003 {
        let a8 = (dim * 255.0).round() as u8;
        for p in cv.px.chunks_exact_mut(4) {
            p.copy_from_slice(&[0, 0, 0, a8]);
        }
        // (cards are drawn after this, so a card and a spotlight can share a frame)
        let max_env = holes.iter().map(|h| h.3).fold(0.0, f64::max).max(1e-6);
        for (r, _, feather, _) in &holes {
            let (x0, y0) = ((r.0 - feather).floor().max(0.0) as i64, (r.1 - feather).floor().max(0.0) as i64);
            let x1 = ((r.0 + r.2 + feather).ceil() as i64).min(cv.w as i64);
            let y1 = ((r.1 + r.3 + feather).ceil() as i64).min(cv.h as i64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
                    let light = holes
                        .iter()
                        .map(|(r, rad, fe, env)| {
                            (1.0 - (sd_rrect(fx, fy, r.0, r.1, r.2, r.3, *rad) / fe).clamp(0.0, 1.0)) * env / max_env
                        })
                        .fold(0.0, f64::max);
                    let i = ((y as u32 * cv.w + x as u32) * 4) as usize;
                    cv.px[i + 3] = (dim * (1.0 - light) * 255.0).round() as u8;
                }
            }
        }
    }

    // cards (text layers with their own background) go under the other annotations
    for lay in &active {
        if let (Layer::Text { template, text, subtitle, params, look, .. }, Some(cards)) = (&lay.layer, &st.cards) {
            let (t, len) = times(lay);
            // a card that opens the video is complete on the first frame (no blank opening
            // frame, and a usable poster): its entrance is treated as already played
            let lead = if lay.start == 0 { 1.0 } else { 0.0 };
            let (t, len) = (t + lead, len + lead);
            let empty = serde_json::Map::new();
            let params = params.as_ref().unwrap_or(&empty);
            let start = tl.secs(lay.start);
            // reveal times are written on the edit; the card wants seconds since it started
            let reveal = params
                .get("reveal")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| tl.from_edit(x) - start + lead).collect())
                .unwrap_or_default();
            crate::cards::draw(
                cv,
                cards,
                &crate::cards::CardInput {
                    template,
                    text: text.as_deref(),
                    subtitle: subtitle.as_deref(),
                    params,
                    position: look.position.as_ref(),
                    t,
                    len,
                    reveal,
                    safe: st.safe,
                },
            );
        }
    }

    for lay in &active {
        let (t, len) = times(lay);
        match &lay.layer {
            Layer::Highlight { rect, style, color, width, radius, enter, follow, .. } => {
                let (map, content) = ctx(lay, *follow);
                let col = color_of(color.as_deref(), st);
                let sw = (width.unwrap_or(4.0) * k).max(1.0);
                let rad = radius.unwrap_or(8.0) * k * map.s;
                let enter = enter.unwrap_or_default();
                let out = fade_env(t, len, 0.0, FADE);
                let (a_in, prog) = match enter {
                    AnnotEnter::Draw => (1.0, effects::ease(Ease::OutCubic, t / ENTER.min(len / 2.0).max(1e-3))),
                    AnnotEnter::Fade => ((t / FADE).clamp(0.0, 1.0), 1.0),
                    AnnotEnter::None => (1.0, 1.0),
                };
                match style.unwrap_or_default() {
                    HighlightStyle::Outline => {
                        // the stroke sits just outside the region
                        let r = px_rect(rect, &map, &content, sw / 2.0 + 2.0 * k);
                        let (cx, cy) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
                        let bb = (r.0 - sw, r.1 - sw, r.0 + r.2 + sw, r.1 + r.3 + sw);
                        cv.shape(bb, col, a_in * out, |x, y| {
                            let c = aa(sd_rrect(x, y, r.0, r.1, r.2, r.3, rad + sw / 2.0).abs() - sw / 2.0);
                            if prog >= 1.0 || c <= 0.0 {
                                return c;
                            }
                            // sweep clockwise from the top-left corner
                            let ang = ((y - cy) / r.3).atan2((x - cx) / r.2);
                            let frac = ((ang + std::f64::consts::PI * 0.75).rem_euclid(std::f64::consts::TAU))
                                / std::f64::consts::TAU;
                            c * ((prog - frac) * 40.0).clamp(0.0, 1.0)
                        });
                    }
                    HighlightStyle::Fill => {
                        let r = px_rect(rect, &map, &content, 2.0 * k);
                        let a = if enter == AnnotEnter::Draw { prog } else { a_in };
                        cv.shape((r.0, r.1, r.0 + r.2, r.1 + r.3), col, 0.3 * a * out, |x, y| {
                            aa(sd_rrect(x, y, r.0, r.1, r.2, r.3, rad))
                        });
                    }
                    HighlightStyle::Underline => {
                        let r = px_rect(rect, &map, &content, 0.0);
                        let (y0, x1) = (r.1 + r.3 + 2.0 * k, r.0 + r.2 * prog);
                        cv.shape((r.0, y0, x1, y0 + sw), col, a_in * out, |x, y| {
                            aa(sd_rrect(x, y, r.0, y0, (x1 - r.0).max(0.01), sw, sw / 2.0))
                        });
                    }
                }
            }
            Layer::Arrow { from, to, label, color, width, size, follow, .. } => {
                let (map, content) = ctx(lay, *follow);
                let col = color_of(color.as_deref(), st);
                let sw = (width.unwrap_or(6.0) * k).max(1.5);
                let alpha = fade_env(t, len, 0.0, FADE);
                let prog = effects::ease(Ease::OutCubic, t / ENTER.min(len / 2.0).max(1e-3));
                let pt = |p: &cutline_core::model::Point| {
                    let r = place_rect(&Rect { x: p.x, y: p.y, w: 0.0, h: 0.0 }, &content);
                    map.pt(r.x * w, r.y * h)
                };
                let (a, b_full) = (pt(from), pt(to));
                let b = (a.0 + (b_full.0 - a.0) * prog, a.1 + (b_full.1 - a.1) * prog);
                let len_px = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
                let full_len = ((b_full.0 - a.0).powi(2) + (b_full.1 - a.1).powi(2)).sqrt().max(1e-6);
                let dir = ((b_full.0 - a.0) / full_len, (b_full.1 - a.1) / full_len);
                let nrm = (-dir.1, dir.0);
                let head = (3.4 * sw).min(len_px);
                let base = (b.0 - dir.0 * head, b.1 - dir.1 * head);
                let hw = 1.9 * sw;
                let tri = [b, (base.0 + nrm.0 * hw, base.1 + nrm.1 * hw), (base.0 - nrm.0 * hw, base.1 - nrm.1 * hw)];
                let sd = |x: f64, y: f64| -> f64 {
                    let shaft = if len_px > head { sd_segment(x, y, a, base) - sw / 2.0 } else { f64::INFINITY };
                    shaft.min(if head > 0.5 { sd_polygon(x, y, &tri) } else { f64::INFINITY })
                };
                let pad = hw + 4.0 * k + 2.0;
                let bb = (a.0.min(b.0) - pad, a.1.min(b.1) - pad, a.0.max(b.0) + pad, a.1.max(b.1) + pad);
                // dark edge for contrast on any background, then the arrow
                cv.shape(bb, [0, 0, 0], 0.45 * alpha, |x, y| aa(sd(x, y) - 1.5 * k.max(0.5)));
                cv.shape(bb, col, alpha, |x, y| aa(sd(x, y)));
                if let (Some(text), Some(font)) = (label.as_deref().filter(|s| !s.is_empty()), st.font.as_ref()) {
                    let fs = (size.unwrap_or(34.0) * k).max(6.0);
                    let tw = text_width(font, fs, text);
                    let (padx, pady) = (0.45 * fs, 0.28 * fs);
                    let (bw, bh) = (tw + 2.0 * padx, fs + 2.0 * pady);
                    let gap = 0.35 * fs;
                    // the label sits behind the tail, clear of the shaft
                    let (mut bx, mut by) = if dir.0.abs() >= dir.1.abs() {
                        (if dir.0 >= 0.0 { a.0 - gap - bw } else { a.0 + gap }, a.1 - bh / 2.0)
                    } else {
                        (a.0 - bw / 2.0, if dir.1 >= 0.0 { a.1 - gap - bh } else { a.1 + gap })
                    };
                    bx = bx.clamp(2.0, (w - bw - 2.0).max(2.0));
                    by = by.clamp(2.0, (h - bh - 2.0).max(2.0));
                    let la = alpha * (t / FADE).clamp(0.0, 1.0);
                    cv.shape((bx, by, bx + bw, by + bh), [16, 18, 24], 0.86 * la, |x, y| {
                        aa(sd_rrect(x, y, bx, by, bw, bh, bh * 0.3))
                    });
                    let sf = font.as_scaled(PxScale::from(fs as f32));
                    let baseline = by + pady + (fs + sf.ascent() as f64 + sf.descent() as f64) / 2.0;
                    cv.text(font, fs, bx + padx, baseline, text, [255, 255, 255], la);
                }
            }
            Layer::Keys { keys, position, size, .. } => {
                let Some(font) = st.font.as_ref() else { continue };
                let ch = (size.unwrap_or(64.0) * k).max(10.0);
                let fs = ch * 0.46;
                let gap = ch * 0.2;
                let labels: Vec<String> = keys.iter().map(|n| key_label(n, font)).collect();
                let widths: Vec<f64> = labels.iter().map(|l| (text_width(font, fs, l) + 0.7 * ch).max(ch)).collect();
                let total = widths.iter().sum::<f64>() + gap * (widths.len().saturating_sub(1)) as f64;
                let (cx, cy) = position_center(position.as_ref(), st.safe, (w, h), (total, ch), "bottom");
                let alpha = fade_env(t, len, FADE, FADE);
                let rise = (1.0 - effects::ease(Ease::OutCubic, t / FADE)) * 0.25 * ch;
                let mut x = cx - total / 2.0;
                let y = cy - ch / 2.0 + rise;
                let sf = font.as_scaled(PxScale::from(fs as f32));
                let baseline = y + (ch * 0.94 + sf.ascent() as f64 + sf.descent() as f64) / 2.0;
                for (label, kw) in labels.iter().zip(&widths) {
                    let r = ch * 0.2;
                    let bb = (x - 2.0, y - 2.0, x + kw + 2.0, y + ch + 2.0);
                    // base (the key's depth), border, face
                    cv.shape(bb, [10, 11, 14], 0.95 * alpha, |px, py| aa(sd_rrect(px, py, x, y, *kw, ch, r)));
                    cv.shape(bb, [86, 92, 104], alpha, |px, py| aa(sd_rrect(px, py, x, y, *kw, ch * 0.94, r)));
                    let b = (1.5 * k).max(1.0);
                    cv.shape(bb, [32, 35, 43], alpha, |px, py| {
                        aa(sd_rrect(px, py, x + b, y + b, kw - 2.0 * b, ch * 0.94 - 2.0 * b, (r - b).max(0.0)))
                    });
                    let tw = text_width(font, fs, label);
                    cv.text(font, fs, x + (kw - tw) / 2.0, baseline, label, [245, 246, 248], alpha);
                    x += kw + gap;
                }
            }
            Layer::Mask { rect, color, text, text_color, text_size, .. } => {
                // a filled mask on the clip's own picture: it moves with the frame and the zoom
                let (map, content) = ctx(lay, Some(true));
                let r = px_rect(rect, &map, &content, 0.0);
                let col = crate::overlay::rgb_of(color.as_deref().unwrap_or("#000000"));
                cv.shape((r.0, r.1, r.0 + r.2, r.1 + r.3), col, 1.0, |x, y| {
                    aa(sd_rrect(x, y, r.0, r.1, r.2, r.3, 0.0))
                });
                if let (Some(tx), Some(font)) = (text.as_deref().filter(|s| !s.is_empty()), st.font.as_ref()) {
                    let fs = text_size.map(|v| v * k * map.s).unwrap_or(0.6 * r.3).max(4.0);
                    let sf = font.as_scaled(PxScale::from(fs as f32));
                    let baseline = r.1 + (r.3 + sf.ascent() as f64 + sf.descent() as f64) / 2.0;
                    let tc = crate::overlay::rgb_of(text_color.as_deref().unwrap_or("#9AA0A6"));
                    cv.text(font, fs, r.0 + 0.3 * fs, baseline, tx, tc, 1.0);
                }
            }
            Layer::Badge { text, position, size, .. } => {
                let Some(font) = st.font.as_ref() else { continue };
                let fs = (size.unwrap_or(30.0) * k).max(6.0);
                let tw = text_width(font, fs, text);
                let (bw, bh) = (tw + 0.9 * fs, 1.7 * fs);
                let (cx, cy) = position_center(position.as_ref(), st.safe, (w, h), (bw, bh), "top_right");
                let alpha = fade_env(t, len, FADE, FADE);
                let (x0, y0) = (cx - bw / 2.0, cy - bh / 2.0);
                cv.shape((x0, y0, x0 + bw, y0 + bh), [16, 18, 24], 0.82 * alpha, |px, py| {
                    aa(sd_rrect(px, py, x0, y0, bw, bh, bh / 2.0))
                });
                let sf = font.as_scaled(PxScale::from(fs as f32));
                let baseline = cy + (sf.ascent() as f64 + sf.descent() as f64) / 2.0;
                cv.text(font, fs, x0 + 0.45 * fs, baseline, text, [255, 255, 255], alpha);
            }
            Layer::Cursor { path, size, click_ripple, color, follow, .. } => {
                let (map, content) = ctx(lay, *follow);
                let ((fx, fy), click) = cursor_at(path, t);
                let r = place_rect(&Rect { x: fx, y: fy, w: 0.0, h: 0.0 }, &content);
                let (x, y) = map.pt(r.x * w, r.y * h);
                let alpha = fade_env(t, len, 0.15, 0.15);
                let unit = 28.0 * size.unwrap_or(1.0) * k;
                if let Some(dt) = click.filter(|dt| *dt < 0.5 && click_ripple.unwrap_or(true)) {
                    let u = dt / 0.5;
                    let rad = unit * (0.3 + 1.1 * effects::ease(Ease::OutCubic, u));
                    let ring = (unit * 0.11).max(1.5);
                    let col = color_of(color.as_deref(), st);
                    let bb = (x - rad - ring - 1.0, y - rad - ring - 1.0, x + rad + ring + 1.0, y + rad + ring + 1.0);
                    cv.shape(bb, col, 0.85 * (1.0 - u) * alpha, |px, py| {
                        aa((((px - x).powi(2) + (py - y).powi(2)).sqrt() - rad).abs() - ring / 2.0)
                    });
                    cv.shape(bb, col, 0.22 * (1.0 - u) * alpha, |px, py| {
                        aa(((px - x).powi(2) + (py - y).powi(2)).sqrt() - rad)
                    });
                }
                // the pointer dips for a moment on a click
                let press = click.filter(|dt| *dt < 0.14).map(|dt| 0.86 + 0.14 * (dt / 0.14)).unwrap_or(1.0);
                let u = unit * press;
                let pts: Vec<(f64, f64)> = POINTER.iter().map(|(px, py)| (x + px * u, y + py * u)).collect();
                let edge = (u * 0.06).max(1.0);
                let bb = (x - edge - 3.0, y - edge - 3.0, x + u * 0.6 + edge + 3.0, y + u + edge + 5.0);
                cv.shape(bb, [0, 0, 0], 0.28 * alpha, |px, py| {
                    aa(sd_polygon(px, py - 2.0 * k.max(0.5), &pts) - edge - 1.0)
                });
                cv.shape(bb, [255, 255, 255], alpha, |px, py| aa(sd_polygon(px, py, &pts) - edge));
                cv.shape(bb, [18, 18, 20], alpha, |px, py| aa(sd_polygon(px, py, &pts)));
            }
            _ => {}
        }
    }
}

/// Center of a box of `size` placed by a position preset inside the safe rectangle.
pub fn position_center(
    pos: Option<&Position>,
    safe: (f64, f64, f64, f64),
    (w, h): (f64, f64),
    (bw, bh): (f64, f64),
    default: &str,
) -> (f64, f64) {
    let (sx, sy, sw, sh) = safe;
    let preset = match pos {
        Some(Position::Point(p)) => return (p.x * w, p.y * h),
        Some(Position::Preset(s)) => s.as_str(),
        None => default,
    };
    let (l, r, cx) = (sx + bw / 2.0, sx + sw - bw / 2.0, w / 2.0);
    let (t, b, cy) = (sy + bh / 2.0, sy + sh - bh / 2.0, h / 2.0);
    match preset {
        "top_left" => (l, t),
        "top_right" => (r, t),
        "bottom_left" => (l, b),
        "bottom_right" => (r, b),
        "top" => (cx, t),
        "bottom" => (cx, b),
        "left" => (l, cy),
        "right" => (r, cy),
        "lower_mid" => (cx, sy + sh * 0.72),
        "upper_mid" => (cx, sy + sh * 0.25),
        _ => (cx, cy),
    }
}

// ------------------------------------------------------------------ rendering

/// Runs of frames with at least one annotation on screen: `(start, end, layers)`.
pub fn runs(tl: &Timeline) -> Vec<(i64, i64, Vec<&TLayer>)> {
    let mut ls: Vec<&TLayer> = tl.layers.iter().filter(|l| l.layer.is_annotation() || l.layer.is_card()).collect();
    ls.sort_by_key(|l| (l.start, l.index));
    let mut out: Vec<(i64, i64, Vec<&TLayer>)> = vec![];
    for l in ls {
        match out.last_mut() {
            Some(r) if l.start <= r.1 => {
                r.1 = r.1.max(l.end);
                r.2.push(l);
            }
            _ => out.push((l.start, l.end, vec![l])),
        }
    }
    // draw order: by layer index (spotlights are composited first inside `draw_frame`)
    for r in &mut out {
        r.2.sort_by_key(|l| (l.z, l.index));
    }
    out
}

/// Render every annotation run to a cached alpha video (QuickTime Animation, lossless).
pub fn render(
    tc: &cutline_deps::Toolchain,
    tl: &Timeline,
    g: &Geometry,
    st: &Style,
    cache_dir: &Path,
) -> Result<Vec<AnnotRender>> {
    let mut out = vec![];
    for (a, b, layers) in runs(tl) {
        // everything that can change a pixel: the layers, the clips they may follow, the look
        let clips: Vec<&TClip> = tl.clips.iter().filter(|c| c.end() > a && c.start < b).collect();
        let key = cutline_media::hash_key(&[
            serde_json::to_string(&layers).unwrap_or_default().as_bytes(),
            serde_json::to_string(&clips).unwrap_or_default().as_bytes(),
            format!("{}x{}@{} {a}-{b} {:?} {:?} {}", g.width, g.height, g.fps, st.accent, st.safe, st.k).as_bytes(),
            st.look_key.as_bytes(),
            cutline_core::VERSION.as_bytes(),
            b"annot-v2",
        ]);
        let path = cache_dir.join(format!("{key}.mov"));
        if !path.exists() {
            std::fs::create_dir_all(cache_dir)?;
            let tmp = cache_dir.join(format!("{key}.tmp.mov"));
            let mut child = std::process::Command::new(&tc.ffmpeg)
                .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba"])
                .args(["-s", &format!("{}x{}", g.width, g.height), "-r", &g.fps.ff(), "-i", "-"])
                .args(["-c:v", "qtrle", "-pix_fmt", "argb", "-an"])
                .arg(&tmp)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .context("spawning ffmpeg for annotations")?;
            {
                let mut stdin = child.stdin.take().context("ffmpeg stdin")?;
                let mut cv = Canvas::new(g.width, g.height);
                for fr in a..b {
                    cv.clear();
                    draw_frame(&mut cv, tl, &layers, st, fr);
                    stdin.write_all(&cv.px).context("writing annotation frames")?;
                }
            }
            let o = child.wait_with_output()?;
            if !o.status.success() {
                anyhow::bail!("ffmpeg failed (annotations): {}", String::from_utf8_lossy(&o.stderr));
            }
            std::fs::rename(&tmp, &path)?;
        }
        out.push(AnnotRender { path, start: tl.secs(a), end: tl.secs(b) });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_rect_distance() {
        assert!(sd_rrect(50.0, 50.0, 0.0, 0.0, 100.0, 100.0, 10.0) < -40.0);
        assert!((sd_rrect(110.0, 50.0, 0.0, 0.0, 100.0, 100.0, 10.0) - 10.0).abs() < 1e-9);
        // the corner is rounded: the square's corner point is outside
        assert!(sd_rrect(0.5, 0.5, 0.0, 0.0, 100.0, 100.0, 10.0) > 0.0);
    }

    #[test]
    fn cursor_moves_between_keys_and_reports_clicks() {
        let path = vec![
            CursorKey { t: 0.0, x: 0.1, y: 0.1, click: false },
            CursorKey { t: 1.0, x: 0.5, y: 0.3, click: true },
            CursorKey { t: 2.0, x: 0.9, y: 0.3, click: false },
        ];
        assert_eq!(cursor_at(&path, 0.0), ((0.1, 0.1), None));
        let ((x, y), c) = cursor_at(&path, 0.5);
        assert!((x - 0.3).abs() < 1e-9 && (y - 0.2).abs() < 1e-9 && c.is_none());
        let ((x, _), c) = cursor_at(&path, 1.2);
        assert!(x > 0.5 && x < 0.6, "eased start {x}");
        assert!((c.unwrap() - 0.2).abs() < 1e-9);
        assert_eq!(cursor_at(&path, 5.0).0, (0.9, 0.3));
    }

    #[test]
    fn polygon_distance_sign() {
        let sq = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!((sd_polygon(5.0, 5.0, &sq) + 5.0).abs() < 1e-9);
        assert!((sd_polygon(13.0, 5.0, &sq) - 3.0).abs() < 1e-9);
    }
}
