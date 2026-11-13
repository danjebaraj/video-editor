//! Review pack (§7.3): everything Claude needs to *see* the edit, sized for vision input
//! (longest side ≤ 1568 px, ≤ 20 images).

use crate::draw::{fit, hstack, save_jpg, shade_outside, stroke_rect, Painter};
use crate::qa::{run_qa, QaReport};
use anyhow::{Context, Result};
use cutline_core::time::format_tc;
use cutline_core::timeline::Timeline;
use cutline_render::pipeline::{self, Loaded, Prepared};
use cutline_render::RenderOptions;
use image::{Rgb, RgbImage};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const MAX_SIDE: u32 = 1568;
pub const MAX_IMAGES: usize = 20;

#[derive(Debug, Clone, Serialize)]
pub struct ReviewReport {
    pub dir: PathBuf,
    pub source_video: PathBuf,
    pub preview: PathBuf,
    pub images: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    pub suggestions: Vec<String>,
    pub qa_ok: bool,
}

/// Grab one frame (seconds) from any media as RGB.
pub fn grab(tc: &cutline_deps::Toolchain, path: &Path, t: f64) -> Result<RgbImage> {
    let o = std::process::Command::new(&tc.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-ss", &format!("{:.6}", t.max(0.0)), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", cutline_testkit::analyze::CONV, "-f", "image2pipe", "-c:v", "png", "-"])
        .output()?;
    anyhow::ensure!(
        o.status.success() && !o.stdout.is_empty(),
        "frame grab at {t:.3}s failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    Ok(image::load_from_memory(&o.stdout)?.to_rgb8())
}

fn clip_label(tl: &Timeline, t: f64) -> String {
    tl.clip_at(tl.fps.secs_to_frames(t)).map(|c| c.id.clone()).unwrap_or_default()
}

/// EDL-style timeline text (clips, transitions, layers, captions).
pub fn timeline_text(pr: &Prepared) -> String {
    let tl = &pr.timeline;
    let mut s = format!("TIMELINE {} fps, {} frames, {}\n\nCLIPS\n", tl.fps, tl.total_frames, format_tc(tl.duration()));
    s.push_str(&format!(
        "{:<20} {:<10} {:<10} {:<10} {:<10} {:<10} {:<6} {}\n",
        "id", "asset", "tl_in", "tl_out", "src_in", "src_out", "speed", "effects / notes"
    ));
    for c in &tl.clips {
        let fx: Vec<String> = c
            .clip
            .effects
            .iter()
            .map(|e| {
                serde_json::to_value(e).ok().and_then(|v| v["type"].as_str().map(str::to_string)).unwrap_or_default()
            })
            .collect();
        let mut notes = fx.join(",");
        if let Some(t) = tl.transitions.iter().find(|t| t.after == c.index) {
            notes.push_str(&format!(" → {} {:.2}s", t.transition.kind, tl.secs(t.frames)));
        }
        if let Some(g) = &c.clip.generated_by {
            notes.push_str(&format!(" [{g}]"));
        }
        s.push_str(&format!(
            "{:<20} {:<10} {:<10} {:<10} {:<10} {:<10} {:<6.2} {}\n",
            c.id,
            c.asset,
            format_tc(tl.secs(c.start)),
            format_tc(tl.secs(c.end())),
            format_tc(c.src_in),
            format_tc(c.src_out),
            c.speed,
            notes.trim()
        ));
    }
    s.push_str("\nLAYERS\n");
    for l in &tl.layers {
        let detail = match &l.layer {
            cutline_core::model::Layer::Text { template, text, .. } => {
                format!("{template}: {}", text.clone().unwrap_or_default())
            }
            other => other.asset().unwrap_or("").to_string(),
        };
        s.push_str(&format!(
            "  [{}] {:<12} {} – {}  z={}  {}\n",
            l.index,
            l.layer.kind(),
            format_tc(tl.secs(l.start)),
            format_tc(tl.secs(l.end)),
            l.z,
            detail
        ));
    }
    let caps: Vec<_> = pr.extras.text_boxes.iter().filter(|b| b.kind == "caption").collect();
    if !caps.is_empty() {
        s.push_str("\nCAPTIONS\n");
        for b in caps {
            s.push_str(&format!("  {} – {}  {}\n", format_tc(b.start), format_tc(b.end), b.text));
        }
    }
    s
}

/// Seconds-intervals with no visual change (cuts, transitions, motion effects, layer starts).
pub fn static_gaps(pr: &Prepared, min: f64) -> Vec<(f64, f64)> {
    let tl = &pr.timeline;
    let mut ev: Vec<f64> = vec![0.0, tl.duration()];
    for c in &tl.clips {
        ev.push(tl.secs(c.start));
        for e in &c.clip.effects {
            if let Some(at) = serde_json::to_value(e).ok().and_then(|v| v["at"].as_f64()) {
                ev.push(tl.secs(c.start) + at);
            }
            if matches!(e, cutline_core::model::Effect::KenBurns { .. } | cutline_core::model::Effect::Zoom { .. }) {
                // continuous motion: count as change every second
                let mut t = tl.secs(c.start);
                while t < tl.secs(c.end()) {
                    ev.push(t);
                    t += 1.0;
                }
            }
        }
    }
    for l in &tl.layers {
        if !matches!(
            l.layer,
            cutline_core::model::Layer::Audio { .. }
                | cutline_core::model::Layer::Sfx { .. }
                | cutline_core::model::Layer::ProgressBar { .. }
        ) {
            ev.push(tl.secs(l.start));
            ev.push(tl.secs(l.end));
        }
    }
    for b in &pr.extras.text_boxes {
        if b.kind == "text" {
            ev.push(b.start);
        }
    }
    ev.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ev.windows(2).filter(|w| w[1] - w[0] > min).map(|w| (w[0], w[1])).collect()
}

pub fn review(l: &Loaded, out_dir: &Path) -> Result<ReviewReport> {
    let tc = cutline_deps::toolchain()?;
    std::fs::create_dir_all(out_dir)?;
    for sub in ["cuts", "captions", "reframe"] {
        let d = out_dir.join(sub);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d)?;
    }
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(l, name.clone(), &e, true)?;
    let preview = out_dir.join("preview_540p.mp4");
    let opts = RenderOptions { draft: true, out: Some(preview.clone()), ..Default::default() };
    let (plan, _exec) = pipeline::render(l, &pr, &opts)?;
    // prefer an up-to-date final render for stills and QA
    let final_path = l.dir.join("out").join(format!("{}.mp4", cutline_render::output_name(&l.project, &l.path)));
    let proj_mtime = std::fs::metadata(&l.path).and_then(|m| m.modified()).ok();
    let use_final = final_path.exists() && std::fs::metadata(&final_path).and_then(|m| m.modified()).ok() >= proj_mtime;
    let src = if use_final { final_path.clone() } else { preview.clone() };
    let tl = &pr.timeline;
    let dur = tl.duration();
    let painter = Painter::new();
    let mut images: Vec<PathBuf> = vec![];
    let mut files: Vec<PathBuf> = vec![];

    // ---- QA
    let ctx = crate::deliver::qa_context(&pr.project, tl, &pr.extras);
    let mut target = tl.target.clone();
    if !use_final {
        target.width = plan.geometry.width;
        target.height = plan.geometry.height;
    }
    let qa: QaReport = run_qa(tc, &src, &target, &ctx)?;
    let qa_path = out_dir.join("qa.json");
    std::fs::write(&qa_path, serde_json::to_vec_pretty(&qa)?)?;
    files.push(qa_path);

    // ---- contact sheet (≤ 24 tiles)
    let step = [0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0]
        .into_iter()
        .find(|s| dur / s <= 24.0)
        .unwrap_or(dur / 24.0);
    let times: Vec<f64> = (0..).map(|k| k as f64 * step + step / 2.0).take_while(|t| *t < dur).take(24).collect();
    let vertical = tl.target.is_vertical();
    let (tw, th, cols) = if vertical { (216u32, 384u32, 6u32) } else { (384, 216, 4) };
    let rows = (times.len() as u32).div_ceil(cols);
    let mut sheet = RgbImage::from_pixel(cols * tw, rows.max(1) * th, Rgb([16, 16, 20]));
    for (k, t) in times.iter().enumerate() {
        let f = grab(tc, &src, *t)?;
        let mut tile = image::imageops::resize(&f, tw, th, image::imageops::FilterType::Triangle);
        painter.label(&mut tile, 6, 6, 15.0, &format!("{}  {}", format_tc(*t), clip_label(tl, *t)), [255, 255, 255]);
        image::imageops::overlay(&mut sheet, &tile, ((k as u32 % cols) * tw) as i64, ((k as u32 / cols) * th) as i64);
    }
    let p = out_dir.join("contact_sheet.jpg");
    save_jpg(&fit(&sheet, MAX_SIDE), &p)?;
    images.push(p);

    // ---- cuts (≤ 8)
    let mut cut_pts: Vec<(String, f64, f64)> = vec![];
    for c in tl.clips.iter().skip(1) {
        let fr = 1.0 / tl.fps.as_f64();
        match tl.transitions.iter().find(|t| t.after + 1 == c.index) {
            Some(t) if t.frames > 0 => cut_pts.push((
                format!("{}→{} ({})", tl.clips[t.after].id, c.id, t.transition.kind),
                tl.secs(t.start) - fr,
                tl.secs(t.start + t.frames) + fr,
            )),
            _ => cut_pts.push((
                format!("{}→{}", tl.clips[c.index - 1].id, c.id),
                tl.secs(c.start) - fr,
                tl.secs(c.start),
            )),
        }
    }
    let cut_pts = pick_evenly(cut_pts, 8);
    for (k, (label, a, b)) in cut_pts.iter().enumerate() {
        let fa = grab(tc, &src, *a)?;
        let fb = grab(tc, &src, *b)?;
        let h = if vertical { 640 } else { 400 };
        let ra = image::imageops::resize(&fa, fa.width() * h / fa.height(), h, image::imageops::FilterType::Triangle);
        let rb = image::imageops::resize(&fb, fb.width() * h / fb.height(), h, image::imageops::FilterType::Triangle);
        let mut img = hstack(&ra, &rb, 8);
        painter.label(
            &mut img,
            8,
            8,
            18.0,
            &format!("{label}  {} | {}", format_tc(*a), format_tc(*b)),
            [255, 230, 120],
        );
        let p = out_dir.join("cuts").join(format!("cut_{:02}.jpg", k + 1));
        save_jpg(&fit(&img, MAX_SIDE), &p)?;
        images.push(p);
    }

    // ---- captions with safe zone (≤ 4)
    let scale = if use_final { 1.0 } else { plan.geometry.width as f64 / tl.target.width as f64 };
    let (sx, sy, sw, sh) = tl.target.safe_rect();
    let caps: Vec<_> = pr.extras.text_boxes.iter().filter(|b| b.kind == "caption").cloned().collect();
    let caps = pick_evenly(caps, 4);
    for (k, b) in caps.iter().enumerate() {
        let t = (b.start + b.end) / 2.0;
        let mut f = grab(tc, &src, t)?;
        let sc = |v: f64| (v * scale).round() as i32;
        shade_outside(&mut f, sc(sx), sc(sy), sc(sw), sc(sh));
        stroke_rect(&mut f, sc(sx), sc(sy), sc(sw), sc(sh), 2, [0, 255, 0]);
        stroke_rect(&mut f, sc(b.x), sc(b.y), sc(b.w), sc(b.h), 2, [255, 200, 0]);
        painter.label(&mut f, 8, 8, 16.0, &format!("{}  \"{}\"", format_tc(t), b.text), [255, 255, 255]);
        let p = out_dir.join("captions").join(format!("caption_{:02}.jpg", k + 1));
        save_jpg(&fit(&f, MAX_SIDE), &p)?;
        images.push(p);
    }

    // ---- reframe (≤ 4 images; 3 times each)
    let mut nre = 0;
    for c in &tl.clips {
        if nre >= 4 {
            break;
        }
        let Some(path) = pr.extras.crop_paths.get(&c.id) else { continue };
        let Some(m) = l.media.get(&c.asset) else { continue };
        let mut panels = vec![];
        for frac in [0.1, 0.5, 0.9] {
            let i = ((path.len() as f64 - 1.0) * frac) as usize;
            let src_t = c.src_in + (i as f64 / path.fps) * c.speed;
            let mut f = grab(tc, &m.path, src_t)?;
            let s = f.width() as f64 / path.src_w as f64;
            stroke_rect(
                &mut f,
                (path.x[i] as f64 * s) as i32,
                (path.y[i] as f64 * s) as i32,
                (path.cw[i] as f64 * s) as i32,
                (path.ch[i] as f64 * s) as i32,
                4,
                [0, 255, 0],
            );
            painter.label(
                &mut f,
                8,
                8,
                22.0,
                &format!("{} @ {}", c.id, format_tc(tl.secs(c.start) + i as f64 / path.fps)),
                [255, 255, 255],
            );
            panels.push(image::imageops::resize(
                &f,
                480,
                480 * f.height() / f.width(),
                image::imageops::FilterType::Triangle,
            ));
        }
        let img = hstack(&hstack(&panels[0], &panels[1], 6), &panels[2], 6);
        let p = out_dir.join("reframe").join(format!("reframe_{}.jpg", sanitize(&c.id)));
        save_jpg(&fit(&img, MAX_SIDE), &p)?;
        images.push(p);
        nre += 1;
    }

    // ---- audio.png
    let ap = out_dir.join("audio.png");
    cutline_render::audio_plan::audio_png(tc, &plan.audio, &preview, &ap).context("audio.png")?;
    images.push(ap);

    // ---- timeline + summary
    let tp = out_dir.join("timeline.txt");
    std::fs::write(&tp, timeline_text(&pr))?;
    files.push(tp);
    let suggestions = suggestions(&pr, &qa, &plan.audio, tc)?;
    let mut md = format!(
        "# Review: {}\n\nSource: `{}`  \nDuration: {}  \nTarget: {} ({}×{} @ {} fps)\n\n## QA: {} ({} errors, {} warnings)\n\n",
        cutline_render::output_name(&l.project, &l.path),
        src.display(),
        format_tc(dur),
        cutline_core::target::target_str(tl.target.name),
        tl.target.width,
        tl.target.height,
        tl.fps,
        if qa.ok { "PASS" } else { "FAIL" },
        qa.errors,
        qa.warnings
    );
    for f in &qa.findings {
        let mark = if f.ok {
            "✅"
        } else if f.severity == crate::qa::Sev::Error {
            "❌"
        } else {
            "⚠️"
        };
        md.push_str(&format!("- {mark} **{}**: {}\n", f.check, f.message));
    }
    md.push_str("\n## Suggested improvements\n\n");
    if suggestions.is_empty() {
        md.push_str("- none — looks good\n");
    }
    for s in &suggestions {
        md.push_str(&format!("- {s}\n"));
    }
    if !pr.diags.is_empty() {
        md.push_str("\n## Notes\n\n");
        for d in &pr.diags {
            md.push_str(&format!("- `{}` {}: {}\n", d.code, d.path, d.message));
        }
    }
    md.push_str("\n## Files\n\n- contact_sheet.jpg, cuts/, captions/, reframe/, audio.png, timeline.txt, qa.json, preview_540p.mp4\n");
    let sp = out_dir.join("summary.md");
    std::fs::write(&sp, md)?;
    files.push(sp);
    let _ = std::fs::remove_dir_all(&plan.work_dir);
    images.truncate(MAX_IMAGES);
    Ok(ReviewReport {
        dir: out_dir.to_path_buf(),
        source_video: src,
        preview,
        images,
        files,
        suggestions,
        qa_ok: qa.ok,
    })
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn pick_evenly<T: Clone>(v: Vec<T>, n: usize) -> Vec<T> {
    if v.len() <= n {
        return v;
    }
    (0..n).map(|k| v[k * (v.len() - 1) / (n - 1).max(1)].clone()).collect()
}

/// Human suggestions from QA + edit analysis.
pub fn suggestions(
    pr: &Prepared,
    qa: &QaReport,
    audio: &cutline_render::audio_plan::AudioPlan,
    tc: &cutline_deps::Toolchain,
) -> Result<Vec<String>> {
    let mut s = vec![];
    for (a, b) in static_gaps(pr, 5.0) {
        s.push(format!(
            "No visual change for {:.1}s ({}–{}): add a cut, punch-in, B-roll or a graphic.",
            b - a,
            format_tc(a),
            format_tc(b)
        ));
    }
    for f in qa.findings.iter().filter(|f| !f.ok) {
        match f.check.as_str() {
            "hook" => s.push("Hook missing: put a hook_title or caption in the first second.".into()),
            "captions_cps" => {
                s.push("Caption reading speed above 20 chars/s: lower max_words_per_line or slow the edit.".into())
            }
            "captions_safe_zone" => s.push("Captions/text leave the safe zone: move them or reduce size.".into()),
            "first_frame" => s.push("First frame is black/blank: start on a strong image.".into()),
            "black_frames" => s.push("Unintended black frames: check gaps between clips.".into()),
            "freeze" => s.push("Frozen picture detected: trim it or add motion.".into()),
            "silence" => s.push("Digital silence > 1.5s: add room tone/music or tighten.".into()),
            "loudness" => s.push(format!("Loudness off target: {}.", f.message)),
            "clipping" => s.push("Audio clipping detected: lower SFX/music gain.".into()),
            _ => {}
        }
    }
    // music vs voice level
    let voice = ["voice", "dialogue"].iter().filter_map(|r| audio.stems.get(*r)).find(|p| p.exists());
    if let (Some(v), Some(m)) = (voice, audio.stems.get("music").filter(|p| p.exists())) {
        if let (Ok(lv), Ok(lm)) =
            (cutline_media::probe::measure_loudness(tc, v), cutline_media::probe::measure_loudness(tc, m))
        {
            let duck = audio.duck.as_ref().filter(|d| !d.sidechain).map(|d| d.params.amount_db).unwrap_or(0.0);
            let diff = lm.integrated_lufs + duck - lv.integrated_lufs;
            if lv.integrated_lufs > -60.0 && diff > -18.0 {
                s.push(format!("Music is only {:.1} LU below the voice under ducking (aim for ≤ −18 LU): lower the music gain_db or increase duck.amount_db.", -diff));
            }
        }
    }
    if pr.timeline.target.is_vertical()
        && !pr.timeline.layers.iter().any(|l| matches!(l.layer, cutline_core::model::Layer::ProgressBar { .. }))
    {
        s.push("Shorts retention: consider a progress_bar layer.".into());
    }
    Ok(s)
}
