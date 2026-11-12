//! Media-content QA: loudness, clipping, silence, black frames, freeze, first frame, hook,
//! captions (safe zone, reading speed).

use crate::qa::{Builder, QaContext, Sev};
use anyhow::Result;
use cutline_core::target::TargetSpec;
use cutline_deps::Toolchain;
use serde_json::json;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

fn overlaps(a: (f64, f64), spans: &[(f64, f64)]) -> bool {
    spans.iter().any(|s| a.0 < s.1 + 0.05 && a.1 > s.0 - 0.05)
}

/// Spans (start, end) from `<key>_start: x` / `<key>_end: y` log lines.
fn spans(log: &str, start_key: &str, end_key: &str) -> Vec<(f64, f64)> {
    let mut out = vec![];
    let mut cur: Option<f64> = None;
    for line in log.lines() {
        if let Some(p) = line.find(start_key) {
            cur = line[p + start_key.len()..].split_whitespace().next().and_then(|v| v.parse().ok());
        }
        if let Some(p) = line.find(end_key) {
            let e: Option<f64> =
                line[p + end_key.len()..].split_whitespace().next().and_then(|v| v.trim_end_matches('|').parse().ok());
            if let (Some(s), Some(e)) = (cur.take(), e) {
                out.push((s, e));
            }
        }
    }
    out
}

/// blackdetect lines carry start and end on one line.
fn black_spans(log: &str) -> Vec<(f64, f64)> {
    log.lines()
        .filter(|l| l.contains("black_start:"))
        .filter_map(|l| {
            let g = |k: &str| l.find(k).and_then(|p| l[p + k.len()..].split_whitespace().next()?.parse::<f64>().ok());
            Some((g("black_start:")?, g("black_end:")?))
        })
        .collect()
}

pub fn media_checks(tc: &Toolchain, file: &Path, t: &TargetSpec, ctx: &QaContext, b: &mut Builder) -> Result<()> {
    // One analysis pass over audio + video.
    let out = Command::new(&tc.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-nostats", "-i"])
        .arg(file)
        .args([
            "-filter_complex",
            "[0:v]scale=320:-2,blackdetect=d=0.3:pix_th=0.08,freezedetect=n=-55dB:d=1.0[vo];\
             [0:a]ebur128=peak=true:framelog=quiet,silencedetect=n=-70dB:d=1.5[ao]",
            "-map",
            "[vo]",
            "-map",
            "[ao]",
            "-f",
            "null",
            "-",
        ])
        .output()?;
    let log = String::from_utf8_lossy(&out.stderr).to_string();
    anyhow::ensure!(
        out.status.success(),
        "QA analysis pass failed: {}",
        log.lines().rev().take(5).collect::<Vec<_>>().join(" | ")
    );
    let dur = ctx.expected_duration.unwrap_or(0.0);

    // loudness
    if let Some(l) = cutline_media::probe::parse_ebur128_summary(&log) {
        if l.integrated_lufs <= -69.0 {
            b.add(
                "loudness",
                Sev::Warn,
                false,
                "no audible audio (silent mix)",
                json!({"integrated": l.integrated_lufs}),
                json!("audible audio"),
            );
        } else if let Some((ti, ttp)) = ctx.loudness_target {
            let ok = (l.integrated_lufs - ti).abs() <= 1.0 && l.true_peak_dbtp <= ttp + 0.05;
            b.add(
                "loudness",
                Sev::Error,
                ok,
                format!("I={:.1} LUFS, TP={:.1} dBTP (target {ti} / ≤{ttp})", l.integrated_lufs, l.true_peak_dbtp),
                json!({"integrated": l.integrated_lufs, "true_peak": l.true_peak_dbtp, "lra": l.lra}),
                json!({"integrated": ti, "tolerance_lu": 1.0, "true_peak_max": ttp}),
            );
        }
        let ok = l.lra <= t.lra_max;
        b.add(
            "loudness_range",
            Sev::Warn,
            ok,
            format!("LRA {:.1} LU (≤ {})", l.lra, t.lra_max),
            json!(l.lra),
            json!(t.lra_max),
        );
    }

    // clipping: sample peaks ≥ −0.1 dBFS
    let clip = count_clipping(tc, file)?;
    b.add("clipping", Sev::Warn, clip == 0, format!("{clip} samples ≥ −0.1 dBFS"), json!(clip), json!(0));

    // silence
    let sil: Vec<(f64, f64)> = spans(&log, "silence_start:", "silence_end:")
        .into_iter()
        .filter(|s| !overlaps(*s, &ctx.intended_silence))
        .collect();
    b.add(
        "silence",
        Sev::Warn,
        sil.is_empty(),
        if sil.is_empty() {
            "no digital silence > 1.5 s".to_string()
        } else {
            format!("{} silent spans > 1.5 s", sil.len())
        },
        json!(sil),
        json!([]),
    );

    // black frames
    let black: Vec<(f64, f64)> = black_spans(&log).into_iter().filter(|s| !overlaps(*s, &ctx.intended_black)).collect();
    b.add(
        "black_frames",
        Sev::Warn,
        black.is_empty(),
        if black.is_empty() {
            "no black spans > 0.3 s".to_string()
        } else {
            format!("{} black spans > 0.3 s", black.len())
        },
        json!(black),
        json!([]),
    );

    // freeze
    let mut fr: Vec<(f64, f64)> = spans(&log, "freeze_start:", "freeze_end:");
    // a freeze lasting until the end has no end line
    if let Some(p) = log.rfind("freeze_start:") {
        if !log[p..].contains("freeze_end:") {
            if let Some(s) = log[p + 13..].split_whitespace().next().and_then(|v| v.parse().ok()) {
                fr.push((s, dur));
            }
        }
    }
    let fr: Vec<(f64, f64)> = fr.into_iter().filter(|s| !overlaps(*s, &ctx.intended_freeze)).collect();
    b.add(
        "freeze",
        Sev::Warn,
        fr.is_empty(),
        if fr.is_empty() { "no frozen spans > 1 s".to_string() } else { format!("{} frozen spans > 1 s", fr.len()) },
        json!(fr),
        json!([]),
    );

    // first frame not black/blank
    let (mean, var) = first_frame_stats(tc, file)?;
    let ok = mean > 0.06 && var > 0.0004;
    b.add(
        "first_frame",
        Sev::Warn,
        ok,
        format!("first frame mean luma {mean:.2}, variance {var:.4}"),
        json!({"mean": mean, "variance": var}),
        json!({"mean_min": 0.06, "variance_min": 0.0004}),
    );

    // hook (shorts): text visible within 1 s
    if ctx.is_shorts {
        let ok = ctx.first_text_at.map(|t| t <= 1.0).unwrap_or(false);
        b.add(
            "hook",
            Sev::Warn,
            ok,
            match ctx.first_text_at {
                Some(t) => format!("first caption/text at {t:.2}s"),
                None => "no caption or text in the first second".into(),
            },
            json!(ctx.first_text_at),
            json!({"max_s": 1.0}),
        );
    }

    // captions: safe zone + reading speed
    if !ctx.caption_boxes.is_empty() {
        let (sx, sy, sw, sh) = t.safe_rect();
        let outside: Vec<_> = ctx
            .caption_boxes
            .iter()
            .filter(|c| c.x < sx - 1.0 || c.y < sy - 1.0 || c.x + c.w > sx + sw + 1.0 || c.y + c.h > sy + sh + 1.0)
            .map(|c| json!({"text": c.text, "start": c.start, "box": [c.x, c.y, c.w, c.h]}))
            .collect();
        b.add(
            "captions_safe_zone",
            Sev::Warn,
            outside.is_empty(),
            format!("{} of {} boxes outside the safe zone", outside.len(), ctx.caption_boxes.len()),
            json!(outside),
            json!({"safe": [sx, sy, sw, sh]}),
        );
        let fast: Vec<_> = ctx
            .caption_boxes
            .iter()
            .filter(|c| c.kind == "caption")
            .filter(|c| {
                let chars = c.text.chars().filter(|ch| !ch.is_whitespace()).count() as f64;
                let d = (c.end - c.start).max(0.05);
                chars / d > ctx.max_cps && chars > 3.0
            })
            .map(|c| json!({"text": c.text, "start": c.start, "cps": c.text.chars().filter(|ch| !ch.is_whitespace()).count() as f64 / (c.end - c.start).max(0.05)}))
            .collect();
        b.add(
            "captions_cps",
            Sev::Warn,
            fast.is_empty(),
            format!("{} captions over {} chars/s", fast.len(), ctx.max_cps),
            json!(fast),
            json!({"max_cps": ctx.max_cps}),
        );
    }
    Ok(())
}

fn count_clipping(tc: &Toolchain, file: &Path) -> Result<u64> {
    let mut child = Command::new(&tc.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(file)
        .args(["-vn", "-f", "f32le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut so = child.stdout.take().unwrap();
    let thr = 10f32.powf(-0.1 / 20.0);
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    let mut carry: Vec<u8> = vec![];
    loop {
        let k = so.read(&mut buf)?;
        if k == 0 {
            break;
        }
        carry.extend_from_slice(&buf[..k]);
        let whole = carry.len() / 4 * 4;
        for c in carry[..whole].chunks_exact(4) {
            if f32::from_le_bytes([c[0], c[1], c[2], c[3]]).abs() >= thr {
                n += 1;
            }
        }
        carry.drain(..whole);
    }
    child.wait()?;
    Ok(n)
}

fn first_frame_stats(tc: &Toolchain, file: &Path) -> Result<(f64, f64)> {
    let out = Command::new(&tc.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-vf", "scale=64:64,format=gray", "-f", "rawvideo", "-"])
        .output()?;
    let px: Vec<f64> = out.stdout.iter().map(|v| *v as f64 / 255.0).collect();
    if px.is_empty() {
        return Ok((0.0, 0.0));
    }
    let m = px.iter().sum::<f64>() / px.len() as f64;
    let v = px.iter().map(|x| (x - m).powi(2)).sum::<f64>() / px.len() as f64;
    Ok((m, v))
}
