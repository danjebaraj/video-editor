//! Reading frames/audio back from media and measuring them.

use anyhow::{bail, Context, Result};
use cutline_deps::Toolchain;
use image::{GrayImage, RgbImage};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

/// Explicit YUV→RGB conversion (BT.709, limited range): ffmpeg versions disagree on whether
/// stream tags are honoured by default, which would make golden comparisons version-dependent.
pub const CONV: &str = "scale=in_color_matrix=bt709:in_range=limited,format=rgb24";

/// Video dimensions via ffprobe.
pub fn dims(tc: &Toolchain, p: &Path) -> Result<(u32, u32)> {
    let out = tc
        .ffprobe_cmd()
        .args(["-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0:s=x"])
        .arg(p)
        .output()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let (w, h) = s.trim().split_once('x').context("no video stream")?;
    Ok((w.parse()?, h.parse()?))
}

/// Decode selected frames (by output frame index) as RGB images. `indices` must be ascending.
pub fn read_frames(tc: &Toolchain, p: &Path, indices: &[u64]) -> Result<Vec<RgbImage>> {
    let (w, h) = dims(tc, p)?;
    if indices.is_empty() {
        return Ok(vec![]);
    }
    let sel = indices.iter().map(|i| format!("eq(n\\,{i})")).collect::<Vec<_>>().join("+");
    let mut child = Command::new(&tc.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(p)
        .args([
            "-vf",
            &format!("select='{sel}'"),
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut out = vec![];
    let mut stdout = child.stdout.take().unwrap();
    let fsz = (w * h * 3) as usize;
    loop {
        let mut buf = vec![0u8; fsz];
        match stdout.read_exact(&mut buf) {
            Ok(()) => out.push(RgbImage::from_raw(w, h, buf).unwrap()),
            Err(_) => break,
        }
    }
    let st = child.wait_with_output()?;
    if !st.status.success() {
        bail!("frame decode failed: {}", String::from_utf8_lossy(&st.stderr));
    }
    Ok(out)
}

/// Decode ALL frames, optionally cropped to (x, y, w, h) first to keep memory small.
pub fn read_all_frames(tc: &Toolchain, p: &Path, crop: Option<(u32, u32, u32, u32)>) -> Result<Vec<RgbImage>> {
    let (mut w, mut h) = dims(tc, p)?;
    let mut args: Vec<String> = vec!["-vf".into(), CONV.into()];
    if let Some((x, y, cw, ch)) = crop {
        let (x, y, cw, ch) = (x & !1, y & !1, (cw + 1) & !1, (ch + 1) & !1);
        args[1] = format!("crop={cw}:{ch}:{x}:{y},{CONV}");
        w = cw;
        h = ch;
    }
    let mut child = Command::new(&tc.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(p)
        .args(&args)
        .args(["-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().unwrap();
    let fsz = (w * h * 3) as usize;
    let mut out = vec![];
    loop {
        let mut buf = vec![0u8; fsz];
        if stdout.read_exact(&mut buf).is_err() {
            break;
        }
        out.push(RgbImage::from_raw(w, h, buf).unwrap());
    }
    child.wait()?;
    Ok(out)
}

/// Decode mono f32 audio samples at 48 kHz from `start` for `dur` seconds (all if dur None).
pub fn read_audio(tc: &Toolchain, p: &Path, start: f64, dur: Option<f64>, channels: u32) -> Result<Vec<f32>> {
    let mut c = Command::new(&tc.ffmpeg);
    c.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-ss", &format!("{start}"), "-i"]).arg(p);
    if let Some(d) = dur {
        c.args(["-t", &format!("{d}")]);
    }
    c.args(["-vn", "-ac", &channels.to_string(), "-ar", "48000", "-f", "f32le", "-"]);
    let out = c.output()?;
    if !out.status.success() {
        bail!("audio decode failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// Dominant frequency (Hz) of a mono 48 kHz buffer via FFT with a Hann window.
pub fn dominant_freq(samples: &[f32]) -> f64 {
    use rustfft::{num_complex::Complex, FftPlanner};
    let n = samples.len().next_power_of_two().max(1024);
    let mut buf: Vec<Complex<f32>> = (0..n)
        .map(|i| {
            let s = samples.get(i).copied().unwrap_or(0.0);
            let wv = if i < samples.len() {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / samples.len() as f32).cos()
            } else {
                0.0
            };
            Complex::new(s * wv, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let (mut best, mut bi) = (0f32, 0usize);
    for (i, c) in buf.iter().enumerate().take(n / 2).skip(1) {
        let m = c.norm();
        if m > best {
            best = m;
            bi = i;
        }
    }
    bi as f64 * 48000.0 / n as f64
}

/// RMS in dBFS.
pub fn rms_db(s: &[f32]) -> f64 {
    if s.is_empty() {
        return -120.0;
    }
    let e: f64 = s.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / s.len() as f64;
    10.0 * e.max(1e-12).log10()
}

fn to_gray(img: &RgbImage) -> GrayImage {
    image::imageops::grayscale(img)
}

/// Mean SSIM over 8×8 windows (stride 4) on luma. Images must have equal size.
pub fn ssim(a: &RgbImage, b: &RgbImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions());
    let (ga, gb) = (to_gray(a), to_gray(b));
    let (w, h) = ga.dimensions();
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let mut total = 0.0;
    let mut n = 0.0;
    let mut y = 0;
    while y + 8 <= h {
        let mut x = 0;
        while x + 8 <= w {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for yy in y..y + 8 {
                for xx in x..x + 8 {
                    let pa = ga.get_pixel(xx, yy)[0] as f64;
                    let pb = gb.get_pixel(xx, yy)[0] as f64;
                    sa += pa;
                    sb += pb;
                    saa += pa * pa;
                    sbb += pb * pb;
                    sab += pa * pb;
                }
            }
            let m = 64.0;
            let (ma, mb) = (sa / m, sb / m);
            let va = saa / m - ma * ma;
            let vb = sbb / m - mb * mb;
            let cov = sab / m - ma * mb;
            total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            n += 1.0;
            x += 4;
        }
        y += 4;
    }
    if n == 0.0 {
        1.0
    } else {
        total / n
    }
}

/// Compare `img` with a golden PNG at `golden`. With `CUTLINE_BLESS=1` (or when missing and
/// `CUTLINE_BLESS_MISSING=1`) the golden is (re)written. Returns the SSIM.
pub fn golden_compare(img: &RgbImage, golden: &Path, min_ssim: f64) -> Result<f64> {
    let bless = std::env::var("CUTLINE_BLESS").map(|v| v == "1").unwrap_or(false);
    let bless_missing = std::env::var("CUTLINE_BLESS_MISSING").map(|v| v == "1").unwrap_or(false);
    if bless || (!golden.exists() && bless_missing) {
        if let Some(p) = golden.parent() {
            std::fs::create_dir_all(p)?;
        }
        img.save(golden)?;
        return Ok(1.0);
    }
    if !golden.exists() {
        bail!("golden image missing: {} (run with CUTLINE_BLESS=1)", golden.display());
    }
    let g = image::open(golden)?.to_rgb8();
    if g.dimensions() != img.dimensions() {
        bail!("golden size {:?} != {:?}", g.dimensions(), img.dimensions());
    }
    // compare at half resolution (box filter): robust to encoder noise / pseudo-random test
    // patterns that differ between ffmpeg builds, still sensitive to layout and motion.
    let half = |i: &RgbImage| {
        image::imageops::resize(
            i,
            (i.width() / 2).max(8),
            (i.height() / 2).max(8),
            image::imageops::FilterType::Triangle,
        )
    };
    let s = ssim(&half(img), &half(&g));
    if s < min_ssim {
        let actual = golden.with_extension("actual.png");
        img.save(&actual)?;
        bail!("SSIM {s:.4} < {min_ssim} vs {} (actual saved to {})", golden.display(), actual.display());
    }
    Ok(s)
}

/// Mean RGB over a rectangle.
pub fn mean_rgb(img: &RgbImage, x: u32, y: u32, w: u32, h: u32) -> [f64; 3] {
    let mut s = [0f64; 3];
    let mut n = 0f64;
    for yy in y..(y + h).min(img.height()) {
        for xx in x..(x + w).min(img.width()) {
            let p = img.get_pixel(xx, yy);
            for c in 0..3 {
                s[c] += p[c] as f64;
            }
            n += 1.0;
        }
    }
    s.map(|v| v / n.max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fft_finds_tone() {
        let s: Vec<f32> = (0..9600).map(|i| (2.0 * std::f32::consts::PI * 660.0 * i as f32 / 48000.0).sin()).collect();
        let f = dominant_freq(&s);
        assert!((f - 660.0).abs() < 6.0, "{f}");
    }
    #[test]
    fn ssim_identity() {
        let a = RgbImage::from_fn(64, 64, |x, y| image::Rgb([(x * 4) as u8, (y * 4) as u8, 0]));
        assert!((ssim(&a, &a) - 1.0).abs() < 1e-9);
        let b = RgbImage::from_pixel(64, 64, image::Rgb([0, 0, 0]));
        assert!(ssim(&a, &b) < 0.5);
    }
}
