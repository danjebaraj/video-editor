//! Auto directives for demos: `fast_forward_idle` (any recording), and for browser recordings
//! `zoom_on_clicks` and `cursor`, which read the recording's events sidecar. Like the other
//! directives they only write explicit items into the project, shown by `cutline expand`.

use crate::auto::{effect_at, timeline_words, with_at};
use crate::pipeline::{context, Loaded};
use anyhow::Result;
use cutline_core::brand::Brand;
use cutline_core::diag::Diagnostic;
use cutline_core::model::*;
use cutline_core::time::{round6, TimeValue};
use cutline_core::timeline::{resolve, TClip};
use cutline_html::demo::{CursorPoint, DemoEvents};
use cutline_media::cache::Cache;
use std::collections::BTreeMap;

fn note(path: String, msg: String) -> Diagnostic {
    Diagnostic::warning("I_AUTO_EXPANDED", path, msg)
}

fn placement(anchor: &str, offset: f64, duration: f64) -> Placement {
    Placement {
        anchor: Some(anchor.to_string()),
        offset: Some(round6(offset)),
        duration: Some(TimeValue::Seconds(round6(duration))),
        ..Default::default()
    }
}

// ------------------------------------------------------------------ fast_forward_idle

/// Share of the picture that changes between samples, ten times a second, over a source span.
/// The first value is 1 (nothing to compare with). Cached by the asset's fingerprint.
pub fn activity(l: &Loaded, asset: &str, from: f64, to: f64) -> Result<Vec<f32>> {
    const W: usize = 160;
    const H: usize = 90;
    let m = l.media.get(asset).ok_or_else(|| anyhow::anyhow!("no media for {asset}"))?;
    let cache = Cache::new(crate::cutline_dir(&l.dir).join("cache").join("analysis"));
    let key =
        cutline_media::hash_key(&[m.fingerprint.as_bytes(), format!("{from:.4}-{to:.4}").as_bytes(), b"activity-v1"]);
    if let Some(v) = cache.get_json::<Vec<f32>>(&key) {
        return Ok(v);
    }
    let tc = cutline_deps::toolchain()?;
    let o = std::process::Command::new(&tc.ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-ss",
            &format!("{from:.6}"),
            "-t",
            &format!("{:.6}", to - from),
            "-i",
        ])
        .arg(&m.path)
        .args(["-an", "-vf", &format!("fps=10,scale={W}:{H}:flags=area,format=gray"), "-f", "rawvideo", "-"])
        .output()?;
    anyhow::ensure!(o.status.success(), "activity analysis failed: {}", String::from_utf8_lossy(&o.stderr));
    let frames: Vec<&[u8]> = o.stdout.chunks_exact(W * H).collect();
    let mut v = vec![1.0f32; frames.len().min(1)];
    for w in frames.windows(2) {
        let changed = w[0].iter().zip(w[1]).filter(|(a, b)| a.abs_diff(**b) > 12).count();
        v.push(changed as f32 / (W * H) as f32);
    }
    cache.put_json(&key, &v)?;
    Ok(v)
}

/// Runs of quiet samples at least `min_idle` long, as source seconds relative to `from`.
pub fn idle_runs(act: &[f32], threshold: f64, min_idle: f64) -> Vec<(f64, f64)> {
    let mut out = vec![];
    let mut start: Option<usize> = None;
    for (i, a) in act.iter().chain(std::iter::once(&1.0)).enumerate() {
        match (start, (*a as f64) < threshold) {
            (None, true) => start = Some(i),
            (Some(s0), false) => {
                // sample i compares frames i−1 and i: the quiet stretch is frames s0−1 … i−1
                let (a0, b0) = ((s0.max(1) - 1) as f64 / 10.0, (i - 1) as f64 / 10.0);
                if b0 - a0 >= min_idle {
                    out.push((a0, b0));
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

fn speed_label(k: f64) -> String {
    if (k - k.round()).abs() < 0.05 {
        format!("{}×", k.round() as i64)
    } else {
        format!("{k:.1}×")
    }
}

/// One piece of a clip after splitting: source span and playback speed.
#[derive(Debug, Clone, PartialEq)]
struct Piece {
    id: String,
    a: f64,
    b: f64,
    speed: f64,
}

pub fn fast_forward_idle(
    l: &Loaded,
    q: &mut Project,
    ff: &FastForwardIdle,
    brand: &Brand,
    notes: &mut Vec<Diagnostic>,
) -> Result<()> {
    let (Some(tl), _) = resolve(q, &l.media, &context(&l.dir)) else { return Ok(()) };
    // where someone speaks on the timeline: never fast-forward under narration
    let words = timeline_words(l, q, &tl, brand, false).unwrap_or_default();
    let speech = cutline_transcribe::mapping::speech_spans(&words, 0.3);
    let mut new_main: Vec<MainItem> = vec![];
    let mut split: BTreeMap<String, (f64, Vec<Piece>)> = BTreeMap::new(); // clip id → (in, pieces)
    let mut badges: Vec<Layer> = vec![];
    let mut ci = 0usize;
    for item in &q.main {
        let MainItem::Clip(c) = item else {
            new_main.push(item.clone());
            continue;
        };
        let tc = &tl.clips[ci];
        ci += 1;
        let eligible = tc.kind == AssetKind::Video
            && !c.is_bumper()
            && c.end_at.is_none()
            && c.speed_ramp.is_empty()
            && !c.reverse
            && (tc.speed - 1.0).abs() < 1e-9
            && (ff.applies_to.is_empty() || ff.applies_to.contains(&c.clip));
        if !eligible {
            new_main.push(item.clone());
            continue;
        }
        let act = activity(l, &c.clip, tc.src_in, tc.src_out)?;
        let t_start = tl.secs(tc.start);
        let runs: Vec<(f64, f64)> = idle_runs(&act, ff.activity, ff.min_idle)
            .into_iter()
            // normal speed at both ends, so the change of pace is not a jolt
            .map(|(a, b)| (tc.src_in + a + ff.keep, (tc.src_in + b - ff.keep).min(tc.src_out)))
            .filter(|(a, b)| b - a >= 1.0)
            .filter(|(a, b)| {
                let (ta, tb) = (t_start + a - tc.src_in, t_start + b - tc.src_in);
                !speech.iter().any(|s| s.0 < tb && s.1 > ta)
            })
            .collect();
        if runs.is_empty() {
            new_main.push(item.clone());
            continue;
        }
        let mut spans: Vec<(f64, f64, f64)> = vec![];
        let mut cur = tc.src_in;
        for (a, b) in &runs {
            if *a - cur > 2.0 / tl.fps.as_f64() {
                spans.push((cur, *a, 1.0));
            }
            spans.push((cur.max(*a), *b, ff.speed));
            cur = *b;
        }
        if tc.src_out - cur > 2.0 / tl.fps.as_f64() {
            spans.push((cur, tc.src_out, 1.0));
        }
        let mut pieces = vec![];
        for (k, (a, b, sp)) in spans.iter().enumerate() {
            let id = if k == 0 { c.id.clone() } else { format!("{}~{}", c.id, k + 1) };
            let mut nc: Clip = (**c).clone();
            nc.id = id.clone();
            nc.in_ = Some(TimeValue::Seconds(round6(*a)));
            nc.out = Some(TimeValue::Seconds(round6(*b)));
            nc.duration = None;
            if *sp > 1.0 {
                nc.speed = Some(*sp);
                // sped-up interface sounds are noise
                if nc.audio.as_deref().map_or(true, |x| x == "keep" || x == "duck") {
                    nc.audio = Some("mute".into());
                }
                if ff.badge {
                    badges.push(Layer::Badge {
                        text: speed_label(*sp),
                        position: None,
                        size: None,
                        generated_by: Some("fast_forward_idle".into()),
                        at: placement(&id, 0.0, (b - a) / sp),
                    });
                }
            }
            nc.generated_by = Some("fast_forward_idle".into());
            // effects keep their place in the footage
            nc.effects = c
                .effects
                .iter()
                .filter_map(|e| {
                    let src = tc.src_in + effect_at(e)?;
                    (src >= *a && src < *b).then(|| with_at(e, (src - a) / sp))
                })
                .chain(c.effects.iter().filter(|e| effect_at(e).is_none() && k == 0).cloned())
                .collect();
            if k > 0 {
                nc.chapter = None;
            }
            new_main.push(MainItem::Clip(Box::new(nc)));
            pieces.push(Piece { id, a: *a, b: *b, speed: *sp });
        }
        let saved: f64 = runs.iter().map(|(a, b)| (b - a) * (1.0 - 1.0 / ff.speed)).sum();
        notes.push(note(
            format!("/main/{}", tc.main_index),
            format!(
                "clip `{}`: {} idle stretch(es) play at {}, saving {saved:.1}s",
                c.id,
                runs.len(),
                speed_label(ff.speed)
            ),
        ));
        split.insert(c.id.clone(), (tc.src_in, pieces));
    }
    q.main = new_main;
    // layers pinned to a split clip stay on the same moment of the footage
    for layer in &mut q.layers {
        let Some(at) = layer.placement_mut() else { continue };
        let Some((src_in, pieces)) = at.anchor.as_ref().and_then(|a| split.get(a)) else { continue };
        let src = src_in + at.offset.unwrap_or(0.0);
        let p = pieces.iter().find(|p| src < p.b).unwrap_or(pieces.last().unwrap());
        at.anchor = Some(p.id.clone());
        at.offset = Some(round6(((src - p.a) / p.speed).max(0.0)));
    }
    q.layers.extend(badges);
    Ok(())
}

// ------------------------------------------------------------------ zoom_on_clicks

fn place(r: [f64; 4], content: Option<&Rect>) -> Rect {
    let r = Rect { x: r[0], y: r[1], w: r[2].max(1e-4), h: r[3].max(1e-4) };
    match content {
        Some(c) => Rect { x: c.x + r.x * c.w, y: c.y + r.y * c.h, w: r.w * c.w, h: r.h * c.h },
        None => r,
    }
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let (x0, y0) = (a[0].min(b[0]), a[1].min(b[1]));
    let (x1, y1) = ((a[0] + a[2]).max(b[0] + b[2]), (a[1] + a[3]).max(b[1] + b[3]));
    [x0, y0, x1 - x0, y1 - y0]
}

/// A click (or a run of clicks close together): when it happens in the clip, and where.
#[derive(Debug, Clone, PartialEq)]
struct Target {
    start: f64,
    end: f64,
    rect: [f64; 4],
}

/// Zoom effects that follow `targets` on a clip `len` seconds long: zoom in on the first, pan
/// to each next one while it is close in time, zoom out when a pause follows.
fn follow_effects(targets: &[Target], z: &ZoomOnClicks, content: Option<&Rect>, len: f64) -> Vec<Effect> {
    let d = z.duration;
    // strongest zoom at which a target (with a little air) still fits
    let scale_of = |t: &Target| zoom_to_params(&place(t.rect, content), 0.05, z.max_scale).0;
    let mut out = vec![];
    let mut i = 0;
    while i < targets.len() {
        // a chain: targets that follow each other too closely to zoom out and back in between
        let mut j = i;
        while j + 1 < targets.len() && targets[j + 1].start - (targets[j].end + z.hold) < 2.0 * d + 1.0 {
            j += 1;
        }
        let chain: Vec<&Target> = targets[i..=j].iter().filter(|t| scale_of(t) >= 1.15).collect();
        i = j + 1;
        let Some(first) = chain.first() else { continue };
        let start = (first.start - 0.1).max(0.0);
        if start + d > len - 0.3 {
            continue;
        }
        // one scale for the whole chain, so moving on is a pan, not a pump
        let s = chain.iter().map(|t| scale_of(t)).fold(z.max_scale, f64::min);
        let pad_for = |t: &Target| {
            let r = place(t.rect, content);
            ((1.0 / s - r.w.max(r.h)) / 2.0).clamp(0.0, 0.4)
        };
        let out_at = chain.last().unwrap().end + z.hold;
        let r = first.rect;
        out.push(Effect::ZoomTo {
            rect: Rect { x: r[0], y: r[1], w: r[2].max(1e-4), h: r[3].max(1e-4) },
            start: round6(start),
            duration: d,
            ease: Ease::InOutCubic,
            padding: Some(round6(pad_for(first))),
            // zoom back out when there is room for it before the clip ends
            end: (out_at + d < len - 0.05).then(|| round6(out_at.max(start + d))),
        });
        for t in &chain[1..] {
            let (_, focus) = zoom_to_params(&place(t.rect, content), pad_for(t), 3.0);
            let st = (t.start - 0.1).max(start + d);
            out.push(Effect::Zoom {
                from: 1.0,
                to: 1.0,
                start: round6(st),
                end: Some(round6(st + d)),
                ease: Ease::InOutCubic,
                focus: Some(Point { x: round6(focus.x), y: round6(focus.y) }),
            });
        }
    }
    out
}

/// Clips that play a browser recording at a steady speed, with the recording's events.
fn recording_clips<'a>(l: &'a Loaded, tl: &'a cutline_core::Timeline) -> Vec<(&'a TClip, &'a DemoEvents)> {
    tl.clips
        .iter()
        .filter(|c| !c.clip.is_bumper() && c.clip.speed_ramp.is_empty() && !c.clip.reverse)
        .filter_map(|c| l.demos.get(&c.asset).map(|ev| (c, ev)))
        .collect()
}

pub fn zoom_on_clicks(l: &Loaded, q: &mut Project, z: &ZoomOnClicks, notes: &mut Vec<Diagnostic>) {
    let (Some(tl), _) = resolve(q, &l.media, &context(&l.dir)) else { return };
    let mut add: BTreeMap<usize, Vec<Effect>> = BTreeMap::new();
    for (c, ev) in recording_clips(l, &tl) {
        let moving = c.clip.effects.iter().any(|e| {
            matches!(e, Effect::PunchIn { .. } | Effect::Zoom { .. } | Effect::ZoomTo { .. } | Effect::KenBurns { .. })
        });
        // a clip with its own zooms keeps them; a fast-forwarded piece is not the place to zoom
        if moving || c.speed > 1.5 {
            continue;
        }
        let local = |t: f64| ((t - c.src_in) / c.speed).max(0.0);
        let mut targets: Vec<Target> = vec![];
        for st in ev.steps.iter().filter(|s| matches!(s.kind.as_str(), "click" | "type")) {
            let Some(rect) = st.rect else { continue };
            if st.t < c.src_in || st.t >= c.src_out {
                continue;
            }
            if !z.steps.is_empty() && !st.name.as_ref().is_some_and(|n| z.steps.contains(n)) {
                continue;
            }
            let t = Target { start: local(st.t0.max(c.src_in)), end: local(st.t1), rect };
            // clicks in quick succession share one framing while that framing is still a real
            // zoom; clicks far apart on screen are followed with a pan instead
            let together = |prev: &Target| {
                t.start - prev.end <= z.merge_within
                    && zoom_to_params(&place(union(prev.rect, t.rect), c.content.as_ref()), 0.05, z.max_scale).0 >= 1.4
            };
            match targets.last_mut() {
                Some(prev) if together(prev) => {
                    prev.end = t.end;
                    prev.rect = union(prev.rect, t.rect);
                }
                _ => targets.push(t),
            }
        }
        let effects = follow_effects(&targets, z, c.content.as_ref(), tl.secs(c.frames));
        if !effects.is_empty() {
            notes.push(note(
                format!("/main/{}", c.main_index),
                format!("clip `{}`: {} zoom move(s) follow its clicks", c.id, effects.len()),
            ));
            add.insert(c.main_index, effects);
        }
    }
    for (mi, effs) in add {
        if let MainItem::Clip(c) = &mut q.main[mi] {
            c.effects.extend(effs);
        }
    }
}

// ------------------------------------------------------------------ cursor

/// Pointer position at recording time `t` (eased between the recorded points, as the recorder
/// moved it).
pub fn pointer_at(cur: &[CursorPoint], t: f64) -> (f64, f64) {
    let Some(first) = cur.first() else { return (0.5, 0.5) };
    let mut pos = (first.x, first.y);
    for w in cur.windows(2) {
        if t >= w[1].t {
            pos = (w[1].x, w[1].y);
        } else if t > w[0].t {
            let u = crate::effects::ease(Ease::InOutCubic, (t - w[0].t) / (w[1].t - w[0].t).max(1e-6));
            pos = (w[0].x + (w[1].x - w[0].x) * u, w[0].y + (w[1].y - w[0].y) * u);
            break;
        }
    }
    pos
}

pub fn cursor(l: &Loaded, q: &mut Project, cfg: &AutoCursor, notes: &mut Vec<Diagnostic>) {
    let (Some(tl), _) = resolve(q, &l.media, &context(&l.dir)) else { return };
    let mut layers = vec![];
    for (c, ev) in recording_clips(l, &tl) {
        let local = |t: f64| round6(((t - c.src_in) / c.speed).max(0.0));
        let (x0, y0) = pointer_at(&ev.cursor, c.src_in);
        let mut path = vec![CursorKey { t: 0.0, x: round6(x0), y: round6(y0), click: false }];
        for p in ev.cursor.iter().filter(|p| p.t > c.src_in && p.t < c.src_out) {
            path.push(CursorKey { t: local(p.t), x: p.x, y: p.y, click: p.click });
        }
        // where the pointer is when the clip ends (it may be mid-move)
        let len = tl.secs(c.frames);
        let (x1, y1) = pointer_at(&ev.cursor, c.src_out);
        if path.last().is_some_and(|k| k.t < round6(len)) {
            path.push(CursorKey { t: round6(len), x: round6(x1), y: round6(y1), click: false });
        }
        layers.push(Layer::Cursor {
            path,
            size: Some(cfg.size),
            click_ripple: Some(cfg.click_ripple),
            color: None,
            follow: None,
            generated_by: Some("cursor".into()),
            at: placement(&c.id, 0.0, len),
        });
        if cfg.keys {
            for k in ev.keys.iter().filter(|k| k.t >= c.src_in && k.t < c.src_out) {
                let special = k
                    .keys
                    .iter()
                    .any(|n| matches!(n.to_ascii_lowercase().as_str(), "enter" | "escape" | "esc" | "tab"));
                if k.keys.len() < 2 && !special {
                    continue;
                }
                let at = local(k.t);
                layers.push(Layer::Keys {
                    keys: k.keys.clone(),
                    position: None,
                    size: None,
                    generated_by: Some("cursor".into()),
                    at: placement(&c.id, at, 1.4f64.min((len - at).max(0.2))),
                });
            }
        }
    }
    if !layers.is_empty() {
        notes.push(note(
            "/auto/cursor".into(),
            format!("{} pointer and shortcut layer(s) from the recordings", layers.len()),
        ));
        q.layers.extend(layers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_runs_need_a_quiet_stretch_of_min_length() {
        // 1 s busy, 4 s quiet, 1 s busy, 1 s quiet (too short), busy
        let mut act = vec![1.0f32; 10];
        act.extend(vec![0.0; 40]);
        act.extend(vec![0.5; 10]);
        act.extend(vec![0.001; 10]);
        act.push(0.9);
        let runs = idle_runs(&act, 0.006, 2.5);
        assert_eq!(runs.len(), 1);
        assert!((runs[0].0 - 0.9).abs() < 1e-9 && (runs[0].1 - 4.9).abs() < 1e-9, "{runs:?}");
        // quiet to the very end counts
        assert_eq!(idle_runs(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 0.006, 0.5), vec![(0.0, 0.6)]);
        assert_eq!(speed_label(4.0), "4×");
        assert_eq!(speed_label(2.5), "2.5×");
    }

    fn cfg() -> ZoomOnClicks {
        ZoomOnClicks { enabled: true, max_scale: 2.0, merge_within: 2.5, hold: 1.2, duration: 0.6, steps: vec![] }
    }

    #[test]
    fn zooms_chain_into_pans_when_clicks_are_close_and_zoom_out_after_a_pause() {
        let t = |s: f64, x: f64| Target { start: s, end: s + 0.5, rect: [x, 0.2, 0.2, 0.1] };
        // two clicks 3.5 s apart (one chain), then one after a long pause
        let e = follow_effects(&[t(1.0, 0.1), t(4.5, 0.6), t(14.0, 0.3)], &cfg(), None, 30.0);
        assert_eq!(e.len(), 3);
        let Effect::ZoomTo { start, end, padding, .. } = &e[0] else { panic!("{:?}", e[0]) };
        assert!((start - 0.9).abs() < 1e-6);
        // zooms out 1.2 s after the chain's last click ends
        assert!((end.unwrap() - (5.0 + 1.2)).abs() < 1e-6, "{end:?}");
        // padded so the scale is exactly max_scale: the 0.2-wide rect fills half the view
        assert!((padding.unwrap() - 0.15).abs() < 1e-6);
        let Effect::Zoom { from, to, start, focus, .. } = &e[1] else { panic!("{:?}", e[1]) };
        assert_eq!((*from, *to), (1.0, 1.0));
        assert!((start - 4.4).abs() < 1e-6 && focus.unwrap().x > 0.6, "pans right to the second click");
        assert!(matches!(&e[2], Effect::ZoomTo { start, .. } if (start - 13.9).abs() < 1e-6));
        // a target that already fills the frame is not worth a zoom
        let wide = Target { start: 1.0, end: 1.5, rect: [0.02, 0.02, 0.95, 0.9] };
        assert!(follow_effects(&[wide], &cfg(), None, 30.0).is_empty());
        // no room to zoom in before the clip ends
        assert!(follow_effects(&[t(9.5, 0.1)], &cfg(), None, 10.0).is_empty());
    }

    #[test]
    fn pointer_is_interpolated_between_recorded_points() {
        let cur = vec![
            CursorPoint { t: 0.0, x: 0.5, y: 0.5, click: false },
            CursorPoint { t: 1.0, x: 0.5, y: 0.5, click: false },
            CursorPoint { t: 2.0, x: 0.1, y: 0.3, click: false },
        ];
        assert_eq!(pointer_at(&cur, 0.5), (0.5, 0.5));
        let (x, y) = pointer_at(&cur, 1.5);
        assert!((x - 0.3).abs() < 1e-9 && (y - 0.4).abs() < 1e-9);
        assert_eq!(pointer_at(&cur, 9.0), (0.1, 0.3));
    }
}
