//! `audio.png`: waveform per role, final mix, duck envelope and momentary loudness.

use image::{Rgb, RgbImage};

pub struct Track<'a> {
    pub name: &'a str,
    pub samples: &'a [f32],
    pub rate: u32,
    pub color: [u8; 3],
}

/// Render the audio overview. `envelope` is a linear gain at `env_rate`; `momentary` is
/// (t, LUFS) points; `target` is the integrated target line.
pub fn render(
    tracks: &[Track],
    envelope: Option<(&[f32], u32)>,
    momentary: &[(f64, f64)],
    target: f64,
    duration: f64,
    width: u32,
) -> RgbImage {
    let lane_h = 110u32;
    let lanes = tracks.len() as u32 + u32::from(envelope.is_some()) + 1;
    let h = lanes * lane_h + 10;
    let mut img = RgbImage::from_pixel(width, h, Rgb([18, 18, 22]));
    let x_of = |t: f64| ((t / duration.max(1e-6)) * (width - 1) as f64).round().clamp(0.0, (width - 1) as f64) as u32;
    // second gridlines
    let step = if duration > 120.0 {
        10.0
    } else if duration > 30.0 {
        5.0
    } else {
        1.0
    };
    let mut t = 0.0;
    while t <= duration {
        let x = x_of(t);
        for y in 0..h {
            img.put_pixel(x, y, Rgb([40, 40, 48]));
        }
        t += step;
    }
    let mut lane = 0u32;
    for tr in tracks {
        let y0 = lane * lane_h + 5;
        let mid = y0 + lane_h / 2;
        label(&mut img, 4, y0 + 2, tr.name, tr.color);
        let per_px = (tr.samples.len() as f64 / width as f64).max(1.0);
        for x in 0..width {
            let a = (x as f64 * per_px) as usize;
            let b = (((x + 1) as f64 * per_px) as usize).min(tr.samples.len());
            if a >= b {
                continue;
            }
            let peak = tr.samples[a..b].iter().fold(0f32, |m, s| m.max(s.abs())).min(1.0);
            let hh = (peak * (lane_h as f32 / 2.0 - 8.0)) as u32;
            for y in mid.saturating_sub(hh)..=(mid + hh).min(h - 1) {
                img.put_pixel(x, y, Rgb(tr.color));
            }
        }
        lane += 1;
    }
    if let Some((env, rate)) = envelope {
        let y0 = lane * lane_h + 5;
        label(&mut img, 4, y0 + 2, "duck envelope", [255, 180, 60]);
        for x in 0..width {
            let t = x as f64 / width as f64 * duration;
            let i = ((t * rate as f64) as usize).min(env.len().saturating_sub(1));
            let g = env.get(i).copied().unwrap_or(1.0).clamp(0.0, 1.0);
            let y = y0 + 10 + ((1.0 - g) * (lane_h as f32 - 20.0)) as u32;
            img.put_pixel(x, y.min(h - 1), Rgb([255, 180, 60]));
            img.put_pixel(x, (y + 1).min(h - 1), Rgb([255, 180, 60]));
        }
        lane += 1;
    }
    // momentary loudness lane: -40..0 LUFS
    let y0 = lane * lane_h + 5;
    label(&mut img, 4, y0 + 2, "momentary LUFS", [120, 220, 255]);
    let y_of = |l: f64| y0 + 10 + (((-l).clamp(0.0, 40.0) / 40.0) * (lane_h as f64 - 20.0)) as u32;
    let ty = y_of(target);
    for x in (0..width).step_by(4) {
        img.put_pixel(x, ty, Rgb([255, 80, 80]));
    }
    for w in momentary.windows(2) {
        let (x1, x2) = (x_of(w[0].0), x_of(w[1].0));
        let y = y_of(w[1].1);
        for x in x1..=x2.max(x1) {
            img.put_pixel(x.min(width - 1), y.min(h - 1), Rgb([120, 220, 255]));
        }
    }
    img
}

/// Tiny built-in 3×5 bitmap font so the PNG needs no font files.
fn label(img: &mut RgbImage, x: u32, y: u32, text: &str, c: [u8; 3]) {
    let mut cx = x;
    for ch in text.to_ascii_uppercase().chars() {
        let g = glyph(ch);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (1 << (2 - col)) != 0 {
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let px = cx + col * 2 + dx;
                            let py = y + row as u32 * 2 + dy;
                            if px < img.width() && py < img.height() {
                                img.put_pixel(px, py, Rgb(c));
                            }
                        }
                    }
                }
            }
        }
        cx += 8;
    }
}

fn glyph(c: char) -> [u8; 5] {
    match c {
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [3, 4, 4, 4, 3],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [3, 4, 5, 5, 3],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 2],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'O' => [2, 5, 5, 5, 2],
        'P' => [6, 5, 6, 4, 4],
        'Q' => [2, 5, 5, 6, 3],
        'R' => [6, 5, 6, 5, 5],
        'S' => [3, 4, 2, 1, 6],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [6, 1, 2, 4, 7],
        '3' => [6, 1, 2, 1, 6],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 6, 1, 6],
        '6' => [3, 4, 7, 5, 7],
        '7' => [7, 1, 2, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 6],
        '-' => [0, 0, 7, 0, 0],
        '.' => [0, 0, 0, 0, 2],
        ':' => [0, 2, 0, 2, 0],
        _ => [0, 0, 0, 0, 0],
    }
}
