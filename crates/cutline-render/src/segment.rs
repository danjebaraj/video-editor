//! Segment = one main clip (with its geometry, color and effects) rendered to a mezzanine.

use crate::escape::f;
use crate::job::{base_args, Job};
use crate::{effects, Geometry};
use cutline_core::model::{AssetKind, ReframeMode};
use cutline_core::timeline::TClip;
use cutline_media::assets::AssetMedia;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const SAMPLE_RATE: i64 = 48_000;

/// Samples at the start of timeline frame `fr`.
pub fn samples_at(fr: i64, g: &Geometry) -> i64 {
    ((fr as f64) * SAMPLE_RATE as f64 / g.fps.as_f64()).round() as i64
}

#[derive(Debug, Clone, Serialize)]
pub struct SegmentSpec {
    pub clip_id: String,
    pub index: usize,
    pub frames: i64,
    pub samples: i64,
    /// Input args (before the filter script), referencing the asset path.
    pub input_args: Vec<String>,
    pub filter: String,
    /// Extra files the filter depends on (sendcmd, LUTs) — hashed into the key.
    pub deps: Vec<PathBuf>,
    pub has_source_audio: bool,
}

/// Extra per-clip inputs computed outside (reframe crop paths etc.).
#[derive(Debug, Clone, Default)]
pub struct ClipExtras {
    /// Geometry chain replacing the default (e.g. track_face crop via sendcmd).
    pub geometry_override: Option<String>,
    pub deps: Vec<PathBuf>,
    /// Resolved LUT file path, if any.
    pub lut_path: Option<PathBuf>,
    /// Pad color for fit_pad (brand background).
    pub pad_color: String,
}

fn atempo_chain(speed: f64) -> Vec<String> {
    let mut v = vec![];
    let mut s = speed;
    while s > 2.0 {
        v.push("atempo=2".to_string());
        s /= 2.0;
    }
    while s < 0.5 {
        v.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    if (s - 1.0).abs() > 1e-9 {
        v.push(format!("atempo={}", f(s)));
    }
    v
}

/// Geometry chain for a source of `sw×sh` into the target. Returns filtergraph text taking
/// `[{inp}]` and producing `[{out}]`.
pub fn geometry_chain(
    mode: ReframeMode,
    sw: u32,
    sh: u32,
    g: &Geometry,
    inp: &str,
    out: &str,
    pad_color: &str,
) -> String {
    let (w, h) = (g.width, g.height);
    let same_aspect = sw > 0 && sh > 0 && ((sw as f64 / sh as f64) - (w as f64 / h as f64)).abs() < 0.01;
    let cs = "in_color_matrix=auto:in_range=auto:out_color_matrix=bt709:out_range=tv";
    if same_aspect {
        return format!("[{inp}]scale={w}:{h}:flags=lanczos:{cs},setsar=1[{out}]");
    }
    match mode {
        ReframeMode::FitPad => format!(
            "[{inp}]scale={w}:{h}:force_original_aspect_ratio=decrease:force_divisible_by=2:flags=lanczos:{cs},pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color={pad_color},setsar=1[{out}]"
        ),
        ReframeMode::FitBlur => format!(
            "[{inp}]scale=w=iw:h=ih:{cs},split=2[{out}_a][{out}_b];\
             [{out}_a]scale={bw}:{bh}:force_original_aspect_ratio=increase:force_divisible_by=2,crop={bw}:{bh},boxblur=10:2,eq=brightness=-0.06:saturation=1.1,scale={w}:{h}:flags=bicubic,setsar=1[{out}_bg];\
             [{out}_b]scale={w}:{h}:force_original_aspect_ratio=decrease:force_divisible_by=2:flags=lanczos,setsar=1[{out}_fg];\
             [{out}_bg][{out}_fg]overlay=(W-w)/2:(H-h)/2[{out}]",
            bw = (w / 8) & !1,
            bh = (h / 8) & !1,
        ),
        _ => format!(
            "[{inp}]scale={w}:{h}:force_original_aspect_ratio=increase:force_divisible_by=2:flags=lanczos:{cs},crop={w}:{h},setsar=1[{out}]"
        ),
    }
}

/// Build the segment spec for a clip.
pub fn build(
    c: &TClip,
    m: &AssetMedia,
    g: &Geometry,
    default_mode: ReframeMode,
    extras: &ClipExtras,
    start_samples: i64,
    end_samples: i64,
) -> SegmentSpec {
    let fps = g.fps;
    let frames = c.frames;
    let samples = end_samples - start_samples;
    let mut input = vec![];
    let src_fps = m.facts.fps.unwrap_or(fps);
    let is_still = matches!(c.kind, AssetKind::Image);
    let dur_secs = fps.frames_to_secs(frames);
    let path = m.path.to_string_lossy().to_string();
    if is_still {
        input.extend(["-loop", "1", "-framerate", &fps.ff(), "-t", &f(dur_secs + 0.5), "-i", &path].map(String::from));
    } else if c.kind == AssetKind::ImageSequence {
        let sf = src_fps.as_f64();
        let ss = (c.src_in * sf).round() / sf;
        input.extend(
            ["-framerate", &src_fps.ff(), "-ss", &f(ss), "-t", &f(c.src_out - ss + 2.0 / sf), "-i", &path]
                .map(String::from),
        );
    } else {
        // Snap in-point to the source frame grid; seek a quarter frame early so the in-frame is
        // kept and the previous one discarded.
        let sf = src_fps.as_f64();
        let in_snap = (c.src_in * sf).round() / sf;
        let seek = (in_snap - 0.25 / sf).max(0.0);
        let len = c.src_out - seek + 2.0 / sf;
        if m.info.as_ref().and_then(|i| i.video.as_ref()).map(|v| v.needs_libvpx_decoder).unwrap_or(false) {
            input.extend(["-c:v", "libvpx-vp9"].map(String::from));
        }
        input.extend(["-ss", &f(seek), "-t", &f(len), "-i", &path].map(String::from));
    }
    let has_audio = m.facts.has_audio && !is_still && c.kind != AssetKind::ImageSequence;
    let mode = c.clip.reframe.as_ref().map(|r| r.mode).unwrap_or(default_mode);

    // ---------------- video
    let mut v = vec![];
    let mut pre = vec!["setpts=PTS-STARTPTS".to_string()];
    if c.clip.reverse {
        pre.push("reverse".into());
        pre.push("setpts=PTS-STARTPTS".into());
    }
    if !c.clip.speed_ramp.is_empty() {
        pre.push(effects::speed_ramp_setpts(&c.clip.speed_ramp));
    } else if (c.speed - 1.0).abs() > 1e-9 {
        pre.push(format!("setpts=PTS/{}", f(c.speed)));
    }
    pre.push(format!("fps={}", fps.ff()));
    v.push(format!("[0:v]{}[vn]", pre.join(",")));
    let (sw, sh) = (m.facts.width, m.facts.height);
    match &extras.geometry_override {
        Some(go) => v.push(go.replace("{in}", "vn").replace("{out}", "vg")),
        None => v.push(geometry_chain(mode, sw, sh, g, "vn", "vg", &extras.pad_color)),
    }
    let post = effects::video_chain(c, g, extras);
    let freeze = effects::freeze_chain(c, g);
    let mut tail = vec![];
    tail.extend(post);
    tail.extend(freeze);
    tail.push(format!("tpad=stop_mode=clone:stop={}", frames));
    tail.push(format!("trim=end_frame={frames}"));
    tail.push("setpts=PTS-STARTPTS".into());
    tail.push("format=yuv422p10le".into());
    v.push(format!("[vg]{}[vout]", tail.join(",")));

    // ---------------- audio (dialogue)
    let mut a = vec![];
    let mute = matches!(c.clip.audio.as_deref(), Some(x) if x != "keep" && x != "duck");
    let secs = samples as f64 / SAMPLE_RATE as f64;
    let mf = 0.01f64.min(secs / 4.0);
    if has_audio && !mute {
        let mut ch = vec!["asetpts=PTS-STARTPTS".to_string()];
        if c.clip.reverse {
            ch.push("areverse".into());
        }
        let sp = if c.clip.speed_ramp.is_empty() {
            c.speed
        } else {
            (c.src_out - c.src_in) / fps.frames_to_secs(frames).max(1e-6)
        };
        ch.extend(atempo_chain(sp));
        ch.push(format!("aresample={SAMPLE_RATE}"));
        ch.push("aformat=sample_fmts=flt:channel_layouts=stereo".into());
        let mut gain = c.clip.gain_db.unwrap_or(0.0);
        if c.clip.audio.as_deref() == Some("duck") {
            gain -= 12.0;
        }
        if gain.abs() > 1e-9 {
            ch.push(format!("volume={}dB", f(gain)));
        }
        ch.extend(effects::freeze_audio(c, g));
        ch.push(format!("apad=whole_len={samples}"));
        ch.push(format!("atrim=end_sample={samples}"));
        ch.push(format!("afade=t=in:d={}", f(mf)));
        ch.push(format!("afade=t=out:st={}:d={}", f(secs - mf), f(mf)));
        a.push(format!("[0:a]{}[aout]", ch.join(",")));
    } else {
        a.push(format!("anullsrc=r={SAMPLE_RATE}:cl=stereo,atrim=end_sample={samples},aformat=sample_fmts=flt:channel_layouts=stereo[aout]"));
    }
    let mut filter = v.join(";\n");
    filter.push_str(";\n");
    filter.push_str(&a.join(";\n"));
    filter.push('\n');
    let mut deps = extras.deps.clone();
    if let Some(l) = &extras.lut_path {
        deps.push(l.clone());
    }
    SegmentSpec {
        clip_id: c.id.clone(),
        index: c.index,
        frames,
        samples,
        input_args: input,
        filter,
        deps,
        has_source_audio: has_audio,
    }
}

/// Job that renders the segment mezzanine (ProRes 422 HQ + PCM f32).
pub fn job(s: &SegmentSpec, tc: &cutline_deps::Toolchain, script: &Path, out_tmp: &Path, draft: bool) -> Job {
    let mut j = Job::new(format!("segment {}", s.clip_id));
    j.args(base_args(false));
    j.args(s.input_args.clone());
    j.args(tc.filter_complex_file_args(script));
    j.args(["-map", "[vout]", "-map", "[aout]"]);
    if draft {
        j.args(["-c:v", "prores_ks", "-profile:v", "0"]);
    } else {
        j.args(["-c:v", "prores_ks", "-profile:v", "3"]);
    }
    j.args(["-vendor", "apl0", "-pix_fmt", "yuv422p10le", "-bits_per_mb", "8000"]);
    j.args(["-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-color_range", "tv"]);
    j.args(["-c:a", "pcm_f32le", "-ar", "48000", "-ac", "2"]);
    j.args(["-map_metadata", "-1", "-fflags", "+bitexact", "-flags:v", "+bitexact", "-flags:a", "+bitexact"]);
    j.args(["-frames:v", &s.frames.to_string()]);
    j.arg(out_tmp.to_string_lossy().to_string());
    j.scripts.push((script.to_path_buf(), s.filter.clone()));
    j.outputs.push(out_tmp.to_path_buf());
    j
}
