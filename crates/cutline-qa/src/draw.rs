//! Tiny raster helpers for review images: text labels (bundled Inter), rectangles, resizing.

use ab_glyph::{Font, PxScale, ScaleFont};
use image::{Rgb, RgbImage};

pub struct Painter {
    font: Option<ab_glyph::FontArc>,
}

impl Painter {
    pub fn new() -> Self {
        let font = cutline_text::FontDb::bundled().ok().and_then(|db| db.get("Inter", false).map(|f| f.font.clone()));
        Self { font }
    }

    /// Draw `text` with a dark backing box at (x, y) (top-left), `size` px.
    pub fn label(&self, img: &mut RgbImage, x: i32, y: i32, size: f32, text: &str, color: [u8; 3]) {
        let Some(font) = &self.font else { return };
        let sf = font.as_scaled(PxScale::from(size));
        let width: f32 = text.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum();
        fill_rect(img, x - 4, y - 2, width as i32 + 8, (sf.ascent() - sf.descent()) as i32 + 4, [0, 0, 0], 0.6);
        let mut pen = x as f32;
        let base = y as f32 + sf.ascent();
        for ch in text.chars() {
            let gid = sf.glyph_id(ch);
            let g = gid.with_scale_and_position(PxScale::from(size), ab_glyph::point(pen, base));
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| {
                    let px = b.min.x as i32 + gx as i32;
                    let py = b.min.y as i32 + gy as i32;
                    if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                        let p = img.get_pixel_mut(px as u32, py as u32);
                        for c in 0..3 {
                            p[c] = (p[c] as f32 * (1.0 - cov) + color[c] as f32 * cov) as u8;
                        }
                    }
                });
            }
            pen += sf.h_advance(gid);
        }
    }
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

pub fn fill_rect(img: &mut RgbImage, x: i32, y: i32, w: i32, h: i32, c: [u8; 3], alpha: f32) {
    for yy in y.max(0)..(y + h).min(img.height() as i32) {
        for xx in x.max(0)..(x + w).min(img.width() as i32) {
            let p = img.get_pixel_mut(xx as u32, yy as u32);
            for k in 0..3 {
                p[k] = (p[k] as f32 * (1.0 - alpha) + c[k] as f32 * alpha) as u8;
            }
        }
    }
}

pub fn stroke_rect(img: &mut RgbImage, x: i32, y: i32, w: i32, h: i32, t: i32, c: [u8; 3]) {
    fill_rect(img, x, y, w, t, c, 1.0);
    fill_rect(img, x, y + h - t, w, t, c, 1.0);
    fill_rect(img, x, y, t, h, c, 1.0);
    fill_rect(img, x + w - t, y, t, h, c, 1.0);
}

/// Shade everything outside the rect.
pub fn shade_outside(img: &mut RgbImage, x: i32, y: i32, w: i32, h: i32) {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    fill_rect(img, 0, 0, iw, y, [200, 0, 0], 0.3);
    fill_rect(img, 0, y + h, iw, ih - y - h, [200, 0, 0], 0.3);
    fill_rect(img, 0, y, x, h, [200, 0, 0], 0.3);
    fill_rect(img, x + w, y, iw - x - w, h, [200, 0, 0], 0.3);
}

/// Fit within `max` px on the longest side.
pub fn fit(img: &RgbImage, max: u32) -> RgbImage {
    let (w, h) = img.dimensions();
    let s = (max as f64 / w.max(h) as f64).min(1.0);
    if s >= 1.0 {
        return img.clone();
    }
    image::imageops::resize(
        img,
        ((w as f64 * s) as u32).max(1),
        ((h as f64 * s) as u32).max(1),
        image::imageops::FilterType::Triangle,
    )
}

pub fn save_jpg(img: &RgbImage, path: &std::path::Path) -> anyhow::Result<()> {
    let f = std::fs::File::create(path)?;
    let mut w = std::io::BufWriter::new(f);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 85).encode_image(img)?;
    Ok(())
}

pub fn hstack(a: &RgbImage, b: &RgbImage, gap: u32) -> RgbImage {
    let h = a.height().max(b.height());
    let mut out = RgbImage::from_pixel(a.width() + b.width() + gap, h, Rgb([20, 20, 24]));
    image::imageops::overlay(&mut out, a, 0, 0);
    image::imageops::overlay(&mut out, b, (a.width() + gap) as i64, 0);
    out
}
