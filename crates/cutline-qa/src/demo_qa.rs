//! Review checks for demos and screen recordings:
//!
//! * **private details**: text recognized in the recordings that looks like an email address,
//!   a home path, a key or an IP address, and that no mask covers;
//! * **legibility**: interface text that ends up too small to read in the delivered frame;
//! * **pacing**: stretches where nothing is said and nothing is pointed at.
//!
//! Text is read from the source recordings at their own resolution (not from the preview), and
//! placed in the output through the clip's frame and zoom.

use crate::ocr::{self, Engine, OcrLine};
use anyhow::Result;
use cutline_core::model::{AssetKind, Layer, Rect};
use cutline_core::time::format_tc;
use cutline_core::timeline::{TClip, Timeline};
use cutline_media::cache::Cache;
use cutline_render::pipeline::{Loaded, Prepared};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DemoFinding {
    /// `private`, `legibility` or `pacing`.
    pub kind: String,
    /// Output seconds.
    pub at: f64,
    pub clip: Option<String>,
    pub message: String,
    /// A layer that would fix it, ready to paste into `layers`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DemoReport {
    /// Engine used for text recognition, or why none was.
    pub ocr: String,
    pub frames_read: usize,
    pub findings: Vec<DemoFinding>,
}

/// Stretches of the edit (output seconds) longer than `max` where nobody speaks and no text,
/// card or annotation is on screen.
pub fn quiet_gaps(pr: &Prepared, max: f64) -> Vec<(f64, f64)> {
    let tl = &pr.timeline;
    let mut busy: Vec<(f64, f64)> = pr.extras.speech.clone();
    for l in &tl.layers {
        if l.layer.is_annotation() || matches!(l.layer, Layer::Text { .. }) {
            busy.push((tl.secs(l.start), tl.secs(l.end)));
        }
    }
    busy.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (start, end) = (tl.secs(tl.origin), tl.secs(tl.body_end));
    let mut out = vec![];
    let mut cur = start;
    for (a, b) in busy {
        if a - cur > max {
            out.push((cur, a));
        }
        cur = cur.max(b);
    }
    if end - cur > max {
        out.push((cur, end));
    }
    out
}

/// Scale from source pixels to output pixels for a clip (without its zoom).
fn base_scale(c: &TClip, tl: &Timeline, src: (f64, f64)) -> f64 {
    let (w, h) = (tl.target.width as f64, tl.target.height as f64);
    match &c.content {
        Some(ct) => ct.h * h / src.1,
        // full-bleed: cover scale (the usual geometry)
        None => (w / src.0).max(h / src.1),
    }
}

fn overlap(a: &Rect, b: &Rect) -> f64 {
    let x = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let y = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if x <= 0.0 || y <= 0.0 {
        0.0
    } else {
        x * y / (a.w * a.h).max(1e-9)
    }
}

/// Whether a mask hides `line` (a box in fractions of the clip's picture) at timeline frame `fr`.
fn masked(tl: &Timeline, c: &TClip, line: &Rect, fr: i64) -> bool {
    let placed = match &c.content {
        Some(ct) => Rect { x: ct.x + line.x * ct.w, y: ct.y + line.y * ct.h, w: line.w * ct.w, h: line.h * ct.h },
        None => *line,
    };
    tl.layers.iter().filter(|l| fr >= l.start && fr < l.end).any(|l| match &l.layer {
        // a mask that follows the clip is written in the clip's own fractions
        Layer::Mask { rect, follow: Some(true), .. } => overlap(line, rect) >= 0.6,
        Layer::Mask { rect, .. } => overlap(&placed, rect) >= 0.6,
        _ => false,
    })
}

/// Source times to read in a clip: every few seconds, and just after each recorded step.
fn sample_times(l: &Loaded, c: &TClip, max: usize) -> Vec<f64> {
    let len = c.src_out - c.src_in;
    let n = ((len / 4.0).ceil() as usize).clamp(1, max);
    let mut v: Vec<f64> = (0..n).map(|i| c.src_in + (i as f64 + 0.5) * len / n as f64).collect();
    if let Some(ev) = l.demos.get(&c.asset) {
        // typed text is on screen once the step is done
        v.extend(ev.steps.iter().map(|s| s.t1 + 0.3).filter(|t| *t > c.src_in && *t < c.src_out));
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v.dedup_by(|a, b| (*a - *b).abs() < 0.75);
    v.truncate(max + 4);
    v
}

fn read_frame(l: &Loaded, engine: Engine, c: &TClip, t: f64, work: &std::path::Path) -> Result<Vec<OcrLine>> {
    let m = l.media.get(&c.asset).ok_or_else(|| anyhow::anyhow!("no media for {}", c.asset))?;
    let cache = Cache::new(cutline_render::cutline_dir(&l.dir).join("cache").join("ocr"));
    let key =
        cutline_media::hash_key(&[m.fingerprint.as_bytes(), format!("{t:.2} {}", engine.name()).as_bytes(), b"ocr-v1"]);
    if let Some(v) = cache.get_json::<Vec<OcrLine>>(&key) {
        return Ok(v);
    }
    let tc = cutline_deps::toolchain()?;
    std::fs::create_dir_all(work)?;
    let png = work.join(format!("{key}.png"));
    let o = tc
        .ffmpeg_cmd()
        .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-ss", &format!("{t:.3}"), "-i"])
        .arg(&m.path)
        .args(["-frames:v", "1"])
        .arg(&png)
        .output()?;
    anyhow::ensure!(o.status.success() && png.is_file(), "frame grab failed: {}", String::from_utf8_lossy(&o.stderr));
    let lines = ocr::recognize(engine, &png)?;
    let _ = std::fs::remove_file(&png);
    cache.put_json(&key, &lines)?;
    Ok(lines)
}

/// Run the demo checks. `ocr_choice` is the `review --ocr` value.
pub fn run(l: &Loaded, pr: &Prepared, ocr_choice: Option<&str>) -> Result<DemoReport> {
    let tl = &pr.timeline;
    let qa = l.project.qa.clone().unwrap_or_default();
    let mut rep = DemoReport::default();
    let recordings: Vec<&TClip> = tl
        .clips
        .iter()
        .filter(|c| c.kind == AssetKind::Video && !c.clip.is_bumper())
        .filter(|c| tl.target.screencast || c.frame.is_some() || l.demos.contains_key(&c.asset))
        .collect();
    if recordings.is_empty() {
        rep.ocr = "no screen recordings in this edit".into();
        return Ok(rep);
    }

    // ---- pacing
    for (a, b) in quiet_gaps(pr, qa.max_quiet.unwrap_or(8.0)) {
        rep.findings.push(DemoFinding {
            kind: "pacing".into(),
            at: a,
            clip: tl.clip_at(tl.fps.secs_to_frames(a)).map(|c| c.id.clone()),
            message: format!(
                "Nothing is said or pointed at for {:.1}s ({}–{}): narrate it, annotate it, cut it, or let `auto.fast_forward_idle` speed it up.",
                b - a,
                format_tc(a),
                format_tc(b)
            ),
            fix: None,
        });
    }

    // ---- text recognition
    let engine = match ocr::setting(ocr_choice, qa.ocr).map_err(|e| e.to_string()).and_then(ocr::resolve) {
        Ok(e) => e,
        Err(why) => {
            rep.ocr = format!("not run: {why}");
            return Ok(rep);
        }
    };
    rep.ocr = engine.name().to_string();
    let (w, h) = (tl.target.width as f64, tl.target.height as f64);
    let min_px = qa.min_text_px.unwrap_or(if h > w { 0.024 } else { 0.020 } * w.min(h));
    let work = cutline_render::cutline_dir(&l.dir).join("tmp").join(format!("ocr-{}", std::process::id()));
    let mut budget = 28usize;
    for c in recordings {
        let Some(m) = l.media.get(&c.asset) else { continue };
        let src = (m.facts.width.max(1) as f64, m.facts.height.max(1) as f64);
        let scale = base_scale(c, tl, src);
        let mut seen: Vec<String> = vec![];
        let mut worst: Option<(f64, f64, usize, usize)> = None; // (share small, median px, n, at frame)
        for t in sample_times(l, c, 6) {
            if budget == 0 {
                break;
            }
            budget -= 1;
            let lines = match read_frame(l, engine, c, t, &work) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("text recognition failed on `{}` at {t:.1}s: {e:#}", c.id);
                    continue;
                }
            };
            rep.frames_read += 1;
            let fr = c.start + tl.fps.secs_to_frames((t - c.src_in) / c.speed);
            let at = tl.secs(fr);
            // private details no mask covers
            for ln in &lines {
                let Some((kind, what)) = ocr::private_kind(&ln.text) else { continue };
                let rect = Rect { x: ln.x, y: ln.y, w: ln.w, h: ln.h };
                if seen.contains(&what) || masked(tl, c, &rect, fr) {
                    continue;
                }
                seen.push(what.clone());
                // a little air around the line
                let pad = 0.004;
                let r = |v: f64| (v * 1e4).round() / 1e4;
                let fix = json!({
                    "type": "mask", "mode": "fill", "follow": true, "anchor": c.id,
                    "rect": {"x": r((ln.x - pad).max(0.0)), "y": r((ln.y - pad).max(0.0)),
                             "w": r((ln.w + 2.0 * pad).min(1.0 - (ln.x - pad).max(0.0))), "h": r((ln.h + 2.0 * pad).min(1.0 - (ln.y - pad).max(0.0)))},
                });
                rep.findings.push(DemoFinding {
                    kind: "private".into(),
                    at,
                    clip: Some(c.id.clone()),
                    message: format!(
                        "Possible {kind} on screen at {} in clip `{}`: \"{what}\". Cover it with a mask (set `color` to the background and `text` to a stand-in), or record with demo data.",
                        format_tc(at),
                        c.id
                    ),
                    fix: Some(fix),
                });
            }
            // how tall the text is in the delivered frame, zoom included
            let dur = tl.secs(c.frames);
            let (zoom, ..) = cutline_render::effects::zoom_at(c, dur, tl.secs(fr - c.start));
            let mut px: Vec<f64> = lines
                .iter()
                .filter(|ln| ln.conf >= 0.4 && ln.text.chars().filter(|ch| ch.is_alphanumeric()).count() >= 3)
                .map(|ln| ln.h * src.1 * scale * zoom)
                .collect();
            if px.len() >= 4 {
                px.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let small = px.iter().filter(|v| **v < min_px).count() as f64 / px.len() as f64;
                let med = px[px.len() / 2];
                if worst.map_or(true, |w| small > w.0) {
                    worst = Some((small, med, px.len(), fr as usize));
                }
            }
        }
        if let Some((small, med, n, fr)) = worst.filter(|w| w.0 > 0.5) {
            let at = tl.secs(fr as i64);
            rep.findings.push(DemoFinding {
                kind: "legibility".into(),
                at,
                clip: Some(c.id.clone()),
                message: format!(
                    "Interface text in clip `{}` is small in the {}×{} output: {:.0} % of {n} lines at {} are under {min_px:.0} px tall (median {med:.0} px). Zoom to what the narration is about (`zoom_to`, `auto.zoom_on_clicks`), record at a smaller viewport, or use a larger frame.",
                    c.id,
                    tl.target.width,
                    tl.target.height,
                    small * 100.0,
                    format_tc(at)
                ),
                fix: None,
            });
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    rep.findings.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap());
    Ok(rep)
}
