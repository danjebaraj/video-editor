//! Render one or more exports, run QA and write the side files.

use crate::qa::{run_qa, QaContext, QaReport};
use anyhow::Result;
use cutline_core::model::{Effect, Project};
use cutline_core::timeline::Timeline;
use cutline_render::pipeline::{self, Loaded, Prepared};
use cutline_render::{ExecReport, Plan, RenderOptions};
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct ExportResult {
    pub export: Option<String>,
    pub output: PathBuf,
    pub qa_path: PathBuf,
    pub report_path: PathBuf,
    pub srt_path: PathBuf,
    pub credits_path: PathBuf,
    pub qa: QaReport,
    pub exec: ExecReport,
    pub duration: f64,
    /// Non-fatal plan findings (e.g. `W_REFRAME_NO_FACES`, `W_CAPTIONS_NO_TRANSCRIPT`).
    pub diagnostics: Vec<cutline_core::Diagnostic>,
}

/// QA expectations derived from the edit.
pub fn qa_context(p: &Project, tl: &Timeline, extras: &cutline_render::PlanExtras) -> QaContext {
    let mut c = QaContext {
        expected_duration: Some(tl.duration()),
        is_shorts: tl.target.is_vertical(),
        max_cps: 20.0,
        screencast: tl.target.screencast,
        chroma444: tl.target.chroma444,
        ..Default::default()
    };
    let lt = p.audio.as_ref().and_then(|a| a.loudness.clone()).unwrap_or_default();
    c.loudness_target = Some((lt.integrated, lt.true_peak));
    for t in &tl.transitions {
        let (s, e) = (tl.secs(t.start), tl.secs(t.start + t.frames));
        if t.transition.kind.contains("black") || t.transition.kind == "fade" || t.transition.kind == "dissolve" {
            c.intended_black.push((s, e));
        }
        c.intended_silence.push((s, e));
    }
    for cl in &tl.clips {
        for e in &cl.clip.effects {
            if let Effect::Freeze { at, duration } = e {
                let s = tl.secs(cl.start) + at;
                c.intended_freeze.push((s - 0.1, s + duration + 0.1));
            }
        }
        if cl.kind == cutline_core::model::AssetKind::Image {
            c.intended_freeze.push((tl.secs(cl.start), tl.secs(cl.end())));
        }
    }
    c.caption_boxes = extras_caption_boxes(extras);
    c.first_text_at = extras_first_text(extras);
    c
}

fn extras_caption_boxes(e: &cutline_render::PlanExtras) -> Vec<crate::qa::CaptionBox> {
    e.text_boxes
        .iter()
        .map(|b| crate::qa::CaptionBox {
            start: b.start,
            end: b.end,
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
            text: b.text.clone(),
            kind: b.kind.clone(),
        })
        .collect()
}

fn extras_first_text(e: &cutline_render::PlanExtras) -> Option<f64> {
    e.text_boxes.iter().map(|b| b.start).fold(None, |a: Option<f64>, s| Some(a.map_or(s, |a| a.min(s))))
}

fn side_path(out: &Path, ext: &str) -> PathBuf {
    let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    out.with_file_name(format!("{stem}.{ext}"))
}

/// Render all (or one named) exports with QA.
pub fn render_all(l: &Loaded, opts: &RenderOptions, only: Option<&str>) -> Result<Vec<ExportResult>> {
    let tc = cutline_deps::toolchain()?;
    let mut results = vec![];
    for (name, e) in pipeline::exports(&l.project) {
        if let Some(o) = only {
            let n = pipeline::export_name(&e);
            if n != o && name.as_deref() != Some(o) {
                continue;
            }
        }
        let pr = pipeline::prepare(l, name.clone(), &e, opts.draft)?;
        let mut o2 = opts.clone();
        if let Some(out) = &e.out {
            o2.out = Some(l.dir.join(out));
        }
        let (plan, exec) = pipeline::render(l, &pr, &o2)?;
        results.push(finish(tc, l, &pr, &plan, exec)?);
        // temp hygiene: remove the run's tmp dir on success
        let _ = std::fs::remove_dir_all(&plan.work_dir);
    }
    if results.is_empty() {
        anyhow::bail!("no export named {:?}", only);
    }
    Ok(results)
}

fn finish(
    tc: &cutline_deps::Toolchain,
    l: &Loaded,
    pr: &Prepared,
    plan: &Plan,
    exec: ExecReport,
) -> Result<ExportResult> {
    let t = std::time::Instant::now();
    let ctx = qa_context(&pr.project, &pr.timeline, &pr.extras);
    let target = if plan.draft {
        let mut t = pr.timeline.target.clone();
        t.width = plan.geometry.width;
        t.height = plan.geometry.height;
        t
    } else {
        pr.timeline.target.clone()
    };
    let qa = run_qa(tc, &plan.output, &target, &ctx)?;
    let qa_secs = t.elapsed().as_secs_f64();
    let qa_path = side_path(&plan.output, "qa.json");
    std::fs::write(&qa_path, serde_json::to_vec_pretty(&qa)?)?;
    let srt_path = side_path(&plan.output, "srt");
    std::fs::write(&srt_path, &pr.extras.srt)?;
    let credits_path = plan.output.with_file_name("credits.txt");
    std::fs::write(&credits_path, credits(&pr.project, l))?;
    let report_path = side_path(&plan.output, "render_report.json");
    let mut timings = exec.timings.clone();
    timings.push(cutline_render::StageTiming { stage: "qa".into(), seconds: qa_secs });
    let report = json!({
        "project": l.path,
        "export": pr.export_name,
        "output": plan.output,
        "target": plan.target,
        "geometry": plan.geometry,
        "duration": plan.duration,
        "frames": plan.frames,
        "draft": plan.draft,
        "timings": timings,
        "cache": {"segments": exec.segments_total, "hits": exec.cache_hits, "misses": exec.cache_misses, "rendered": exec.rendered_segments},
        "loudness": exec.loudness,
        "tools": {"cutline": cutline_core::VERSION, "ffmpeg": tc.version, "ffmpeg_path": tc.ffmpeg},
        "qa": {"ok": qa.ok, "errors": qa.errors, "warnings": qa.warnings, "path": qa_path},
        "diagnostics": pr.diags,
    });
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    Ok(ExportResult {
        export: pr.export_name.clone(),
        output: plan.output.clone(),
        qa_path,
        report_path,
        srt_path,
        credits_path,
        qa,
        exec,
        duration: plan.duration,
        diagnostics: pr.diags.clone(),
    })
}

/// `credits.txt`: every asset used, for attribution and audits.
pub fn credits(p: &Project, l: &Loaded) -> String {
    let mut s = String::from("# Assets used in this render\n");
    for (id, a) in &p.assets {
        let role = a.role.map(|r| format!("{r:?}").to_lowercase()).unwrap_or_else(|| "-".into());
        let fp = l.media.get(id).map(|m| m.fingerprint.clone()).unwrap_or_default();
        s.push_str(&format!("{id}\t{}\trole={role}\tsynthetic={}\tfingerprint={fp}\n", a.path, a.synthetic));
    }
    s
}

pub const PUBLISHING_CHECKLIST: &str = "Publishing checklist:
  [ ] YouTube altered/synthetic content disclosure ticked if realistic AI-generated people, voices or scenes are present (AI voice-over may qualify)
  [ ] Music and SFX rights confirmed (ElevenLabs plan covers monetized publishing / licenses on file)
  [ ] Chapters pasted into the description (long form) — `cutline chapters <project>`
  [ ] Thumbnail exported — `cutline thumbnail <project> --at <t> --text \"...\"`
  [ ] Captions uploaded as .srt (written next to the video)";
