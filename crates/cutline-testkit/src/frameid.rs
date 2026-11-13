//! Frame-ID pattern: each frame encodes its index as 16 binary blocks (pure black/white).
//!
//! The default layout follows the spec (16 blocks of 64×64 in the top row). A layout can be
//! placed anywhere (e.g. a centered 8×2 grid that survives a 16:9→9:16 cover crop) and can be
//! mapped through a geometry transform so the decoder knows where the blocks land in an output.

use image::RgbImage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrameIdLayout {
    /// Top-left of the block grid, in pixels.
    pub x0: f64,
    pub y0: f64,
    /// Block size in pixels.
    pub bw: f64,
    pub bh: f64,
    /// Grid shape; cols * rows must be 16. Bits are MSB first, row-major.
    pub cols: u32,
    pub rows: u32,
}

impl FrameIdLayout {
    /// Spec default: top-left row of 16 64×64 blocks.
    pub fn top_row() -> Self {
        Self { x0: 0.0, y0: 0.0, bw: 64.0, bh: 64.0, cols: 16, rows: 1 }
    }

    /// 8×2 grid of `b`-px blocks centered horizontally at vertical offset `y0` within width `w`.
    pub fn centered(w: u32, y0: f64, b: f64) -> Self {
        Self { x0: (w as f64 - 8.0 * b) / 2.0, y0, bw: b, bh: b, cols: 8, rows: 2 }
    }

    /// Apply `out = in * scale + offset` (uniform scale + translation, e.g. a cover crop).
    pub fn transform(&self, sx: f64, sy: f64, ox: f64, oy: f64) -> Self {
        Self { x0: self.x0 * sx + ox, y0: self.y0 * sy + oy, bw: self.bw * sx, bh: self.bh * sy, ..*self }
    }

    /// Layout after scaling a `sw×sh` source to cover `tw×th` (scale up, center crop).
    pub fn after_cover(&self, sw: u32, sh: u32, tw: u32, th: u32) -> Self {
        let s = (tw as f64 / sw as f64).max(th as f64 / sh as f64);
        let ox = (tw as f64 - sw as f64 * s) / 2.0;
        let oy = (th as f64 - sh as f64 * s) / 2.0;
        self.transform(s, s, ox, oy)
    }

    /// Layout after fitting a `sw×sh` source inside `tw×th` (scale down, center pad).
    pub fn after_fit(&self, sw: u32, sh: u32, tw: u32, th: u32) -> Self {
        let s = (tw as f64 / sw as f64).min(th as f64 / sh as f64);
        let ox = (tw as f64 - sw as f64 * s) / 2.0;
        let oy = (th as f64 - sh as f64 * s) / 2.0;
        self.transform(s, s, ox, oy)
    }

    /// Block rectangle (x, y, w, h) for bit index `i` (0 = MSB).
    pub fn block(&self, i: u32) -> (f64, f64, f64, f64) {
        let c = i % self.cols;
        let r = i / self.cols;
        (self.x0 + c as f64 * self.bw, self.y0 + r as f64 * self.bh, self.bw, self.bh)
    }

    /// Bounding box of the grid.
    pub fn bbox(&self) -> (f64, f64, f64, f64) {
        (self.x0, self.y0, self.bw * self.cols as f64, self.bh * self.rows as f64)
    }

    /// ffmpeg `geq` luma expression drawing the pattern over the input (`lum(X,Y)` elsewhere).
    /// Use on a `yuv444p` (or gray) input; chroma must be neutralized separately in the region.
    pub fn geq_luma_expr(&self) -> String {
        let (x0, y0, w, h) = self.bbox();
        let inside = format!("between(X,{x0},{x1}-1)*between(Y,{y0},{y1}-1)", x1 = x0 + w, y1 = y0 + h);
        let col = format!("floor((X-{x0})/{bw})", bw = self.bw);
        let row = format!("floor((Y-{y0})/{bh})", bh = self.bh);
        let bit = format!("(15-({row}*{cols}+{col}))", cols = self.cols);
        let val = format!("mod(floor(N/pow(2,{bit})),2)*255");
        format!("if({inside},{val},lum(X,Y))")
    }

    pub fn geq_chroma_expr(&self, plane: &str) -> String {
        let (x0, y0, w, h) = self.bbox();
        format!("if(between(X,{x0},{x1}-1)*between(Y,{y0},{y1}-1),128,{plane}(X,Y))", x1 = x0 + w, y1 = y0 + h)
    }
}

/// Decode the frame index from an RGB image. Samples the inner 50% of each block.
/// Returns `None` if any block is ambiguous (mean luma between 64 and 192).
pub fn decode_frame_id(img: &RgbImage, layout: &FrameIdLayout) -> Option<u16> {
    let mut v: u16 = 0;
    for i in 0..16 {
        let (x, y, w, h) = layout.block(i);
        let (ix0, iy0) = ((x + w * 0.25).round() as i64, (y + h * 0.25).round() as i64);
        let (ix1, iy1) = ((x + w * 0.75).round() as i64, (y + h * 0.75).round() as i64);
        let mut sum = 0f64;
        let mut n = 0f64;
        for yy in iy0.max(0)..iy1.min(img.height() as i64) {
            for xx in ix0.max(0)..ix1.min(img.width() as i64) {
                let p = img.get_pixel(xx as u32, yy as u32);
                sum += 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
                n += 1.0;
            }
        }
        if n == 0.0 {
            return None;
        }
        let m = sum / n;
        if (64.0..=192.0).contains(&m) {
            return None;
        }
        v = (v << 1) | u16::from(m > 128.0);
    }
    Some(v)
}

/// Draw the pattern into an image (used for unit tests of the decoder).
pub fn draw_frame_id(img: &mut RgbImage, layout: &FrameIdLayout, id: u16) {
    for i in 0..16 {
        let bit = (id >> (15 - i)) & 1;
        let c = if bit == 1 { 255 } else { 0 };
        let (x, y, w, h) = layout.block(i);
        for yy in y as u32..(y + h) as u32 {
            for xx in x as u32..(x + w) as u32 {
                if xx < img.width() && yy < img.height() {
                    img.put_pixel(xx, yy, image::Rgb([c, c, c]));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_in_memory() {
        let l = FrameIdLayout::top_row();
        for id in [0u16, 1, 2, 299, 0xABCD, 0xFFFF] {
            let mut img = RgbImage::from_pixel(1280, 720, image::Rgb([90, 120, 200]));
            draw_frame_id(&mut img, &l, id);
            assert_eq!(decode_frame_id(&img, &l), Some(id));
        }
    }
    #[test]
    fn cover_transform() {
        let l = FrameIdLayout::centered(1920, 64.0, 64.0);
        let t = l.after_cover(1920, 1080, 1080, 1920);
        let (x, _, w, _) = t.bbox();
        assert!(x >= 0.0 && x + w <= 1080.0, "{t:?}");
    }
}
