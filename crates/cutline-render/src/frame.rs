//! Screen frames: draws the background, shadow and window chrome of a framed clip as one PNG
//! with a transparent hole where the recording shows. The segment renderer pads the recording
//! to the output size and lays this picture over it.

use crate::annot::{sd_rrect, text_width, Canvas};
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use anyhow::{Context, Result};
use cutline_core::frame::FrameLayout;
use cutline_core::model::{FrameBackground, FrameStyle, FrameTheme, ScreenFrame};
use std::path::{Path, PathBuf};

/// A frame's background, resolved against the brand and the project's assets.
#[derive(Debug, Clone)]
pub enum Background {
    Gradient([u8; 3], [u8; 3]),
    /// Image file and its fingerprint.
    Image(PathBuf, String),
}

/// Everything the segment renderer needs to draw a clip inside its frame.
#[derive(Debug, Clone)]
pub struct FramePlan {
    /// The chrome picture, sized for the geometry the segment works in.
    pub png: PathBuf,
    pub layout: FrameLayout,
    /// Size the picture was drawn for.
    pub size: (u32, u32),
}

fn lighten(c: [u8; 3], amt: f64) -> [u8; 3] {
    c.map(|v| (v as f64 + (255.0 - v as f64) * amt).round() as u8)
}

/// Resolve `background` (default `brand`: a soft gradient from the brand background).
pub fn background(
    bg: Option<&FrameBackground>,
    brand_bg: &str,
    asset: impl Fn(&str) -> Option<(PathBuf, String)>,
) -> Background {
    let brand = || {
        let c = crate::overlay::rgb_of(brand_bg);
        Background::Gradient(lighten(c, 0.16), c)
    };
    match bg {
        None => brand(),
        Some(FrameBackground::Gradient { gradient }) if gradient.len() == 2 => {
            Background::Gradient(crate::overlay::rgb_of(&gradient[0]), crate::overlay::rgb_of(&gradient[1]))
        }
        Some(FrameBackground::Gradient { .. }) => brand(),
        Some(FrameBackground::Name(n)) if n == "brand" => brand(),
        Some(FrameBackground::Name(n)) if n.starts_with('#') => {
            let c = crate::overlay::rgb_of(n);
            Background::Gradient(c, c)
        }
        Some(FrameBackground::Name(n)) => match asset(n) {
            Some((p, fp)) => Background::Image(p, fp),
            None => brand(),
        },
    }
}

#[inline]
fn aa(d: f64) -> f64 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// Draw the chrome picture for `layout` in a `w×h` frame.
pub fn draw(
    fr: &ScreenFrame,
    l: &FrameLayout,
    (w, h): (u32, u32),
    bg: &Background,
    font: Option<&FontArc>,
) -> Result<Canvas> {
    let mut cv = Canvas::new(w, h);
    let u = w.min(h) as f64 / 1080.0;
    // ---- background (opaque)
    match bg {
        Background::Gradient(a, b) => {
            let den = (w + h).max(1) as f64;
            for y in 0..h {
                for x in 0..w {
                    let t = (x + y) as f64 / den;
                    let i = ((y * w + x) * 4) as usize;
                    for c in 0..3 {
                        cv.px[i + c] = (a[c] as f64 + (b[c] as f64 - a[c] as f64) * t).round() as u8;
                    }
                    cv.px[i + 3] = 255;
                }
            }
        }
        Background::Image(p, _) => {
            let img = image::open(p).with_context(|| format!("frame background {}", p.display()))?.to_rgb8();
            let s = (w as f64 / img.width() as f64).max(h as f64 / img.height() as f64);
            let (rw, rh) =
                (((img.width() as f64 * s).ceil() as u32).max(w), ((img.height() as f64 * s).ceil() as u32).max(h));
            let r = image::imageops::resize(&img, rw, rh, image::imageops::FilterType::Triangle);
            let (ox, oy) = ((rw - w) / 2, (rh - h) / 2);
            for y in 0..h {
                for x in 0..w {
                    let p = r.get_pixel(x + ox, y + oy);
                    let i = ((y * w + x) * 4) as usize;
                    cv.px[i..i + 3].copy_from_slice(&p.0);
                    cv.px[i + 3] = 255;
                }
            }
        }
    }
    let (wx, wy, ww, wh) = l.window;
    let rad = l.radius;
    // ---- shadow
    let strength = fr.shadow.unwrap_or(0.5);
    if strength > 0.0 {
        let (blur, dy) = (46.0 * u, 16.0 * u);
        let bb = (wx - blur, wy - blur + dy, wx + ww + blur, wy + wh + blur + dy);
        cv.shape(bb, [0, 0, 0], 0.62 * strength, |x, y| {
            let d = sd_rrect(x, y - dy, wx, wy, ww, wh, rad);
            let t = (1.0 - d / blur).clamp(0.0, 1.0);
            t * t
        });
    }
    // ---- window body
    let light = fr.theme == Some(FrameTheme::Light);
    let (bar_c, field_c, text_c, edge_c): ([u8; 3], [u8; 3], [u8; 3], [u8; 3]) = if light {
        ([236, 237, 240], [255, 255, 255], [74, 77, 85], [205, 207, 212])
    } else {
        ([43, 45, 51], [28, 30, 35], [201, 204, 211], [18, 19, 22])
    };
    let wb = (wx - 1.0, wy - 1.0, wx + ww + 1.0, wy + wh + 1.0);
    match fr.style {
        FrameStyle::Browser | FrameStyle::Window => {
            let bar_bottom = wy + l.bar;
            cv.shape((wx - 1.0, wy - 1.0, wx + ww + 1.0, bar_bottom), bar_c, 1.0, |x, y| {
                aa(sd_rrect(x, y, wx, wy, ww, wh, rad).max(y - bar_bottom))
            });
            // hairline under the bar
            let hl = (1.0 * u).max(1.0);
            cv.shape((wx, bar_bottom - hl, wx + ww, bar_bottom), edge_c, if light { 1.0 } else { 0.6 }, |_, _| 1.0);
            // window controls
            let cy = wy + l.bar / 2.0;
            let r = 0.125 * l.bar;
            for (i, col) in [[255u8, 95, 87], [254, 188, 46], [40, 200, 64]].iter().enumerate() {
                let cx = wx + 0.42 * l.bar + i as f64 * 3.1 * r;
                cv.shape((cx - r - 1.0, cy - r - 1.0, cx + r + 1.0, cy + r + 1.0), *col, 1.0, |x, y| {
                    aa(((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r)
                });
            }
            let fs = 0.34 * l.bar;
            let label = if fr.style == FrameStyle::Browser { fr.url.as_deref() } else { fr.title.as_deref() };
            if fr.style == FrameStyle::Browser {
                let (fw, fh) = ((0.5 * ww).min(ww - 9.0 * l.bar).max(0.0), 0.6 * l.bar);
                let (fx, fy) = (wx + (ww - fw) / 2.0, cy - fh / 2.0);
                if fw > 4.0 {
                    cv.shape((fx, fy, fx + fw, fy + fh), field_c, 1.0, |x, y| {
                        aa(sd_rrect(x, y, fx, fy, fw, fh, fh * 0.3))
                    });
                }
            }
            if let (Some(text), Some(font)) = (label.filter(|s| !s.is_empty()), font) {
                let tw = text_width(font, fs, text);
                let sf = font.as_scaled(PxScale::from(fs as f32));
                let baseline = cy + (sf.ascent() as f64 + sf.descent() as f64) / 2.0;
                if tw < ww - 8.0 * l.bar {
                    cv.text(font, fs, wx + (ww - tw) / 2.0, baseline, text, text_c, 1.0);
                }
            }
        }
        FrameStyle::Phone | FrameStyle::Tablet => {
            cv.shape(wb, [14, 15, 18], 1.0, |x, y| aa(sd_rrect(x, y, wx, wy, ww, wh, rad)));
            // a faint rim so the bezel reads against dark backgrounds
            let rim = (1.5 * u).max(1.0);
            cv.shape(wb, [120, 124, 134], 0.55, |x, y| aa(sd_rrect(x, y, wx, wy, ww, wh, rad).abs() - rim / 2.0));
        }
        FrameStyle::None | FrameStyle::Off => {}
    }
    // ---- the hole the recording shows through
    let (cx, cy, cw, ch) = (l.content.0 as f64, l.content.1 as f64, l.content.2 as f64, l.content.3 as f64);
    let inner = (rad - l.bezel).max(0.0);
    let square_top = l.bar > 0.0;
    for y in l.content.1..(l.content.1 + l.content.3).min(h) {
        for x in l.content.0..(l.content.0 + l.content.2).min(w) {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let d = if square_top {
                // rounded at the bottom only: the bar covers the top corners
                sd_rrect(fx, fy, cx, cy - inner, cw, ch + inner, inner).max(cy - fy)
            } else {
                sd_rrect(fx, fy, cx, cy, cw, ch, inner)
            };
            let open = aa(d);
            if open > 0.0 {
                let i = ((y * w + x) * 4) as usize;
                cv.px[i + 3] = ((cv.px[i + 3] as f64) * (1.0 - open)).round() as u8;
            }
        }
    }
    Ok(cv)
}

/// The chrome picture for a clip, drawn once and cached by everything that shapes it.
pub fn ensure(
    fr: &ScreenFrame,
    src: (u32, u32),
    size: (u32, u32),
    bg: &Background,
    cache_dir: &Path,
) -> Result<FramePlan> {
    let layout = cutline_core::frame::layout(fr, src, size);
    let bg_key = match bg {
        Background::Gradient(a, b) => format!("{a:?}{b:?}"),
        Background::Image(_, fp) => fp.clone(),
    };
    let key = cutline_media::hash_key(&[
        serde_json::to_string(fr).unwrap_or_default().as_bytes(),
        format!("{src:?} {size:?} {bg_key}").as_bytes(),
        cutline_core::VERSION.as_bytes(),
        b"frame-v1",
    ]);
    let png = cache_dir.join(format!("{key}.png"));
    if !png.exists() {
        std::fs::create_dir_all(cache_dir)?;
        let font = cutline_text::FontDb::bundled().ok().and_then(|db| db.get("Inter", false).map(|f| f.font.clone()));
        let cv = draw(fr, &layout, size, bg, font.as_ref())?;
        let tmp = cache_dir.join(format!("{key}.tmp.png"));
        {
            use image::ImageEncoder;
            let file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            image::codecs::png::PngEncoder::new_with_quality(
                file,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Adaptive,
            )
            .write_image(&cv.px, size.0, size.1, image::ExtendedColorType::Rgba8)?;
        }
        std::fs::rename(&tmp, &png)?;
    }
    Ok(FramePlan { png, layout, size })
}
