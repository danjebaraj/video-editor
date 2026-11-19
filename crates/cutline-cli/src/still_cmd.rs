//! `cutline still`: one upright picture of any image cutline reads (HEIC/HEIF included) as a
//! JPEG or PNG, optionally scaled down. Video Studio uses it to make photos Claude can view.

use crate::out::*;
use anyhow::anyhow;
use serde_json::json;
use std::path::Path;

pub fn run(input: &Path, out: &Path, max_side: Option<u32>, quality: u8) -> CmdResult {
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let jpeg = match ext.as_str() {
        "jpg" | "jpeg" => true,
        "png" => false,
        _ => return Err(CliError::usage(anyhow!("--out must end in .jpg, .jpeg or .png"))),
    };
    if !(1..=100).contains(&quality) {
        return Err(CliError::usage(anyhow!("--quality must be 1 to 100")));
    }
    if max_side == Some(0) {
        return Err(CliError::usage(anyhow!("--max-side must be at least 1")));
    }
    if !input.is_file() {
        return Err(CliError::validation(anyhow!("E_IMAGE_DECODE: no such file: {}", input.display())));
    }
    let tc = cutline_deps::toolchain().tool_err()?;
    if let Some(d) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| CliError::internal(anyhow!("creating {}: {e}", d.display())))?;
    }
    // ffmpeg only decodes (a HEIF tile grid is composed in an internal filter graph, which
    // takes no `-vf`); the image crate scales and writes, so `--quality` means what it says.
    let tmp = out.with_extension(format!("{}.tmp.png", std::process::id()));
    let r = tc
        .ffmpeg_cmd()
        .args(cutline_media::still::decode_args(input, &tmp, !jpeg))
        .output()
        .map_err(|e| CliError::tool(anyhow!("running ffmpeg: {e}")))?;
    let fail = |reason: String| {
        let _ = std::fs::remove_file(&tmp);
        CliError::validation(anyhow!("E_IMAGE_DECODE: {reason}"))
    };
    if !r.status.success() || !tmp.is_file() {
        return Err(fail(cutline_media::still::ffmpeg_reason(&String::from_utf8_lossy(&r.stderr))));
    }
    let mut img = image::open(&tmp).map_err(|e| fail(format!("the decoded picture can't be read: {e}")))?;
    if let Some(m) = max_side.filter(|m| img.width().max(img.height()) > *m) {
        let (w, h) = (img.width() as f64, img.height() as f64);
        let k = m as f64 / w.max(h);
        let (nw, nh) = (((w * k).round() as u32).max(1), ((h * k).round() as u32).max(1));
        img = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
    }
    // decoded as RGBA for a PNG; the alpha channel stays only when something is transparent
    if !jpeg && (!img.color().has_alpha() || img.to_rgba8().pixels().all(|p| p[3] == 255)) {
        img = image::DynamicImage::ImageRgb8(img.to_rgb8());
    }
    let (width, height) = (img.width(), img.height());
    let written = if jpeg {
        std::fs::File::create(out).map_err(anyhow::Error::from).and_then(|f| {
            let mut w = std::io::BufWriter::new(f);
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, quality)
                .encode_image(&img.to_rgb8())
                .map_err(anyhow::Error::from)
        })
    } else {
        img.save_with_format(out, image::ImageFormat::Png).map_err(anyhow::Error::from)
    };
    let _ = std::fs::remove_file(&tmp);
    written.map_err(|e| CliError::internal(anyhow!("writing {}: {e:#}", out.display())))?;
    Ok(Output::ok(
        json!({"out": out, "width": width, "height": height}),
        format!("{} ({width}×{height})", out.display()),
    ))
}
