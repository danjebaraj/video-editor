//! Synthetic media generators built on ffmpeg `lavfi` sources.

use crate::frameid::FrameIdLayout;
use anyhow::{bail, Context, Result};
use cutline_deps::Toolchain;
use std::path::Path;
use std::process::Command;

pub fn run(mut cmd: Command) -> Result<()> {
    tracing::debug!(?cmd, "testkit");
    let out = cmd.output().context("spawning ffmpeg")?;
    if !out.status.success() {
        bail!(
            "ffmpeg failed: {:?}\n{}",
            cmd,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Parameters for a frame-ID test source.
#[derive(Debug, Clone)]
pub struct FrameIdSource {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration: f64,
    /// Background color (ffmpeg color name or 0xRRGGBB).
    pub color: String,
    /// Sine tone frequency for the audio track (None = no audio).
    pub tone_hz: Option<f64>,
    pub layout: FrameIdLayout,
}

impl FrameIdSource {
    pub fn new(width: u32, height: u32, fps: u32, duration: f64) -> Self {
        Self {
            width,
            height,
            fps,
            duration,
            color: "0x3060a0".into(),
            tone_hz: Some(440.0),
            layout: FrameIdLayout::top_row(),
        }
    }
}

/// Generate a frame-ID video (H.264 CRF 12, yuv420p, keyframe each second) with a sine tone.
pub fn frameid_video(tc: &Toolchain, out: &Path, s: &FrameIdSource) -> Result<()> {
    let (w, h, r, d) = (s.width, s.height, s.fps, s.duration);
    let l = &s.layout;
    // gradient background (luma ramp over the base color) + testsrc2 sanity region + ID blocks.
    let tw = (w / 4).max(160) & !1;
    let th = (h / 4).max(120) & !1;
    let (bx, by, bw, bh) = l.bbox();
    let (bwi, bhi) = (bw.round() as u32, bh.round() as u32);
    // ID blocks rendered on a small canvas (fast) and overlaid; local layout starts at 0,0.
    let local = FrameIdLayout {
        x0: 0.0,
        y0: 0.0,
        ..*l
    };
    let graph = format!(
        "gradients=s={w}x{h}:r={r}:d={d}:c0={c}:c1=0x101010:x0=0:y0=0:x1=0:y1={h}:speed=0.00001,format=yuv420p[bg];\
         testsrc2=s={tw}x{th}:r={r}:d={d},format=yuv420p[ts];\
         color=c=black:s={bwi}x{bhi}:r={r}:d={d},format=gray,geq=lum='{lum}',format=yuv420p[id];\
         [bg][ts]overlay=x={ox}:y={oy}:shortest=1[b1];[b1][id]overlay=x={bx}:y={by}:shortest=1,format=yuv420p[v]",
        c = s.color,
        ox = w - tw - 16,
        oy = h - th - 16,
        lum = local.geq_luma_expr(),
    );
    let mut c = tc.ffmpeg_cmd();
    c.args(["-filter_complex", &graph]);
    if let Some(hz) = s.tone_hz {
        c.args([
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency={hz}:sample_rate=48000:duration={d}"),
        ]);
    }
    c.args(["-map", "[v]"]);
    if s.tone_hz.is_some() {
        c.args(["-map", "0:a", "-ac", "2", "-c:a", "aac", "-b:a", "192k"]);
    }
    c.args([
        "-c:v",
        "libx264",
        "-crf",
        "12",
        "-preset",
        "veryfast",
        "-g",
        &r.to_string(),
        "-pix_fmt",
        "yuv420p",
    ]);
    c.args([
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
    ]);
    c.args(["-t", &d.to_string()]).arg(out);
    run(c)
}

/// Pure sine tone WAV/whatever extension.
pub fn tone(tc: &Toolchain, out: &Path, hz: f64, dur: f64, amp_db: f64) -> Result<()> {
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency={hz}:sample_rate=48000:duration={dur}"),
    ]);
    c.args([
        "-af",
        &format!("volume={amp_db}dB,aformat=channel_layouts=stereo"),
    ])
    .arg(out);
    run(c)
}

/// Pink-noise "music" bed.
pub fn noise_bed(tc: &Toolchain, out: &Path, dur: f64, amp: f64) -> Result<()> {
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &format!("anoisesrc=color=pink:sample_rate=48000:amplitude={amp}:duration={dur}:seed=7"),
    ]);
    c.args(["-af", "aformat=channel_layouts=stereo"]).arg(out);
    run(c)
}

/// "Speech-like" track: tone bursts at the given (start, end) spans, silence elsewhere.
pub fn bursts(tc: &Toolchain, out: &Path, dur: f64, spans: &[(f64, f64)], hz: f64) -> Result<()> {
    let expr = if spans.is_empty() {
        "0".to_string()
    } else {
        spans
            .iter()
            .map(|(a, b)| format!("between(t,{a},{b})"))
            .collect::<Vec<_>>()
            .join("+")
    };
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &format!("aevalsrc='0.5*sin(2*PI*{hz}*t)*gt({expr},0)':s=48000:d={dur}"),
    ]);
    c.args(["-af", "aformat=channel_layouts=stereo"]).arg(out);
    run(c)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaKind {
    ProRes4444,
    Vp9Alpha,
    PngSequence,
}

/// A red circle (alpha `a`/255 inside, 0 outside) moving left→right on a transparent canvas.
/// Radius r = h/4; center x = r + (w-2r)*t/dur, y = h/2.
pub fn alpha_overlay(
    tc: &Toolchain,
    out: &Path,
    kind: AlphaKind,
    w: u32,
    h: u32,
    fps: u32,
    dur: f64,
    a: u8,
) -> Result<()> {
    let rad = h as f64 / 4.0;
    let cx = format!("({rad}+({w}-2*{rad})*T/{dur})");
    let inside = format!("lte(hypot(X-{cx},Y-{cy}),{rad})", cy = h as f64 / 2.0);
    let graph = format!(
        "color=c=black@0:s={w}x{h}:r={fps}:d={dur},format=rgba,geq=r='255*{inside}':g='40*{inside}':b='40*{inside}':a='{a}*{inside}'"
    );
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &graph]);
    match kind {
        AlphaKind::ProRes4444 => {
            c.args([
                "-c:v",
                "prores_ks",
                "-profile:v",
                "4444",
                "-pix_fmt",
                "yuva444p10le",
                "-alpha_bits",
                "16",
            ]);
        }
        AlphaKind::Vp9Alpha => {
            c.args([
                "-c:v",
                "libvpx-vp9",
                "-pix_fmt",
                "yuva420p",
                "-b:v",
                "0",
                "-crf",
                "20",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
                "-auto-alt-ref",
                "0",
            ]);
        }
        AlphaKind::PngSequence => {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            c.args(["-c:v", "png", "-start_number", "0"]);
        }
    }
    c.arg(out);
    run(c)
}

/// High-contrast checkerboard grid (for motion smoothness measurement).
pub fn grid_video(
    tc: &Toolchain,
    out: &Path,
    w: u32,
    h: u32,
    fps: u32,
    dur: f64,
    cell: u32,
) -> Result<()> {
    let g = format!(
        "color=c=white:s={w}x{h}:r={fps}:d={dur},format=gray,geq=lum='255*mod(floor(X/{cell})+floor(Y/{cell}),2)',format=yuv420p"
    );
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &g,
        "-f",
        "lavfi",
        "-i",
        &format!("anullsrc=r=48000:cl=stereo:d={dur}"),
    ]);
    c.args([
        "-shortest",
        "-c:v",
        "libx264",
        "-crf",
        "12",
        "-preset",
        "veryfast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
    ])
    .arg(out);
    run(c)
}

/// Still image (solid color with a label-free gradient).
pub fn still(tc: &Toolchain, out: &Path, w: u32, h: u32, color: &str) -> Result<()> {
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &format!("color=c={color}:s={w}x{h}:d=1"),
        "-frames:v",
        "1",
    ])
    .arg(out);
    run(c)
}

/// Beep+flash pairs for A/V sync: white frame and a 1 kHz beep at each time in `at`.
pub fn beep_flash(
    tc: &Toolchain,
    out: &Path,
    w: u32,
    h: u32,
    fps: u32,
    dur: f64,
    at: &[f64],
) -> Result<()> {
    let fd = 1.0 / fps as f64;
    let on: Vec<String> = at
        .iter()
        .map(|t| format!("between(t,{t},{})", t + fd * 0.999))
        .collect();
    let on_a: Vec<String> = at
        .iter()
        .map(|t| format!("between(t,{t},{})", t + 0.05))
        .collect();
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &format!("color=c=black:s={w}x{h}:r={fps}:d={dur},format=yuv420p,geq=lum='16+219*gt({},0)':cb=128:cr=128", on.join("+")),
        "-f",
        "lavfi",
        "-i",
        &format!("aevalsrc='0.5*sin(2*PI*1000*t)*gt({},0)':s=48000:d={dur}", on_a.join("+")),
    ]);
    c.args([
        "-c:v", "libx264", "-crf", "12", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-c:a",
        "aac", "-ac", "2",
    ])
    .arg(out);
    run(c)
}
