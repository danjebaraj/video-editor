//! Post-render QA (§7.2).

use anyhow::Result;
use cutline_core::target::TargetSpec;
use cutline_deps::Toolchain;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Sev {
    Error,
    Warn,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub check: String,
    pub severity: Sev,
    pub ok: bool,
    pub message: String,
    pub measured: Value,
    pub expected: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct QaReport {
    pub file: PathBuf,
    pub target: String,
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub findings: Vec<Finding>,
}

/// What QA knows about the intended edit.
#[derive(Debug, Clone, Default)]
pub struct QaContext {
    pub expected_duration: Option<f64>,
    pub loudness_target: Option<(f64, f64)>,
    /// Timeline spans where black/silence/freeze is intended (fades, freeze effects).
    pub intended_black: Vec<(f64, f64)>,
    pub intended_freeze: Vec<(f64, f64)>,
    pub intended_silence: Vec<(f64, f64)>,
    /// Caption/text boxes (start, end, x, y, w, h) in output pixels, and their text.
    pub caption_boxes: Vec<CaptionBox>,
    /// Timeline seconds where text/captions first appear (for the hook check).
    pub first_text_at: Option<f64>,
    pub is_shorts: bool,
    pub max_cps: f64,
    /// `profile: screencast`: frozen spans are normal (pauses between keystrokes).
    pub screencast: bool,
    /// `chroma: "444"`: a 4:4:4 High 4:4:4 Predictive stream is also correct.
    pub chroma444: bool,
    /// Delivered as WebM (VP9 + Opus) instead of MP4 (H.264 + AAC).
    pub webm: bool,
    /// The export has no audio track by design.
    pub no_audio: bool,
    /// Size cap in bytes.
    pub max_bytes: Option<u64>,
    /// The export should loop seamlessly (last frame leads into the first).
    pub looped: bool,
    /// Spans of speech (file seconds) in an export delivered without audio.
    pub muted_speech: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptionBox {
    pub start: f64,
    pub end: f64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub text: String,
    /// `caption` or `text`.
    pub kind: String,
}

pub struct Builder {
    pub f: Vec<Finding>,
}

impl Builder {
    pub fn add(
        &mut self,
        check: &str,
        sev: Sev,
        ok: bool,
        message: impl Into<String>,
        measured: Value,
        expected: Value,
    ) {
        self.f.push(Finding { check: check.into(), severity: sev, ok, message: message.into(), measured, expected });
    }
}

fn ffprobe_json(tc: &Toolchain, p: &Path) -> Result<Value> {
    let out = tc.ffprobe_cmd().args(["-print_format", "json", "-show_format", "-show_streams"]).arg(p).output()?;
    anyhow::ensure!(out.status.success(), "ffprobe failed: {}", String::from_utf8_lossy(&out.stderr));
    Ok(serde_json::from_slice(&out.stdout)?)
}

fn fnum(v: &Value) -> Option<f64> {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64())
}

pub fn run_qa(tc: &Toolchain, file: &Path, t: &TargetSpec, ctx: &QaContext) -> Result<QaReport> {
    let mut b = Builder { f: vec![] };
    let j = ffprobe_json(tc, file)?;
    let streams = j["streams"].as_array().cloned().unwrap_or_default();
    let v = streams.iter().find(|s| s["codec_type"] == "video").cloned().unwrap_or(Value::Null);
    let a = streams.iter().find(|s| s["codec_type"] == "audio").cloned().unwrap_or(Value::Null);
    let fmt = &j["format"];

    // container / codec
    let fmt_name = fmt["format_name"].as_str().unwrap_or("");
    let faststart = cutline_media::probe::moov_before_mdat(file).unwrap_or(false);
    let measured = json!({
        "format": fmt_name, "vcodec": v["codec_name"], "profile": v["profile"], "pix_fmt": v["pix_fmt"],
        "acodec": a["codec_name"], "aprofile": a["profile"], "sample_rate": a["sample_rate"], "channels": a["channels"], "faststart": faststart
    });
    let is444 = ctx.chroma444 && v["profile"] == "High 4:4:4 Predictive" && v["pix_fmt"] == "yuv444p";
    let silent_ok = ctx.no_audio && a.is_null();
    if ctx.webm {
        let ok = fmt_name.contains("webm")
            && v["codec_name"] == "vp9"
            && v["pix_fmt"] == "yuv420p"
            && (silent_ok
                || (!ctx.no_audio && a["codec_name"] == "opus" && a["sample_rate"] == "48000" && a["channels"] == 2));
        let want = if ctx.no_audio { "WebM / VP9 yuv420p / no audio" } else { "WebM / VP9 yuv420p / Opus 48k stereo" };
        b.add(
            "container_codec",
            Sev::Error,
            ok,
            if ok { want } else { "container or codec settings differ from the delivery profile" },
            measured,
            json!({"format": "webm", "vcodec": "vp9", "pix_fmt": "yuv420p", "acodec": if ctx.no_audio { Value::Null } else { json!("opus") }}),
        );
    } else {
        let audio_ok = silent_ok
            || (!ctx.no_audio
                && a["codec_name"] == "aac"
                && a["profile"] == "LC"
                && a["sample_rate"] == "48000"
                && a["channels"] == 2);
        let ok = fmt_name.contains("mp4")
            && v["codec_name"] == "h264"
            && ((v["profile"] == "High" && v["pix_fmt"] == "yuv420p") || is444)
            && audio_ok
            && faststart;
        let summary = if !ok {
            "container or codec settings differ from the delivery profile"
        } else if ctx.no_audio {
            "MP4 / H.264 High yuv420p / no audio / faststart"
        } else if is444 {
            "MP4 / H.264 High 4:4:4 yuv444p / AAC-LC 48k stereo / faststart"
        } else {
            "MP4 / H.264 High yuv420p / AAC-LC 48k stereo / faststart"
        };
        b.add("container_codec", Sev::Error, ok, summary, measured,
            json!({"format": "mp4", "vcodec": "h264", "profile": "High", "pix_fmt": "yuv420p", "acodec": "aac", "aprofile": "LC", "sample_rate": "48000", "channels": 2, "faststart": true}));
    }
    // size cap
    if let Some(max) = ctx.max_bytes {
        let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
        b.add(
            "file_size",
            Sev::Error,
            size <= max,
            format!("{:.2} MB (cap {:.2} MB)", size as f64 / 1e6, max as f64 / 1e6),
            json!(size),
            json!({"max_bytes": max}),
        );
    }

    // geometry / fps
    let w = v["width"].as_u64().unwrap_or(0) as u32;
    let h = v["height"].as_u64().unwrap_or(0) as u32;
    let rfr = v["r_frame_rate"].as_str().unwrap_or("0/1");
    let afr = v["avg_frame_rate"].as_str().unwrap_or("0/1");
    let rf = cutline_media::probe::parse_rational(rfr).unwrap_or(0.0);
    let af = cutline_media::probe::parse_rational(afr).unwrap_or(0.0);
    let want = t.fps.as_f64();
    let ok = w == t.width && h == t.height && (rf - want).abs() < 0.01 && (af - want).abs() < 0.02;
    b.add(
        "geometry_fps",
        Sev::Error,
        ok,
        format!("{w}x{h} @ {af:.3} fps (CFR {})", (rf - af).abs() < 0.02),
        json!({"width": w, "height": h, "r_frame_rate": rfr, "avg_frame_rate": afr}),
        json!({"width": t.width, "height": t.height, "fps": t.fps.ff()}),
    );

    // color tags
    let ok = v["color_primaries"] == "bt709"
        && v["color_transfer"] == "bt709"
        && v["color_space"] == "bt709"
        && v["color_range"] == "tv";
    b.add("color_tags", Sev::Error, ok, "BT.709 primaries/transfer/matrix, tv range",
        json!({"primaries": v["color_primaries"], "transfer": v["color_transfer"], "matrix": v["color_space"], "range": v["color_range"]}),
        json!({"primaries": "bt709", "transfer": "bt709", "matrix": "bt709", "range": "tv"}));

    // duration (WebM keeps it on the container, not the streams)
    let vdur = fnum(&v["duration"]).or_else(|| fnum(&fmt["duration"])).unwrap_or(0.0);
    let adur = fnum(&a["duration"]).or_else(|| fnum(&fmt["duration"])).unwrap_or(0.0);
    let frame = 1.0 / want;
    if let Some(ed) = ctx.expected_duration {
        let ok = (vdur - ed).abs() <= frame + 1e-3;
        b.add(
            "duration",
            Sev::Error,
            ok,
            format!("video {vdur:.3}s vs timeline {ed:.3}s"),
            json!(vdur),
            json!({"value": ed, "tolerance_s": frame}),
        );
    }
    if let Some(maxd) = t.max_duration {
        let ok = vdur <= maxd;
        b.add("duration_limit", Sev::Error, ok, format!("{vdur:.1}s (limit {maxd:.0}s)"), json!(vdur), json!(maxd));
    }

    // A/V alignment
    let vs = fnum(&v["start_time"]).unwrap_or(0.0);
    let as_ = fnum(&a["start_time"]).unwrap_or(0.0);
    let ok = vs.abs() < 1e-3 && as_.abs() < 0.03 && (vdur - adur).abs() <= frame + 0.03;
    if !ctx.no_audio {
        b.add(
            "av_alignment",
            Sev::Error,
            ok,
            format!("starts v={vs:.3} a={as_:.3}, durations v={vdur:.3} a={adur:.3}"),
            json!({"video_start": vs, "audio_start": as_, "video_duration": vdur, "audio_duration": adur}),
            json!({"start": 0, "tolerance_s": frame}),
        );
    }

    // bitrate (info)
    if let Some(br) = fnum(&v["bit_rate"]).or_else(|| fnum(&fmt["bit_rate"])) {
        let kbps = br / 1000.0;
        let ok = kbps >= t.bitrate_kbps.0 as f64 && kbps <= t.bitrate_kbps.1 as f64;
        b.add("bitrate", Sev::Info, ok, format!("{kbps:.0} kb/s"), json!(kbps), json!(t.bitrate_kbps));
    }

    crate::qa_media::media_checks(tc, file, t, ctx, &mut b)?;

    let errors = b.f.iter().filter(|x| !x.ok && x.severity == Sev::Error).count();
    let warnings = b.f.iter().filter(|x| !x.ok && x.severity == Sev::Warn).count();
    Ok(QaReport {
        file: file.to_path_buf(),
        target: cutline_core::target::target_str(t.name).to_string(),
        ok: errors == 0,
        errors,
        warnings,
        findings: b.f,
    })
}

pub fn summary(r: &QaReport) -> String {
    let mut s = format!("QA {}: {} errors, {} warnings\n", if r.ok { "PASS" } else { "FAIL" }, r.errors, r.warnings);
    for f in &r.findings {
        let mark = if f.ok {
            "✔"
        } else {
            match f.severity {
                Sev::Error => "✘",
                Sev::Warn => "⚠",
                Sev::Info => "·",
            }
        };
        s.push_str(&format!("  {mark} {:<18} {}\n", f.check, f.message));
    }
    s
}
