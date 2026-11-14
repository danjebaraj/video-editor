//! Timeline → render plan → ffmpeg commands/filter scripts; segment renderer; encoder profiles.

pub mod assemble;
pub mod audio_plan;
pub mod auto;
pub mod beatrefs;
pub mod cues;
pub mod custom_transitions;
pub mod effects;
pub mod escape;
pub mod extras;
pub mod job;
pub mod overlay;
pub mod pipeline;
pub mod reframe;
pub mod segment;
pub mod terminal;
pub mod transitions;

use anyhow::{Context, Result};
use cutline_core::model::{Project, ReframeMode, TargetName};
use cutline_core::target::target_str;
use cutline_core::time::Fps;
use cutline_core::timeline::Timeline;
use cutline_deps::Toolchain;
use cutline_media::assets::ProjectMedia;
use job::{base_args, Job};
use rayon::prelude::*;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub fps: Fps,
}

impl Geometry {
    /// Draft geometry: shorter side 540 px.
    pub fn draft(&self) -> Self {
        let s = 540.0 / self.width.min(self.height) as f64;
        let ev = |v: f64| ((v.round() as u32) + 1) & !1;
        Self { width: ev(self.width as f64 * s), height: ev(self.height as f64 * s), fps: self.fps }
    }
    /// Scale factor relative to the full target width.
    pub fn scale_to(&self, full_w: u32) -> f64 {
        self.width as f64 / full_w as f64
    }
}

#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    pub draft: bool,
    pub no_cache: bool,
    pub jobs: usize,
    pub hw: bool,
    /// Override output path.
    pub out: Option<PathBuf>,
    /// Skip loudness normalization (preview).
    pub skip_loudness: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedSegment {
    pub clip_id: String,
    pub key: String,
    pub mezzanine: PathBuf,
    pub frames: i64,
    pub job: Job,
    #[serde(skip)]
    pub spec: segment::SegmentSpec,
}

/// Everything needed to render (and to print with --dry-run).
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub name: String,
    pub target: String,
    pub geometry: Geometry,
    /// Full-size target geometry (differs from `geometry` in draft mode).
    pub full: Geometry,
    pub draft: bool,
    pub project_dir: PathBuf,
    pub work_dir: PathBuf,
    pub segments: Vec<PlannedSegment>,
    pub pre_jobs: Vec<Job>,
    pub audio: audio_plan::AudioPlan,
    pub final_job: Job,
    pub output: PathBuf,
    pub duration: f64,
    pub frames: i64,
    #[serde(skip)]
    pub timeline: Timeline,
}

pub fn output_name(p: &Project, project_path: &Path) -> String {
    p.name.clone().unwrap_or_else(|| {
        project_path.file_stem().and_then(|s| s.to_str()).unwrap_or("video").trim_end_matches(".json5").to_string()
    })
}

/// `.cutline` working dir next to the project file.
pub fn cutline_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(".cutline")
}

/// Build the render plan. `export_name` is used in the output name for multi-export.
pub fn build_plan(
    tc: &Toolchain,
    p: &Project,
    project_path: &Path,
    media: &ProjectMedia,
    tl: &Timeline,
    opts: &RenderOptions,
    export_name: Option<&str>,
    extras: &PlanExtras,
) -> Result<Plan> {
    let project_dir = project_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let full = Geometry { width: tl.target.width, height: tl.target.height, fps: tl.fps };
    let g = if opts.draft { full.draft() } else { full };
    let cdir = cutline_dir(&project_dir);
    let seg_dir = cdir.join("cache").join("segments");
    let run_id = format!("{}-{}", std::process::id(), export_name.unwrap_or("main"));
    let work = cdir.join("tmp").join(run_id);
    let name = output_name(p, project_path);
    let default_mode = p.default_reframe.as_ref().map(|r| r.mode).unwrap_or(ReframeMode::Cover);
    let mut segments = vec![];
    for c in &tl.clips {
        let m = media.get(&c.asset).with_context(|| format!("no media for asset {}", c.asset))?;
        let ex = extras
            .clip
            .get(&c.id)
            .cloned()
            .unwrap_or_else(|| segment::ClipExtras { pad_color: extras.pad_color.clone(), ..Default::default() });
        let spec = segment::build(
            c,
            m,
            &g,
            default_mode,
            &ex,
            segment::samples_at(c.start, &g),
            segment::samples_at(c.end(), &g),
        );
        let mut dep_fps = vec![];
        for d in &spec.deps {
            dep_fps.push(cutline_media::fingerprint(d).unwrap_or_default());
        }
        let inputs_norm: Vec<String> = spec
            .input_args
            .iter()
            .map(|a| if a == &m.path.to_string_lossy() { m.fingerprint.clone() } else { a.clone() })
            .collect();
        let filter_norm = normalize_paths(&spec.filter, &[&project_dir, &cdir]);
        let key = cutline_media::hash_key(&[
            m.fingerprint.as_bytes(),
            filter_norm.as_bytes(),
            inputs_norm.join(" ").as_bytes(),
            dep_fps.join(",").as_bytes(),
            format!("{}x{}@{}", g.width, g.height, g.fps).as_bytes(),
            cutline_core::VERSION.as_bytes(),
            tc.version.as_bytes(),
            if opts.draft { b"draft" } else { b"final" },
            b"seg-v1",
        ]);
        let mezz = seg_dir.join(format!("{key}.mov"));
        let script = work.join(format!("seg_{:03}_{}.filter", c.index, sanitize(&c.id)));
        let tmp_out = seg_dir.join(format!("{key}.tmp.mov"));
        let job = segment::job(&spec, tc, &script, &tmp_out, opts.draft);
        segments.push(PlannedSegment { clip_id: c.id.clone(), key, mezzanine: mezz, frames: c.frames, job, spec });
    }
    let mezz: Vec<PathBuf> = segments.iter().map(|s| s.mezzanine.clone()).collect();
    let audio = audio_plan::plan(tc, p, tl, &g, &mezz, media, &work, &project_dir, extras, opts)?;
    let output = match &opts.out {
        Some(o) => o.clone(),
        None => {
            let suffix = export_name.map(|e| format!("-{e}")).unwrap_or_default();
            let draft = if opts.draft { "-draft" } else { "" };
            project_dir.join("out").join(format!("{name}{suffix}{draft}.mp4"))
        }
    };
    let (pre_jobs, final_job) =
        final_job(tc, tl, &g, &full, &mezz, &audio.final_wav, &work, &output, opts, extras, p, media)?;
    Ok(Plan {
        name,
        target: target_str(tl.target.name).to_string(),
        geometry: g,
        full,
        draft: opts.draft,
        project_dir,
        work_dir: work,
        segments,
        pre_jobs,
        audio,
        final_job,
        output,
        duration: tl.duration(),
        frames: tl.total_frames,
        timeline: tl.clone(),
    })
}

/// Inputs from other subsystems (text/captions/reframe/brand) that the plan needs.
#[derive(Debug, Clone, Default)]
pub struct PlanExtras {
    pub clip: std::collections::BTreeMap<String, segment::ClipExtras>,
    pub pad_color: String,
    /// ASS documents to burn in (text templates, captions): (file name, content), in order.
    pub ass_docs: Vec<(String, String)>,
    /// Image overlays produced by templates (emoji pops).
    pub extra_images: Vec<ExtraImage>,
    pub fonts_dir: Option<PathBuf>,
    /// Accent color (`0xRRGGBB`) for the progress bar.
    pub accent: String,
    /// Speech activity (timeline seconds) for ducking.
    pub speech: Vec<(f64, f64)>,
    /// SFX alias → file.
    pub sfx_files: std::collections::BTreeMap<String, PathBuf>,
    /// Pre-rendered overlay files for HTML layers (layer index → mezzanine).
    pub html_renders: std::collections::BTreeMap<usize, PathBuf>,
    /// LUT dir for bundled LUTs.
    pub luts_dir: Option<PathBuf>,
    /// Laid-out text/caption boxes (for QA safe-zone and hook checks).
    pub text_boxes: Vec<TextBox>,
    /// SRT captions in timeline time.
    pub srt: String,
    /// Non-fatal findings from extras (e.g. W_REFRAME_NO_FACES).
    pub warnings: Vec<cutline_core::Diagnostic>,
    /// Brand logo drawn as a watermark.
    pub watermark: Option<WatermarkSpec>,
    /// Reframe crop paths per clip id (for the review pack).
    pub crop_paths: std::collections::BTreeMap<String, cutline_reframe::CropPath>,
}

/// The brand logo as a watermark (output seconds; width as a fraction of the frame width).
#[derive(Debug, Clone, Serialize)]
pub struct WatermarkSpec {
    pub path: PathBuf,
    pub position: String,
    pub opacity: f64,
    pub scale: f64,
    pub start: f64,
    pub end: f64,
}

/// Template image overlay (full-target pixel coordinates).
#[derive(Debug, Clone, Serialize)]
pub struct ExtraImage {
    pub path: PathBuf,
    pub start: f64,
    pub end: f64,
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
    pub pop: bool,
}

/// A laid-out text box on the timeline, in output pixels.
#[derive(Debug, Clone, Serialize)]
pub struct TextBox {
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

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn normalize_paths(s: &str, roots: &[&Path]) -> String {
    let mut out = s.to_string();
    for r in roots {
        out = out.replace(&escape::filter_path(r), "<root>").replace(&r.to_string_lossy().to_string(), "<root>");
    }
    out
}

/// Whether the deliverable is 4:4:4 (`chroma: "444"`; drafts and hardware encodes stay 4:2:0).
pub fn final_chroma444(tl: &Timeline, opts: &RenderOptions) -> bool {
    tl.target.chroma444 && !opts.draft && !opts.hw
}

/// Encoder args for the final deliverable.
pub fn encoder_args(tl: &Timeline, g: &Geometry, opts: &RenderOptions, tc: &Toolchain) -> Vec<String> {
    // Keyframe interval. Every keyframe re-encodes the picture from scratch, so on a still
    // image it lands on slightly different pixel values than the frames before it: a short
    // interval shows as a faint pulse on static shots (most visible on text and flat UI). Keep
    // keyframes rare (10 s for screen recordings, 2 s otherwise) and let the encoder add one
    // at each real scene change, where a pulse cannot be seen and seeking wants one anyway.
    let gop_secs = if tl.target.screencast { 10.0 } else { 2.0 };
    let gop = ((g.fps.as_f64() * gop_secs).round() as i64).max(1).to_string();
    let mut v: Vec<String> = vec![];
    let hw = if opts.draft || opts.hw { draft_hw_encoder(tc) } else { None };
    let uses_hw = hw.is_some();
    match hw {
        Some(enc) => {
            v.extend([String::from("-c:v"), enc.clone()]);
            if enc == "h264_videotoolbox" {
                v.extend(["-b:v", "6M", "-profile:v", "high"].map(String::from));
            } else {
                v.extend(["-b:v", "6M"].map(String::from));
            }
        }
        None => {
            let (preset, crf) = if opts.draft {
                ("veryfast", 23)
            } else if tl.target.screencast {
                // text and UI edges need more bits than camera footage
                ("slow", tl.target.crf.saturating_sub(2))
            } else {
                ("slow", tl.target.crf)
            };
            let level = if g.width * g.height > 1920 * 1088 { "5.1" } else { "4.2" };
            let profile = if final_chroma444(tl, opts) { "high444" } else { "high" };
            if tl.target.screencast && !opts.draft {
                // flat areas and hard edges: stronger deblocking, more reference frames
                v.extend(["-tune", "animation"].map(String::from));
            }
            v.extend(
                [
                    "-c:v",
                    "libx264",
                    "-preset",
                    preset,
                    "-crf",
                    &crf.to_string(),
                    "-profile:v",
                    profile,
                    "-level:v",
                    level,
                    "-bf",
                    "2",
                    "-g",
                    &gop,
                ]
                .map(String::from),
            );
            v.extend(["-x264-params", "scenecut=40"].map(String::from));
        }
    }
    let pix = if !uses_hw && final_chroma444(tl, opts) { "yuv444p" } else { "yuv420p" };
    v.extend(["-pix_fmt", pix, "-r", &g.fps.ff(), "-fps_mode", "cfr"].map(String::from));
    v.extend(
        ["-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-color_range", "tv"]
            .map(String::from),
    );
    v.extend(audio_plan::AUDIO_ENCODE_ARGS.map(String::from));
    v.extend(["-movflags", "+faststart", "-map_metadata", "-1"].map(String::from));
    v
}

fn draft_hw_encoder(tc: &Toolchain) -> Option<String> {
    if std::env::var("CUTLINE_NO_HW").is_ok() {
        return None;
    }
    let caps = cutline_deps::tools::capabilities(tc).ok()?;
    let order: &[&str] =
        if cfg!(target_os = "macos") { &["h264_videotoolbox"] } else { &["h264_nvenc", "h264_qsv", "h264_amf"] };
    for e in order {
        if caps.encoders.contains(*e) && hw_encoder_works(tc, e) {
            return Some(e.to_string());
        }
    }
    None
}

fn hw_encoder_works(tc: &Toolchain, enc: &str) -> bool {
    std::process::Command::new(&tc.ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=256x256:d=0.1",
            "-c:v",
            enc,
            "-f",
            "null",
            "-",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn final_job(
    tc: &Toolchain,
    tl: &Timeline,
    g: &Geometry,
    full: &Geometry,
    mezz: &[PathBuf],
    wav: &Path,
    work: &Path,
    output: &Path,
    opts: &RenderOptions,
    extras: &PlanExtras,
    p: &Project,
    media: &ProjectMedia,
) -> Result<(Vec<Job>, Job)> {
    let mut j = Job::new("final encode");
    j.args(base_args(false));
    for m in mezz {
        j.args(["-i".to_string(), m.to_string_lossy().to_string()]);
    }
    let (mut lines, mut cur) = assemble::join_video(tl, g);
    let mut next_input = mezz.len();
    let ov = overlay::apply(tl, g, full, &mut lines, &mut cur, &mut next_input, extras, p, media, work)?;
    j.args(ov.input_args.clone());
    j.args(["-i".to_string(), wav.to_string_lossy().to_string()]);
    let audio_idx = next_input;
    // stamp color properties on the frames: newer ffmpeg lets (unspecified) frame properties
    // override the -color_* output options after overlays
    lines.push(format!(
        "[{cur}]format={},setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv[vout]",
        if final_chroma444(tl, opts) { "yuv444p" } else { "yuv420p" }
    ));
    let script = work.join("final.filter");
    let text = lines.join(";\n") + "\n";
    j.args(tc.filter_complex_file_args(&script));
    j.args(["-map".to_string(), "[vout]".to_string(), "-map".to_string(), format!("{audio_idx}:a")]);
    j.args(encoder_args(tl, g, opts, tc));
    j.args(["-frames:v".to_string(), tl.total_frames.to_string()]);
    j.args(["-t".to_string(), escape::f(tl.duration())]);
    j.arg(output.to_string_lossy().to_string());
    j.scripts.push((script, text));
    j.scripts.extend(ov.scripts);
    j.files.extend(ov.files);
    if !extras.ass_docs.is_empty() {
        if let Some(fd) = &extras.fonts_dir {
            let conf = work.join("fonts.conf");
            j.scripts.push((conf.clone(), fontconfig_isolated(fd, &work.join("fccache"))));
            j.env.push(("FONTCONFIG_FILE".into(), conf.to_string_lossy().to_string()));
        }
    }
    j.outputs.push(output.to_path_buf());
    Ok((ov.pre_jobs, j))
}

/// A fontconfig config that only knows the bundled fonts, so text renders identically whether
/// or not system fonts exist (and never falls back to a system font).
pub fn fontconfig_isolated(fonts_dir: &Path, cache: &Path) -> String {
    format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"fonts.dtd\">\n<fontconfig>\n  <dir>{}</dir>\n  <cachedir>{}</cachedir>\n</fontconfig>\n",
        xml_escape(&fonts_dir.to_string_lossy()),
        xml_escape(&cache.to_string_lossy())
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct StageTiming {
    pub stage: String,
    pub seconds: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ExecReport {
    pub output: PathBuf,
    pub segments_total: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub rendered_segments: Vec<String>,
    pub timings: Vec<StageTiming>,
    pub loudness: Option<audio_plan::LoudnessResult>,
}

/// Execute a plan. Segment mezzanines are rendered in parallel (`opts.jobs`).
pub fn execute(tc: &Toolchain, plan: &Plan, opts: &RenderOptions) -> Result<ExecReport> {
    let mut rep = ExecReport { output: plan.output.clone(), segments_total: plan.segments.len(), ..Default::default() };
    std::fs::create_dir_all(&plan.work_dir)?;
    let t = Instant::now();
    let mut seen = std::collections::BTreeSet::new();
    // identical segments share a cache key: render each key once
    let todo: Vec<&PlannedSegment> = plan
        .segments
        .iter()
        .filter(|s| (opts.no_cache || !s.mezzanine.exists()) && seen.insert(s.key.clone()))
        .collect();
    let pending = plan.segments.iter().filter(|s| opts.no_cache || !s.mezzanine.exists()).count();
    rep.cache_hits = plan.segments.len() - pending;
    rep.cache_misses = pending;
    rep.rendered_segments = todo.iter().map(|s| s.clip_id.clone()).collect();
    let jobs = if opts.jobs == 0 {
        (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2) / 2).max(1)
    } else {
        opts.jobs
    };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;
    let res: Vec<Result<()>> = pool.install(|| {
        todo.par_iter()
            .map(|s| {
                s.job.run(tc)?;
                let tmp = &s.job.outputs[0];
                std::fs::rename(tmp, &s.mezzanine)?;
                Ok(())
            })
            .collect()
    });
    for r in res {
        r?;
    }
    rep.timings.push(StageTiming { stage: "segments".into(), seconds: t.elapsed().as_secs_f64() });

    let t = Instant::now();
    for j in &plan.pre_jobs {
        j.run(tc)?;
    }
    rep.timings.push(StageTiming { stage: "overlays".into(), seconds: t.elapsed().as_secs_f64() });

    let t = Instant::now();
    rep.loudness = audio_plan::execute(tc, &plan.audio)?;
    rep.timings.push(StageTiming { stage: "audio".into(), seconds: t.elapsed().as_secs_f64() });

    let t = Instant::now();
    plan.final_job.run(tc)?;
    rep.timings.push(StageTiming { stage: "encode".into(), seconds: t.elapsed().as_secs_f64() });
    Ok(rep)
}

/// Text for `render --dry-run`: every command and filter script, in execution order.
pub fn dry_run_text(tc: &Toolchain, plan: &Plan) -> String {
    let ff = tc.ffmpeg.to_string_lossy().to_string();
    let mut s = String::new();
    let mut section = |title: &str, j: &Job| {
        s.push_str(&format!("# {title}\n{}\n", j.display(&ff)));
        for (p, c) in &j.scripts {
            s.push_str(&format!("## {}\n{}\n", p.display(), c.trim_end()));
        }
        s.push('\n');
    };
    for seg in &plan.segments {
        section(&format!("segment {} → {}", seg.clip_id, seg.mezzanine.display()), &seg.job);
    }
    for j in &plan.pre_jobs {
        section(&j.label.clone(), j);
    }
    for j in audio_plan::jobs_for_display(&plan.audio) {
        section(&j.label.clone(), &j);
    }
    section("final encode", &plan.final_job);
    s
}

/// Map target name to its string for file names.
pub fn target_label(t: TargetName) -> &'static str {
    target_str(t)
}

/// Face detector availability (`yunet`, `model missing`, `not compiled …`).
pub fn reframe_detector_status() -> &'static str {
    cutline_reframe::detector_status()
}
