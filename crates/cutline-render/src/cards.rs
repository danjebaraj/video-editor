//! Business card templates (`title_card`, `section_card`, `feature_list`, `stat`,
//! `testimonial`, `compare`, `logo_wall`, `cta_link`): text layers drawn in Rust in the
//! annotation pass, with their own background, images (logo, photo, QR code) and motion.

use crate::annot::{sd_rrect, text_width, Canvas};
use crate::effects::ease;
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use cutline_core::brand::Brand;
use cutline_core::model::{Ease, Position};
use image::RgbaImage;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What the card templates need besides the layer itself.
pub struct CardCtx {
    pub title: FontArc,
    pub body: FontArc,
    pub body_bold: FontArc,
    pub text: [u8; 3],
    pub accent: [u8; 3],
    pub background: [u8; 3],
    pub logo: Option<PathBuf>,
    /// Asset id → file, for images named in params.
    pub assets: BTreeMap<String, PathBuf>,
    images: RefCell<BTreeMap<(PathBuf, u32, u32, bool), Option<RgbaImage>>>,
}

/// A face of `family` at `weight`: a variable font is set to that weight, otherwise the bold
/// or regular file is picked.
pub fn face(db: &cutline_text::FontDb, family: &str, weight: f32) -> Option<FontArc> {
    let e = db.get(family, weight >= 600.0)?;
    let variable = e.path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains("wght"));
    if variable {
        if let Ok(data) = std::fs::read(&e.path) {
            if let Ok(mut f) = ab_glyph::FontVec::try_from_vec(data) {
                use ab_glyph::VariableFont;
                f.set_variation(b"wght", weight);
                return Some(FontArc::new(f));
            }
        }
    }
    Some(e.font.clone())
}

impl CardCtx {
    pub fn new(brand: &Brand, assets: BTreeMap<String, PathBuf>) -> Option<Self> {
        let db = cutline_text::FontDb::bundled().ok()?;
        Some(Self {
            title: face(&db, &brand.fonts.title, 700.0)?,
            body: face(&db, &brand.fonts.body, 400.0)?,
            body_bold: face(&db, &brand.fonts.body, 650.0)?,
            text: crate::overlay::rgb_of(&brand.colors.text),
            accent: crate::overlay::rgb_of(&brand.colors.accent),
            background: crate::overlay::rgb_of(&brand.colors.background),
            logo: brand.file(&brand.logo).filter(|p| p.exists()),
            assets,
            images: RefCell::new(BTreeMap::new()),
        })
    }

    /// An image scaled to fit `max_w × max_h` (cached). `white`: recolored to a white
    /// silhouette (logos with transparency) or to grey (opaque images).
    fn image(&self, path: &Path, max_w: f64, max_h: f64, white: bool) -> Option<RgbaImage> {
        let key = (path.to_path_buf(), max_w.round() as u32, max_h.round() as u32, white);
        if let Some(i) = self.images.borrow().get(&key) {
            return i.clone();
        }
        let out = image::open(path).ok().map(|i| i.to_rgba8()).and_then(|img| {
            let s = (max_w / img.width() as f64).min(max_h / img.height() as f64);
            let (w, h) =
                (((img.width() as f64 * s).round() as u32).max(1), ((img.height() as f64 * s).round() as u32).max(1));
            let mut r = image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3);
            if white {
                let transparent = r.pixels().any(|p| p[3] < 250);
                for p in r.pixels_mut() {
                    if transparent {
                        p.0 = [255, 255, 255, p[3]];
                    } else {
                        let g = (0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64) as u8;
                        p.0 = [g, g, g, p[3]];
                    }
                }
            }
            Some(r)
        });
        self.images.borrow_mut().insert(key, out.clone());
        out
    }

    fn asset_image(&self, id: &str, max_w: f64, max_h: f64, white: bool) -> Option<RgbaImage> {
        self.image(self.assets.get(id)?, max_w, max_h, white)
    }
}

// ------------------------------------------------------------------ helpers

#[inline]
fn aa(d: f64) -> f64 {
    (0.5 - d).clamp(0.0, 1.0)
}

fn blit(cv: &mut Canvas, img: &RgbaImage, x: f64, y: f64, alpha: f64, radius: f64) {
    let (x0, y0) = (x.round() as i64, y.round() as i64);
    let (w, h) = (img.width() as f64, img.height() as f64);
    for (px, py, p) in img.enumerate_pixels() {
        let mut a = p[3] as f64 / 255.0 * alpha;
        if radius > 0.0 {
            a *= aa(sd_rrect(px as f64 + 0.5, py as f64 + 0.5, 0.0, 0.0, w, h, radius));
        }
        cv.blend(x0 + px as i64, y0 + py as i64, [p[0], p[1], p[2]], a);
    }
}

/// Greedy word wrap; `\n` forces a break.
pub fn wrap(font: &FontArc, size: f64, text: &str, max_w: f64) -> Vec<String> {
    let mut lines = vec![];
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if !cur.is_empty() && text_width(font, size, &cand) > max_w {
                lines.push(std::mem::take(&mut cur));
                cur = word.to_string();
            } else {
                cur = cand;
            }
        }
        lines.push(cur);
    }
    lines
}

const LINE: f64 = 1.22;

/// The largest size ≤ `size` at which `text` wraps into `max_lines` lines of `max_w`.
pub fn fit(font: &FontArc, text: &str, size: f64, max_w: f64, max_lines: usize) -> (f64, Vec<String>) {
    let mut s = size;
    loop {
        let lines = wrap(font, s, text, max_w);
        let widest = lines.iter().map(|l| text_width(font, s, l)).fold(0.0, f64::max);
        if (lines.len() <= max_lines && widest <= max_w) || s <= size * 0.45 {
            return (s, lines);
        }
        s *= 0.94;
    }
}

/// Draw lines from `y` (top) down; `align` 0 = left at `x`, 0.5 = centred on `x`, 1 = right.
/// Returns the height used.
#[allow(clippy::too_many_arguments)]
fn lines(
    cv: &mut Canvas,
    font: &FontArc,
    size: f64,
    ls: &[String],
    x: f64,
    y: f64,
    align: f64,
    rgb: [u8; 3],
    alpha: f64,
) -> f64 {
    let sf = font.as_scaled(PxScale::from(size as f32));
    let lh = size * LINE;
    for (i, l) in ls.iter().enumerate() {
        let w = text_width(font, size, l);
        let base = y + i as f64 * lh + (lh + sf.ascent() as f64 + sf.descent() as f64) / 2.0;
        cv.text(font, size, x - w * align, base, l, rgb, alpha);
    }
    ls.len() as f64 * lh
}

/// Fade and rise in: (alpha, y offset) for an element appearing at `t0`.
fn appear(t: f64, t0: f64, rise: f64) -> (f64, f64) {
    let u = ease(Ease::OutCubic, (t - t0) / 0.45);
    if t < t0 {
        (0.0, rise)
    } else {
        (u, (1.0 - u) * rise)
    }
}

fn s<'a>(p: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    p.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}
fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}
fn mix(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    [0, 1, 2].map(|i| (a[i] as f64 + (b[i] as f64 - a[i] as f64) * t).round() as u8)
}

/// What a card layer says, for one frame.
pub struct CardInput<'a> {
    pub template: &'a str,
    pub text: Option<&'a str>,
    pub subtitle: Option<&'a str>,
    pub params: &'a Map<String, Value>,
    pub position: Option<&'a Position>,
    /// Seconds since the layer started, and its length.
    pub t: f64,
    pub len: f64,
    /// Reveal times (seconds since the layer started) for `feature_list`.
    pub reveal: Vec<f64>,
    /// Safe rectangle in pixels of the canvas.
    pub safe: (f64, f64, f64, f64),
}

fn background(cv: &mut Canvas, ctx: &CardCtx, mode: &str, alpha: f64) {
    let (w, h) = (cv.w, cv.h);
    let (top, bottom, a) = match mode {
        "none" => return,
        "scrim" => ([8, 10, 14], [8, 10, 14], 0.74),
        "brand" => (mix(ctx.background, [255, 255, 255], 0.12), ctx.background, 1.0),
        c => (crate::overlay::rgb_of(c), crate::overlay::rgb_of(c), 1.0),
    };
    let a8 = (a * alpha * 255.0).round() as u8;
    let den = (w + h).max(1) as f64;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let c = mix(top, bottom, (x + y) as f64 / den);
            cv.px[i..i + 3].copy_from_slice(&c);
            cv.px[i + 3] = a8;
        }
    }
}

/// Draw one card for one frame.
pub fn draw(cv: &mut Canvas, ctx: &CardCtx, c: &CardInput) {
    let (w, h) = (cv.w as f64, cv.h as f64);
    let u = w.min(h) / 100.0;
    let (sx, sy, sw, sh) = c.safe;
    let p = c.params;
    let fade = (c.t / 0.3).clamp(0.0, 1.0).min(((c.len - c.t) / 0.3).clamp(0.0, 1.0));
    let default_bg = if matches!(c.template, "feature_list" | "stat") { "scrim" } else { "brand" };
    let bg = s(p, "background").unwrap_or(default_bg);
    background(cv, ctx, bg, fade);
    // text on a light custom background would need a dark color; params.color overrides
    let ink = s(p, "color").map(crate::overlay::rgb_of).unwrap_or(ctx.text);
    let accent = s(p, "accent").map(crate::overlay::rgb_of).unwrap_or(ctx.accent);
    let muted = mix(ink, if bg == "brand" { ctx.background } else { [128, 128, 128] }, 0.38);
    let (cx, cy) = (sx + sw / 2.0, sy + sh / 2.0);
    match c.template {
        "title_card" => {
            let title = c.text.unwrap_or("");
            let (ts, tl) = fit(&ctx.title, title, 11.0 * u, sw * 0.86, 2);
            let sub = c.subtitle.map(|t| fit(&ctx.body, t, 4.2 * u, sw * 0.8, 2));
            let logo = (p.get("logo") != Some(&Value::Bool(false)))
                .then(|| ctx.logo.as_ref().and_then(|l| ctx.image(l, 26.0 * u, 11.0 * u, false)))
                .flatten();
            let logo_h = logo.as_ref().map(|i| i.height() as f64 + 5.0 * u).unwrap_or(0.0);
            let title_h = tl.len() as f64 * ts * LINE;
            let sub_h = sub.as_ref().map(|(s, l)| 3.4 * u + l.len() as f64 * s * LINE).unwrap_or(0.0);
            let mut y = cy - (logo_h + title_h + sub_h) / 2.0;
            if let Some(img) = &logo {
                let (a, dy) = appear(c.t, 0.0, 1.5 * u);
                blit(cv, img, cx - img.width() as f64 / 2.0, y + dy, a * fade, 0.0);
                y += logo_h;
            }
            let (a, dy) = appear(c.t, 0.1, 2.0 * u);
            y += lines(cv, &ctx.title, ts, &tl, cx, y + dy, 0.5, ink, a * fade);
            if let Some((ss, sl)) = &sub {
                // accent rule that grows from the centre, then the promise
                let (a, dy) = appear(c.t, 0.3, 1.5 * u);
                let rw = 9.0 * u * a;
                let ry = y + 1.3 * u;
                cv.shape((cx - rw, ry, cx + rw, ry + 0.5 * u), accent, fade, |_, _| 1.0);
                lines(cv, &ctx.body, *ss, sl, cx, y + 3.4 * u + dy, 0.5, muted, a * fade);
            }
        }
        "section_card" => {
            let x = sx + sw * 0.08;
            let num = p.get("number").map(|v| match v {
                Value::Number(n) => format!("{:02}", n.as_f64().unwrap_or(0.0) as i64),
                Value::String(s) => s.clone(),
                _ => String::new(),
            });
            let (ts, tl) = fit(&ctx.title, c.text.unwrap_or(""), 9.0 * u, sw * 0.8, 2);
            let sub = c.subtitle.map(|t| fit(&ctx.body, t, 4.0 * u, sw * 0.8, 2));
            let num_h = if num.is_some() { 6.0 * u } else { 0.0 };
            let total = num_h
                + tl.len() as f64 * ts * LINE
                + sub.as_ref().map(|(s, l)| 1.5 * u + l.len() as f64 * s * LINE).unwrap_or(0.0);
            let mut y = cy - total / 2.0;
            // accent bar draws down the left edge of the block
            let grow = ease(Ease::OutCubic, c.t / 0.5);
            cv.shape((x - 3.2 * u, y, x - 2.4 * u, y + total * grow), accent, fade, |_, _| 1.0);
            if let Some(n) = &num {
                let (a, dy) = appear(c.t, 0.0, 1.5 * u);
                lines(cv, &ctx.body_bold, 3.8 * u, &[n.clone()], x, y + dy, 0.0, accent, a * fade);
                y += num_h;
            }
            let (a, dy) = appear(c.t, 0.1, 2.0 * u);
            y += lines(cv, &ctx.title, ts, &tl, x, y + dy, 0.0, ink, a * fade);
            if let Some((ss, sl)) = &sub {
                let (a, dy) = appear(c.t, 0.25, 1.5 * u);
                lines(cv, &ctx.body, *ss, sl, x, y + 1.5 * u + dy, 0.0, muted, a * fade);
            }
        }
        "feature_list" => {
            let items = strings(p.get("items"));
            let fs = 4.6 * u;
            let max_w = sw * 0.72;
            let wrapped: Vec<Vec<String>> = items.iter().map(|i| wrap(&ctx.body, fs, i, max_w - 6.0 * u)).collect();
            let head = c.text.map(|t| fit(&ctx.title, t, 6.6 * u, max_w, 2));
            let head_h = head.as_ref().map(|(s, l)| l.len() as f64 * s * LINE + 3.0 * u).unwrap_or(0.0);
            let gap = 2.2 * u;
            let list_h: f64 = wrapped.iter().map(|l| l.len() as f64 * fs * LINE + gap).sum::<f64>() - gap;
            let widest = wrapped
                .iter()
                .flatten()
                .map(|l| text_width(&ctx.body, fs, l) + 6.0 * u)
                .chain(head.iter().flat_map(|(s, l)| l.iter().map(|x| text_width(&ctx.title, *s, x))))
                .fold(0.0, f64::max);
            let x = cx - widest / 2.0;
            let mut y = cy - (head_h + list_h) / 2.0;
            if let Some((hs, hl)) = &head {
                let (a, dy) = appear(c.t, 0.0, 1.5 * u);
                lines(cv, &ctx.title, *hs, hl, x, y + dy, 0.0, ink, a * fade);
                y += head_h;
            }
            for (i, l) in wrapped.iter().enumerate() {
                let t0 = c.reveal.get(i).copied().unwrap_or(0.4 + 0.5 * i as f64);
                let (a, dy) = appear(c.t, t0, 1.5 * u);
                // check mark in an accent disc
                let (dx, dyc, r) = (x + 2.0 * u, y + fs * LINE / 2.0 + dy, 1.7 * u);
                cv.shape((dx - r - 1.0, dyc - r - 1.0, dx + r + 1.0, dyc + r + 1.0), accent, a * fade, |px, py| {
                    aa(((px - dx).powi(2) + (py - dyc).powi(2)).sqrt() - r)
                });
                let tick = [(dx - 0.8 * u, dyc), (dx - 0.2 * u, dyc + 0.6 * u), (dx + 0.85 * u, dyc - 0.65 * u)];
                let dark = mix(accent, [0, 0, 0], 0.78);
                cv.shape((dx - r, dyc - r, dx + r, dyc + r), dark, a * fade, |px, py| {
                    let d = seg(px, py, tick[0], tick[1]).min(seg(px, py, tick[1], tick[2]));
                    aa(d - 0.22 * u)
                });
                y += lines(cv, &ctx.body, fs, l, x + 6.0 * u, y + dy, 0.0, ink, a * fade) + gap;
            }
        }
        "stat" => {
            let n = p.get("number").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let dec = p.get("decimals").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let shown = n * ease(Ease::OutCubic, (c.t - 0.1) / 0.9);
            let num = format!("{}{}{}", s(p, "prefix").unwrap_or(""), group(shown, dec), s(p, "suffix").unwrap_or(""));
            let full = format!("{}{}{}", s(p, "prefix").unwrap_or(""), group(n, dec), s(p, "suffix").unwrap_or(""));
            // size from the final number so the digits do not jump while counting
            let (ns, _) = fit(&ctx.title, &full, 20.0 * u, sw * 0.86, 1);
            let label = c.subtitle.map(|t| fit(&ctx.body, t, 4.4 * u, sw * 0.8, 2));
            let source = s(p, "source").map(|t| fit(&ctx.body, t, 2.6 * u, sw * 0.8, 1));
            let total = ns * LINE
                + label.as_ref().map(|(s, l)| l.len() as f64 * s * LINE + u).unwrap_or(0.0)
                + source.as_ref().map(|(s, _)| s * LINE + 2.5 * u).unwrap_or(0.0);
            let (pcx, pcy) = crate::annot::position_center(c.position, c.safe, (w, h), (sw * 0.6, total), "center");
            let mut y = pcy - total / 2.0;
            let (a, dy) = appear(c.t, 0.0, 2.0 * u);
            y += lines(cv, &ctx.title, ns, &[num], pcx, y + dy, 0.5, accent, a * fade);
            if let Some((ls, ll)) = &label {
                let (a, dy) = appear(c.t, 0.2, 1.5 * u);
                y += lines(cv, &ctx.body, *ls, ll, pcx, y + u + dy, 0.5, ink, a * fade) + u;
            }
            if let Some((ss, sl)) = &source {
                let (a, _) = appear(c.t, 0.4, 0.0);
                lines(cv, &ctx.body, *ss, sl, pcx, y + 2.5 * u, 0.5, muted, a * fade);
            }
        }
        "testimonial" => {
            let max_w = sw * 0.78;
            let (qs, ql) = fit(&ctx.body, c.text.unwrap_or(""), 5.4 * u, max_w, 5);
            let name = s(p, "name").or(c.subtitle);
            let role: Vec<&str> = [s(p, "title"), s(p, "company")].into_iter().flatten().collect();
            let photo = s(p, "photo").and_then(|id| ctx.asset_image(id, 11.0 * u, 11.0 * u, false));
            let logo = s(p, "logo").and_then(|id| ctx.asset_image(id, 18.0 * u, 6.0 * u, false));
            let quote_h = ql.len() as f64 * qs * LINE;
            let attr_h = if name.is_some() || photo.is_some() { 11.0 * u } else { 0.0 };
            let total = 8.0 * u + quote_h + 4.0 * u + attr_h;
            let x = cx - max_w / 2.0;
            let mut y = cy - total / 2.0;
            // opening quote mark
            let (a, dy) = appear(c.t, 0.0, 1.5 * u);
            cv.text(&ctx.title, 20.0 * u, x - 0.6 * u, y + 13.5 * u + dy, "\u{201C}", accent, a * fade);
            y += 8.0 * u;
            let (a, dy) = appear(c.t, 0.15, 1.5 * u);
            y += lines(cv, &ctx.body, qs, &ql, x, y + dy, 0.0, ink, a * fade) + 4.0 * u;
            let (a, dy) = appear(c.t, 0.4, 1.5 * u);
            let mut ax = x;
            if let Some(img) = &photo {
                let side = img.width().min(img.height()) as f64;
                blit(cv, img, ax, y + dy + (11.0 * u - img.height() as f64) / 2.0, a * fade, side / 2.0);
                ax += img.width() as f64 + 2.5 * u;
            }
            if let Some(n) = name {
                let two = !role.is_empty();
                let ny = y + dy + if two { 1.6 * u } else { 3.6 * u };
                lines(cv, &ctx.body_bold, 3.6 * u, &[n.to_string()], ax, ny, 0.0, ink, a * fade);
                if two {
                    lines(cv, &ctx.body, 2.9 * u, &[role.join(", ")], ax, ny + 4.6 * u, 0.0, muted, a * fade);
                }
            }
            if let Some(img) = &logo {
                blit(
                    cv,
                    img,
                    x + max_w - img.width() as f64,
                    y + dy + (11.0 * u - img.height() as f64) / 2.0,
                    a * fade,
                    0.0,
                );
            }
        }
        "compare" => {
            let side = |k: &str| -> (String, Vec<String>) {
                let o = p.get(k).and_then(|v| v.as_object());
                (
                    o.and_then(|o| o.get("title")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    strings(o.and_then(|o| o.get("items"))),
                )
            };
            let (left, right) = (side("left"), side("right"));
            let head = c.text.map(|t| fit(&ctx.title, t, 6.0 * u, sw * 0.86, 1));
            let head_h = head.as_ref().map(|(s, _)| s * LINE + 3.5 * u).unwrap_or(0.0);
            let gap = 3.0 * u;
            let col_w = ((sw * 0.9 - gap) / 2.0).min(64.0 * u);
            let fs = 3.6 * u;
            let rows = left.1.len().max(right.1.len()) as f64;
            let col_h = 9.0 * u + rows * (fs * LINE + 1.8 * u) + 2.0 * u;
            let mut y = cy - (head_h + col_h) / 2.0;
            if let Some((hs, hl)) = &head {
                let (a, dy) = appear(c.t, 0.0, 1.5 * u);
                lines(cv, &ctx.title, *hs, hl, cx, y + dy, 0.5, ink, a * fade);
                y += head_h;
            }
            for (k, (title, items)) in [left, right].iter().enumerate() {
                let ours = k == 1;
                let x = if ours { cx + gap / 2.0 } else { cx - gap / 2.0 - col_w };
                let (a, dy) = appear(c.t, 0.15 + 0.2 * k as f64, 1.5 * u);
                let panel =
                    if ours { mix(ctx.background, accent, 0.16) } else { mix(ctx.background, [255, 255, 255], 0.05) };
                let r = 1.6 * u;
                cv.shape((x, y + dy, x + col_w, y + dy + col_h), panel, 0.96 * a * fade, |px, py| {
                    aa(sd_rrect(px, py, x, y + dy, col_w, col_h, r))
                });
                if ours {
                    let bw = (0.35 * u).max(1.5);
                    cv.shape((x - bw, y + dy - bw, x + col_w + bw, y + dy + col_h + bw), accent, a * fade, |px, py| {
                        aa(sd_rrect(px, py, x, y + dy, col_w, col_h, r).abs() - bw / 2.0)
                    });
                }
                let (tsz, tl) = fit(&ctx.body_bold, title, 4.2 * u, col_w - 6.0 * u, 1);
                lines(
                    cv,
                    &ctx.body_bold,
                    tsz,
                    &tl,
                    x + 3.0 * u,
                    y + dy + 2.6 * u,
                    0.0,
                    if ours { accent } else { muted },
                    a * fade,
                );
                let mut iy = y + dy + 9.0 * u;
                for it in items {
                    let (mx, my, m) = (x + 4.2 * u, iy + fs * LINE / 2.0, 0.9 * u);
                    let col = if ours { accent } else { muted };
                    let stroke = 0.2 * u;
                    cv.shape((mx - 2.0 * m, my - 2.0 * m, mx + 2.0 * m, my + 2.0 * m), col, a * fade, |px, py| {
                        let d = if ours {
                            seg(px, py, (mx - m, my), (mx - 0.2 * m, my + 0.8 * m)).min(seg(
                                px,
                                py,
                                (mx - 0.2 * m, my + 0.8 * m),
                                (mx + m, my - 0.8 * m),
                            ))
                        } else {
                            seg(px, py, (mx - 0.8 * m, my - 0.8 * m), (mx + 0.8 * m, my + 0.8 * m)).min(seg(
                                px,
                                py,
                                (mx - 0.8 * m, my + 0.8 * m),
                                (mx + 0.8 * m, my - 0.8 * m),
                            ))
                        };
                        aa(d - stroke)
                    });
                    let (isz, il) = fit(&ctx.body, it, fs, col_w - 10.0 * u, 1);
                    lines(
                        cv,
                        &ctx.body,
                        isz,
                        &il,
                        x + 7.0 * u,
                        iy + (fs - isz) * LINE / 2.0,
                        0.0,
                        if ours { ink } else { muted },
                        a * fade,
                    );
                    iy += fs * LINE + 1.8 * u;
                }
            }
        }
        "logo_wall" => {
            let ids = strings(p.get("logos"));
            let mono = p.get("monochrome").and_then(|v| v.as_bool()).unwrap_or(false);
            let head = c.text.map(|t| fit(&ctx.body, t, 4.0 * u, sw * 0.86, 1));
            let head_h = head.as_ref().map(|(s, _)| s * LINE + 4.0 * u).unwrap_or(0.0);
            let n = ids.len().max(1);
            let cols = if w >= h {
                [1, 2, 3, 4, 3, 3, 4, 4, 5, 5, 4, 4][(n - 1).min(11)]
            } else {
                [1, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3][(n - 1).min(11)]
            };
            let rows = n.div_ceil(cols);
            let (cell_w, cell_h) = ((sw * 0.86 / cols as f64).min(34.0 * u), 15.0 * u);
            let grid_h = rows as f64 * cell_h;
            let mut y = cy - (head_h + grid_h) / 2.0;
            if let Some((hs, hl)) = &head {
                let (a, dy) = appear(c.t, 0.0, 1.5 * u);
                lines(cv, &ctx.body, *hs, hl, cx, y + dy, 0.5, muted, a * fade);
                y += head_h;
            }
            for (i, id) in ids.iter().enumerate() {
                let (r, col) = (i / cols, i % cols);
                let in_row = if r == rows - 1 { n - r * cols } else { cols };
                let x0 = cx - in_row as f64 * cell_w / 2.0;
                let (ccx, ccy) = (x0 + (col as f64 + 0.5) * cell_w, y + (r as f64 + 0.5) * cell_h);
                let (a, dy) = appear(c.t, 0.15 + 0.07 * i as f64, 1.2 * u);
                if let Some(img) = ctx.asset_image(id, cell_w * 0.68, cell_h * 0.56, mono) {
                    blit(
                        cv,
                        &img,
                        ccx - img.width() as f64 / 2.0,
                        ccy - img.height() as f64 / 2.0 + dy,
                        a * fade * if mono { 0.85 } else { 1.0 },
                        0.0,
                    );
                }
            }
        }
        "cta_link" => {
            let url = s(p, "url").unwrap_or("");
            let with_qr = p.get("qr").and_then(|v| v.as_bool()).unwrap_or(true) && !url.is_empty();
            let qr = with_qr.then(|| qr_modules(url)).flatten();
            let vertical = h > w;
            let qr_side = if qr.is_some() { 26.0 * u } else { 0.0 };
            let text_w = if qr.is_some() && !vertical { sw * 0.86 - qr_side - 6.0 * u } else { sw * 0.86 };
            let (ts, tl) = fit(&ctx.title, c.text.unwrap_or(""), 8.0 * u, text_w, 3);
            let sub = c.subtitle.map(|t| fit(&ctx.body, t, 3.8 * u, text_w, 2));
            let (us, ul) = fit(&ctx.body_bold, url, 4.2 * u, text_w - 6.0 * u, 1);
            let pill_h = us * LINE + 2.6 * u;
            let text_h = tl.len() as f64 * ts * LINE
                + sub.as_ref().map(|(s, l)| l.len() as f64 * s * LINE + 1.5 * u).unwrap_or(0.0)
                + if url.is_empty() { 0.0 } else { pill_h + 4.0 * u };
            // the text block is as wide as its widest line, so the code sits next to it
            let text_w = tl
                .iter()
                .map(|l| text_width(&ctx.title, ts, l))
                .chain(sub.iter().flat_map(|(s, l)| l.iter().map(|x| text_width(&ctx.body, *s, x))))
                .chain(ul.iter().map(|l| text_width(&ctx.body_bold, us, l) + 6.0 * u))
                .fold(0.0, f64::max);
            let (tx, mut y, align, qx, qy);
            if vertical || qr.is_none() {
                let total = text_h + if qr.is_some() { qr_side + 5.0 * u } else { 0.0 };
                tx = cx;
                y = cy - total / 2.0;
                align = 0.5;
                qx = cx - qr_side / 2.0;
                qy = y + text_h + 5.0 * u;
            } else {
                let block = text_w + 6.0 * u + qr_side;
                tx = cx - block / 2.0;
                y = cy - text_h / 2.0;
                align = 0.0;
                qx = tx + text_w + 6.0 * u;
                qy = cy - qr_side / 2.0;
            }
            let (a, dy) = appear(c.t, 0.0, 2.0 * u);
            y += lines(cv, &ctx.title, ts, &tl, tx, y + dy, align, ink, a * fade);
            if let Some((ss, sl)) = &sub {
                let (a, dy) = appear(c.t, 0.15, 1.5 * u);
                y += lines(cv, &ctx.body, *ss, sl, tx, y + 1.5 * u + dy, align, muted, a * fade) + 1.5 * u;
            }
            if !url.is_empty() {
                let (a, dy) = appear(c.t, 0.3, 1.5 * u);
                let pw = text_width(&ctx.body_bold, us, &ul[0]) + 6.0 * u;
                let px0 = tx - pw * align;
                let py0 = y + 4.0 * u + dy;
                cv.shape((px0, py0, px0 + pw, py0 + pill_h), accent, a * fade, |px, py| {
                    aa(sd_rrect(px, py, px0, py0, pw, pill_h, pill_h / 2.0))
                });
                let on = if luma(accent) > 150.0 { [16, 18, 24] } else { [255, 255, 255] };
                lines(cv, &ctx.body_bold, us, &ul, px0 + 3.0 * u, py0 + 1.3 * u, 0.0, on, a * fade);
            }
            if let Some((n, mods)) = &qr {
                let (a, dy) = appear(c.t, 0.4, 1.5 * u);
                let (x0, y0) = (qx, qy + dy);
                cv.shape((x0, y0, x0 + qr_side, y0 + qr_side), [255, 255, 255], a * fade, |px, py| {
                    aa(sd_rrect(px, py, x0, y0, qr_side, qr_side, 1.6 * u))
                });
                // modules on whole pixels so scanners see clean edges
                let quiet = 2.2 * u;
                let m = ((qr_side - 2.0 * quiet) / *n as f64).floor().max(1.0);
                let off = (qr_side - m * *n as f64) / 2.0;
                let (ox, oy) = ((x0 + off).round(), (y0 + off).round());
                for (i, dark) in mods.iter().enumerate() {
                    if *dark {
                        let (mx, my) = (ox + (i % n) as f64 * m, oy + (i / n) as f64 * m);
                        cv.shape((mx, my, mx + m, my + m), [10, 12, 16], a * fade, |_, _| 1.0);
                    }
                }
            }
        }
        _ => {}
    }
}

fn luma(c: [u8; 3]) -> f64 {
    0.299 * c[0] as f64 + 0.587 * c[1] as f64 + 0.114 * c[2] as f64
}

fn seg(px: f64, py: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((px - a.0) * dx + (py - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    ((px - a.0 - t * dx).powi(2) + (py - a.1 - t * dy).powi(2)).sqrt()
}

/// `1234567.5` → `1,234,567.5`.
fn group(v: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, v.abs());
    let (int, frac) = s.split_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((&s, None));
    let mut out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    if v < 0.0 {
        out.insert(0, '-');
    }
    out
}

/// The link a QR code should carry: the URL as written, with `https://` when it has no scheme.
pub fn qr_target(url: &str) -> String {
    if url.contains("://") || url.starts_with("mailto:") {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

/// QR modules for a URL: (modules per side, dark flags row by row).
pub fn qr_modules(url: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::with_error_correction_level(qr_target(url).as_bytes(), qrcode::EcLevel::M).ok()?;
    let n = code.width();
    Some((n, code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_grouped() {
        assert_eq!(group(1234567.0, 0), "1,234,567");
        assert_eq!(group(999.0, 0), "999");
        assert_eq!(group(1234.56, 1), "1,234.6");
        assert_eq!(group(-1000.0, 0), "-1,000");
    }

    #[test]
    fn qr_code_carries_a_full_link() {
        assert_eq!(qr_target("example.com/demo"), "https://example.com/demo");
        assert_eq!(qr_target("http://example.com"), "http://example.com");
        let (n, m) = qr_modules("example.com/demo").unwrap();
        assert!(n >= 21 && m.len() == n * n);
        // finder pattern: the top-left 7×7 has a dark border
        assert!((0..7).all(|i| m[i] && m[i * n]));
    }

    #[test]
    fn wrap_breaks_on_words_and_newlines() {
        let db = cutline_text::FontDb::bundled().unwrap();
        let f = face(&db, "Inter", 400.0).unwrap();
        let l = wrap(&f, 40.0, "one two three four five six", 200.0);
        assert!(l.len() >= 3 && l.iter().all(|x| text_width(&f, 40.0, x) <= 200.0), "{l:?}");
        assert_eq!(wrap(&f, 40.0, "a\nb", 1000.0), vec!["a", "b"]);
        // a bold face of the variable font is wider than the regular one
        let b = face(&db, "Inter", 700.0).unwrap();
        assert!(text_width(&b, 40.0, "Heading") > text_width(&f, 40.0, "Heading") * 1.02);
    }
}
