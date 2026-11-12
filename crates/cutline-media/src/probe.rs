//! ffprobe wrapper producing normalized media info.

use anyhow::{bail, Context, Result};
use cutline_deps::Toolchain;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoInfo {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// Size after applying rotation metadata (what the viewer sees).
    pub display_width: u32,
    pub display_height: u32,
    pub sar: String,
    /// Frame rate as `num/den` (avg_frame_rate, falling back to r_frame_rate).
    pub fps: String,
    pub fps_f64: f64,
    pub vfr: bool,
    pub rotation: i32,
    pub pix_fmt: String,
    pub color_primaries: Option<String>,
    pub color_transfer: Option<String>,
    pub color_space: Option<String>,
    pub color_range: Option<String>,
    pub alpha: bool,
    /// When alpha must be decoded with libvpx-vp9 (WebM VP9 alpha).
    pub needs_libvpx_decoder: bool,
    pub frames: Option<u64>,
    pub bit_depth: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub channel_layout: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaInfo {
    pub path: String,
    pub format: String,
    pub duration: f64,
    pub size: u64,
    pub bit_rate: Option<u64>,
    /// video | image | audio | image_sequence
    pub kind: String,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loudness: Option<Loudness>,
    pub faststart: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Loudness {
    pub integrated_lufs: f64,
    pub lra: f64,
    pub true_peak_dbtp: f64,
}

pub fn parse_rational(s: &str) -> Option<f64> {
    let (a, b) = s.split_once('/').unwrap_or((s, "1"));
    let a: f64 = a.trim().parse().ok()?;
    let b: f64 = b.trim().parse().ok()?;
    (b != 0.0).then(|| a / b)
}

fn s(v: &serde_json::Value) -> Option<String> {
    v.as_str()
        .map(str::to_string)
        .filter(|x| !x.is_empty() && x != "unknown")
}

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"];

fn pix_has_alpha(p: &str) -> bool {
    p.starts_with("yuva")
        || p.starts_with("rgba")
        || p.starts_with("bgra")
        || p.starts_with("argb")
        || p.starts_with("abgr")
        || p.starts_with("gbrap")
        || p.starts_with("ya")
        || p == "pal8"
}

fn bit_depth(p: &str) -> u32 {
    for d in [16, 14, 12, 10, 9] {
        if p.contains(&format!("p{d}"))
            || p.ends_with(&format!("{d}le"))
            || p.ends_with(&format!("{d}be"))
        {
            return d;
        }
    }
    8
}

/// Probe a media file. `path` may be a printf image-sequence pattern (`frames/%05d.png`).
pub fn probe(tc: &Toolchain, path: &Path, fps_hint: Option<f64>) -> Result<MediaInfo> {
    let ps = path.to_string_lossy().to_string();
    let is_seq = ps.contains('%');
    let mut cmd = tc.ffprobe_cmd();
    cmd.args(["-print_format", "json", "-show_format", "-show_streams"]);
    if is_seq {
        cmd.args([
            "-f",
            "image2",
            "-framerate",
            &format!("{}", fps_hint.unwrap_or(30.0)),
        ]);
    } else if !path.exists() {
        bail!("file not found: {}", path.display());
    }
    cmd.arg(path);
    let out = cmd.output().context("running ffprobe")?;
    if !out.status.success() {
        bail!(
            "ffprobe failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let j: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let fmt = &j["format"];
    let streams = j["streams"].as_array().cloned().unwrap_or_default();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let vs = streams.iter().find(|st| {
        st["codec_type"] == "video" && st["disposition"]["attached_pic"].as_i64() != Some(1)
    });
    let a = streams.iter().find(|st| st["codec_type"] == "audio");

    let video = vs.map(|v| {
        let codec = s(&v["codec_name"]).unwrap_or_default();
        let width = v["width"].as_u64().unwrap_or(0) as u32;
        let height = v["height"].as_u64().unwrap_or(0) as u32;
        let mut rotation = 0i32;
        if let Some(sd) = v["side_data_list"].as_array() {
            for d in sd {
                if let Some(r) = d["rotation"].as_i64() {
                    rotation = r as i32;
                }
            }
        }
        if rotation == 0 {
            if let Some(r) = v["tags"]["rotate"]
                .as_str()
                .and_then(|x| x.parse::<i32>().ok())
            {
                rotation = -r; // tag is clockwise, displaymatrix counter-clockwise
            }
        }
        let rot_norm = rotation.rem_euclid(360);
        let (dw, dh) = if rot_norm == 90 || rot_norm == 270 {
            (height, width)
        } else {
            (width, height)
        };
        let r = s(&v["r_frame_rate"]).unwrap_or_else(|| "0/1".into());
        let avg = s(&v["avg_frame_rate"])
            .filter(|x| x != "0/0")
            .unwrap_or_else(|| r.clone());
        let rf = parse_rational(&r).unwrap_or(0.0);
        let af = parse_rational(&avg).unwrap_or(rf);
        let vfr = rf > 0.0 && af > 0.0 && ((rf - af).abs() / rf) > 0.01;
        let pix = s(&v["pix_fmt"]).unwrap_or_default();
        let alpha_mode = v["tags"]["alpha_mode"].as_str() == Some("1")
            || v["tags"]["ALPHA_MODE"].as_str() == Some("1");
        let vp9_alpha = codec == "vp9" && alpha_mode;
        let vp8_alpha = codec == "vp8" && alpha_mode;
        let fps_str = if is_seq {
            format!("{}", fps_hint.unwrap_or(30.0))
        } else {
            avg.clone()
        };
        VideoInfo {
            alpha: pix_has_alpha(&pix) || vp9_alpha || vp8_alpha,
            needs_libvpx_decoder: vp9_alpha || vp8_alpha,
            bit_depth: bit_depth(&pix),
            codec,
            width,
            height,
            display_width: dw,
            display_height: dh,
            sar: s(&v["sample_aspect_ratio"]).unwrap_or_else(|| "1:1".into()),
            fps_f64: parse_rational(&fps_str).unwrap_or(af),
            fps: fps_str,
            vfr,
            rotation,
            pix_fmt: pix,
            color_primaries: s(&v["color_primaries"]),
            color_transfer: s(&v["color_transfer"]),
            color_space: s(&v["color_space"]),
            color_range: s(&v["color_range"]),
            frames: v["nb_frames"].as_str().and_then(|x| x.parse().ok()),
        }
    });
    let audio = a.map(|a| AudioInfo {
        codec: s(&a["codec_name"]).unwrap_or_default(),
        sample_rate: a["sample_rate"]
            .as_str()
            .and_then(|x| x.parse().ok())
            .unwrap_or(0),
        channels: a["channels"].as_u64().unwrap_or(0) as u32,
        channel_layout: s(&a["channel_layout"]),
    });
    let format = s(&fmt["format_name"]).unwrap_or_default();
    let mut duration = fmt["duration"]
        .as_str()
        .and_then(|x| x.parse().ok())
        .unwrap_or(0.0);
    if duration == 0.0 {
        if let Some(v) = vs {
            duration = v["duration"]
                .as_str()
                .and_then(|x| x.parse().ok())
                .unwrap_or(0.0);
        }
    }
    let kind = if is_seq {
        "image_sequence"
    } else if video.is_none() {
        "audio"
    } else if IMAGE_EXT.contains(&ext.as_str()) || format.contains("_pipe") || format == "image2" {
        "image"
    } else {
        "video"
    }
    .to_string();
    let faststart = if format.contains("mp4") || format.contains("mov") {
        moov_before_mdat(path).ok()
    } else {
        None
    };
    Ok(MediaInfo {
        path: ps,
        size: fmt["size"]
            .as_str()
            .and_then(|x| x.parse().ok())
            .unwrap_or(0),
        bit_rate: fmt["bit_rate"].as_str().and_then(|x| x.parse().ok()),
        format,
        duration,
        kind,
        video,
        audio,
        loudness: None,
        faststart,
    })
}

/// True if the `moov` atom precedes `mdat` (i.e. `+faststart`).
pub fn moov_before_mdat(path: &Path) -> Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut pos = 0u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8])?;
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let typ: [u8; 4] = [h[4], h[5], h[6], h[7]];
        if size == 1 {
            f.read_exact(&mut h[8..16])?;
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
        } else if size == 0 {
            size = len - pos;
        }
        if &typ == b"moov" {
            return Ok(true);
        }
        if &typ == b"mdat" {
            return Ok(false);
        }
        if size < 8 {
            break;
        }
        pos += size;
    }
    bail!("no moov/mdat atoms found")
}

/// Fast integrated loudness / LRA / true peak via `ebur128`.
pub fn measure_loudness(tc: &Toolchain, path: &Path) -> Result<Loudness> {
    let out = std::process::Command::new(&tc.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-nostats", "-i"])
        .arg(path)
        .args([
            "-vn",
            "-af",
            "ebur128=peak=true:framelog=quiet",
            "-f",
            "null",
            "-",
        ])
        .output()?;
    let err = String::from_utf8_lossy(&out.stderr);
    parse_ebur128_summary(&err).context("could not parse ebur128 summary")
}

pub fn parse_ebur128_summary(s: &str) -> Option<Loudness> {
    let idx = s.rfind("Summary:")?;
    let tail = &s[idx..];
    let grab = |key: &str| -> Option<f64> {
        let p = tail.find(key)?;
        tail[p + key.len()..]
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    };
    Some(Loudness {
        integrated_lufs: grab("I:")?,
        lra: grab("LRA:")?,
        true_peak_dbtp: grab("Peak:").unwrap_or(f64::NAN),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ebur_parse() {
        let s = "[Parsed_ebur128_0 @ 0x] Summary:\n\n  Integrated loudness:\n    I:         -14.2 LUFS\n    Threshold: -24.5 LUFS\n\n  Loudness range:\n    LRA:         3.1 LU\n    Threshold: -34.6 LUFS\n    LRA low:   -15.8 LUFS\n    LRA high:  -12.7 LUFS\n\n  True peak:\n    Peak:       -1.2 dBFS";
        let l = parse_ebur128_summary(s).unwrap();
        assert_eq!(l.integrated_lufs, -14.2);
        assert_eq!(l.lra, 3.1);
        assert_eq!(l.true_peak_dbtp, -1.2);
    }
    #[test]
    fn rationals() {
        assert_eq!(parse_rational("30000/1001").unwrap(), 30000.0 / 1001.0);
        assert_eq!(parse_rational("25").unwrap(), 25.0);
        assert!(parse_rational("1/0").is_none());
    }
}
