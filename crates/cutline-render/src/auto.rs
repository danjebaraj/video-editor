//! Transcripts in timeline time, and `auto` directive expansion (jump cuts, filler removal,
//! punch-ins, SFX on transitions) into explicit, inspectable timeline items.

use crate::pipeline::{context, Loaded};
use anyhow::Result;
use cutline_core::brand::Brand;
use cutline_core::diag::Diagnostic;
use cutline_core::model::{Clip, Effect, Layer, MainItem, Project, Role};
use cutline_core::timeline::{resolve, Timeline};
use cutline_media::cache::Cache;
use cutline_transcribe::{mapping, Transcript, Word};
use std::collections::BTreeMap;

/// Transcripts are cached globally (by asset fingerprint + model) so `cutline transcribe` and
/// renders share them.
pub fn transcript_cache(_l: &Loaded) -> Cache {
    Cache::global("transcripts")
}

pub fn whisper_model() -> String {
    cutline_transcribe::whisper::resolve_model()
}

/// Transcript (source time) for an asset, with corrections applied. `None` when no transcript
/// file, cache entry or whisper is available.
pub fn transcript_for(
    l: &Loaded,
    p: &Project,
    brand: &Brand,
    asset: &str,
    allow_whisper: bool,
) -> Result<Option<Transcript>> {
    let Some(a) = p.assets.get(asset) else { return Ok(None) };
    let Some(m) = l.media.get(asset) else { return Ok(None) };
    if !m.facts.exists || !m.facts.has_audio && a.transcript.is_none() {
        return Ok(None);
    }
    let tc = cutline_deps::toolchain()?;
    let tfile = a.transcript.as_ref().map(|t| l.dir.join(t));
    let lang = p.captions.as_ref().and_then(|c| c.lang.clone());
    let got = cutline_transcribe::get(
        tc,
        asset,
        &m.path,
        &m.fingerprint,
        tfile.as_deref(),
        &whisper_model(),
        lang.as_deref(),
        &transcript_cache(l),
        allow_whisper,
    )?;
    let Some((t, _src)) = got else { return Ok(None) };
    let (mut t, _) = with_script(&l.dir, a, t)?;
    let mut corr: BTreeMap<String, String> = brand.dictionary.clone();
    if let Some(c) = &p.captions {
        corr.extend(c.corrections.clone());
    }
    t.apply_corrections(&corr);
    Ok(Some(t))
}

/// Transcript (source time) for an asset before the project is probed (used to resolve cues):
/// the asset's `transcript` file, else the shared cache (keyed like `transcript_for`), else
/// whisper when `allow_whisper`. `None` when nothing is available.
pub fn load_transcript(
    dir: &std::path::Path,
    asset_id: &str,
    asset: Option<&cutline_core::model::Asset>,
    lang: Option<&str>,
    allow_whisper: bool,
) -> Result<Option<Transcript>> {
    let Some(a) = asset else { anyhow::bail!("unknown asset `{asset_id}`") };
    let tfile = a.transcript.as_ref().map(|t| dir.join(t));
    let media = dir.join(&a.path);
    let fingerprint = if tfile.is_some() {
        String::new()
    } else {
        cutline_media::fingerprint_any(&media).map_err(|e| anyhow::anyhow!("cannot read {}: {e}", media.display()))?
    };
    let tc = cutline_deps::toolchain()?;
    let got = cutline_transcribe::get(
        tc,
        asset_id,
        &media,
        &fingerprint,
        tfile.as_deref(),
        &whisper_model(),
        lang,
        &Cache::global("transcripts"),
        allow_whisper,
    )?;
    match got {
        Some((t, _)) => Ok(Some(with_script(dir, a, t)?.0)),
        None => Ok(None),
    }
}

/// Replace a transcript's words with the asset's `script`, keeping the transcript's timing.
/// Returns the transcript unchanged (and no stats) when the asset has no script.
pub fn with_script(
    dir: &std::path::Path,
    a: &cutline_core::model::Asset,
    t: Transcript,
) -> Result<(Transcript, Option<cutline_transcribe::align::AlignStats>)> {
    let Some(s) = &a.script else { return Ok((t, None)) };
    let path = dir.join(s);
    let text = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("script file {}: {e}", path.display()))?;
    let (aligned, stats) = cutline_transcribe::align::align(&t, &text);
    Ok((aligned, Some(stats)))
}

/// Warnings for assets whose script and audio disagree (wrong script file, or a different take).
pub fn script_warnings(l: &Loaded, p: &Project) -> Vec<Diagnostic> {
    let mut out = vec![];
    for (id, a) in &p.assets {
        if a.script.is_none() {
            continue;
        }
        let lang = p.captions.as_ref().and_then(|c| c.lang.clone());
        // transcript without the script, to measure how well they match
        let bare = cutline_core::model::Asset { script: None, ..a.clone() };
        let Ok(Some(t)) = load_transcript(&l.dir, id, Some(&bare), lang.as_deref(), false) else { continue };
        let Ok((_, Some(st))) = with_script(&l.dir, a, t) else { continue };
        if st.match_ratio() < 0.7 {
            out.push(
                Diagnostic::warning(
                    "W_SCRIPT_MISMATCH",
                    format!("/assets/{}/script", cutline_core::diag::ptr_token(id)),
                    format!(
                        "only {:.0}% of the script's {} words were heard in `{id}` ({} missing, {} extra in the audio)",
                        st.match_ratio() * 100.0,
                        st.script_words,
                        st.inserted + st.substituted,
                        st.dropped
                    ),
                )
                .hint("check that the script file is the text of this take"),
            );
        }
    }
    out
}

/// Assets whose speech feeds captions/ducking: explicit `captions.assets`, else every clip
/// asset with audio, every voice-role audio layer and every clip audio replacement.
pub fn speech_assets(p: &Project, l: &Loaded) -> Vec<String> {
    if let Some(c) = &p.captions {
        if !c.assets.is_empty() {
            return c.assets.clone();
        }
    }
    let mut v: Vec<String> = vec![];
    for c in p.clips() {
        if l.media.get(&c.clip).map(|m| m.facts.has_audio).unwrap_or(false)
            || p.assets.get(&c.clip).map(|a| a.transcript.is_some()).unwrap_or(false)
        {
            v.push(c.clip.clone());
        }
        if let Some(a) = &c.audio {
            if p.assets.contains_key(a) {
                v.push(a.clone());
            }
        }
    }
    for layer in &p.layers {
        if let Layer::Audio { asset, role, .. } = layer {
            let r = role.or(p.assets.get(asset).and_then(|a| a.role));
            if r == Some(Role::Voice) || p.assets.get(asset).map(|a| a.transcript.is_some()).unwrap_or(false) {
                v.push(asset.clone());
            }
        }
    }
    v.sort();
    v.dedup();
    v
}

/// All spoken words in timeline seconds (sorted).
pub fn timeline_words(l: &Loaded, p: &Project, tl: &Timeline, brand: &Brand, allow_whisper: bool) -> Result<Vec<Word>> {
    let assets = speech_assets(p, l);
    let mut trs: BTreeMap<String, Transcript> = BTreeMap::new();
    for a in &assets {
        if let Some(t) = transcript_for(l, p, brand, a, allow_whisper)? {
            trs.insert(a.clone(), t);
        }
    }
    let mut out = vec![];
    for c in &tl.clips {
        match c.clip.audio.as_deref() {
            Some("mute") => {}
            Some(x) if x != "keep" && x != "duck" => {
                if let Some(t) = trs.get(x) {
                    out.extend(mapping::map_layer(&t.words, tl.secs(c.start), tl.secs(c.end()), 0.0));
                }
            }
            _ => {
                if let Some(t) = trs.get(&c.asset) {
                    out.extend(mapping::map_clip(&t.words, c, tl));
                }
            }
        }
    }
    for tlay in &tl.layers {
        if let Layer::Audio { asset, src_in, .. } = &tlay.layer {
            if let Some(t) = trs.get(asset) {
                let si = src_in.map(|v| v.seconds(tl.fps, 0.0)).unwrap_or(0.0);
                out.extend(mapping::map_layer(&t.words, tl.secs(tlay.start), tl.secs(tlay.end), si));
            }
        }
    }
    out.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    Ok(out)
}

/// Hard cut times on the timeline (clip boundaries without transitions + transition midpoints).
pub fn cut_times(tl: &Timeline) -> Vec<f64> {
    let mut v = vec![];
    for c in tl.clips.iter().skip(1) {
        match tl.transitions.iter().find(|t| t.after + 1 == c.index) {
            Some(t) if t.frames > 0 => v.push(tl.secs(t.start) + tl.secs(t.frames) / 2.0),
            _ => v.push(tl.secs(c.start)),
        }
    }
    v
}

// ------------------------------------------------------------------ silence analysis

/// silencedetect spans (absolute source seconds) within [from, to), cached.
pub fn silences(l: &Loaded, asset: &str, from: f64, to: f64, noise_db: f64, min_d: f64) -> Result<Vec<(f64, f64)>> {
    let m = l.media.get(asset).ok_or_else(|| anyhow::anyhow!("no media for {asset}"))?;
    if !m.facts.has_audio {
        return Ok(vec![]);
    }
    let cache = Cache::new(crate::cutline_dir(&l.dir).join("cache").join("analysis"));
    let key = cutline_media::hash_key(&[
        m.fingerprint.as_bytes(),
        format!("{from:.4}-{to:.4}-{noise_db}-{min_d}").as_bytes(),
        b"silence-v1",
    ]);
    if let Some(v) = cache.get_json::<Vec<(f64, f64)>>(&key) {
        return Ok(v);
    }
    let tc = cutline_deps::toolchain()?;
    let o = std::process::Command::new(&tc.ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-ss",
            &format!("{from:.6}"),
            "-t",
            &format!("{:.6}", to - from),
            "-i",
        ])
        .arg(&m.path)
        .args(["-vn", "-af", &format!("silencedetect=n={noise_db}dB:d={min_d}"), "-f", "null", "-"])
        .output()?;
    let s = String::from_utf8_lossy(&o.stderr);
    let mut v = vec![];
    let mut cur: Option<f64> = None;
    for line in s.lines() {
        if let Some(p) = line.find("silence_start: ") {
            cur = line[p + 15..].split_whitespace().next().and_then(|x| x.parse().ok());
        }
        if let Some(p) = line.find("silence_end: ") {
            let e: Option<f64> = line[p + 13..].split_whitespace().next().and_then(|x| x.parse().ok());
            if let (Some(a), Some(b)) = (cur.take(), e) {
                v.push((from + a, from + b));
            }
        }
    }
    if let Some(a) = cur {
        v.push((from + a, to));
    }
    cache.put_json(&key, &v)?;
    Ok(v)
}

fn intersect(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut v = vec![];
    for &(a0, a1) in a {
        for &(b0, b1) in b {
            let (s, e) = (a0.max(b0), a1.min(b1));
            if e > s {
                v.push((s, e));
            }
        }
    }
    v
}

fn merge(mut v: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut out: Vec<(f64, f64)> = vec![];
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect::<String>().to_lowercase()
}

// ------------------------------------------------------------------ expansion

/// Apply `auto` directives. Returns the explicit project (with `auto` removed) and notes.
pub fn expand(l: &Loaded, p: &Project, brand: &Brand) -> Result<(Project, Vec<Diagnostic>)> {
    let Some(auto) = p.auto.clone() else { return Ok((p.clone(), vec![])) };
    let mut notes = vec![];
    let mut q = p.clone();
    q.auto = None;
    let jc = auto.jump_cuts.clone().filter(|j| j.enabled);
    let rf = auto.remove_fillers.clone().filter(|r| r.enabled);
    if jc.is_some() || rf.is_some() {
        // resolve once for source ranges
        let (tl0, _) = resolve(p, &l.media, &context(&l.dir));
        let tl0 = tl0.ok_or_else(|| anyhow::anyhow!("project does not resolve; fix validation errors first"))?;
        let mut new_main: Vec<MainItem> = vec![];
        let mut ci = 0usize;
        for item in &p.main {
            let MainItem::Clip(c) = item else {
                new_main.push(item.clone());
                continue;
            };
            let tc = &tl0.clips[ci];
            ci += 1;
            if c.end_at.is_some() {
                // a fitted clip's length is set by its cue; splitting it would break the fit
                new_main.push(item.clone());
                continue;
            }
            let mut remove: Vec<(f64, f64)> = vec![];
            let transcript = transcript_for(l, p, brand, &c.clip, false)?;
            let mut generated = "";
            if let Some(j) = &jc {
                if (j.applies_to.is_empty() || j.applies_to.contains(&c.clip)) && c.speed_ramp.is_empty() && !c.reverse
                {
                    let mut sil =
                        silences(l, &c.clip, tc.src_in, tc.src_out, j.noise_db.unwrap_or(-35.0), j.min_silence)?;
                    if let Some(t) = &transcript {
                        // intersect with transcript gaps
                        let mut gaps = vec![];
                        let mut prev = tc.src_in;
                        for w in t.words.iter().filter(|w| w.end > tc.src_in && w.start < tc.src_out) {
                            if w.start - prev >= j.min_silence {
                                gaps.push((prev, w.start));
                            }
                            prev = prev.max(w.end);
                        }
                        if tc.src_out - prev >= j.min_silence {
                            gaps.push((prev, tc.src_out));
                        }
                        sil = intersect(&sil, &gaps);
                    }
                    for (a, b) in sil {
                        if b - a > j.min_silence {
                            let (ra, rb) = (
                                if a <= tc.src_in + 1e-6 { a } else { a + j.keep_padding },
                                if b >= tc.src_out - 1e-6 { b } else { b - j.keep_padding },
                            );
                            if rb > ra {
                                remove.push((ra, rb));
                                generated = "jump_cuts";
                            }
                        }
                    }
                }
            }
            if let (Some(r), Some(t)) = (&rf, &transcript) {
                let fill: Vec<String> = r.words.iter().map(|w| norm(w)).collect();
                let ws: Vec<&Word> = t.words.iter().filter(|w| w.end > tc.src_in && w.start < tc.src_out).collect();
                for (k, w) in ws.iter().enumerate() {
                    if fill.contains(&norm(&w.w)) {
                        let lo = if k > 0 { ws[k - 1].end } else { tc.src_in };
                        let hi = ws.get(k + 1).map(|n| n.start).unwrap_or(tc.src_out);
                        remove.push(((w.start - 0.03).max(lo), (w.end + 0.03).min(hi)));
                        if generated.is_empty() {
                            generated = "remove_fillers";
                        }
                    }
                }
            }
            let remove = merge(remove);
            if remove.is_empty() {
                new_main.push(item.clone());
                continue;
            }
            // kept source pieces
            let min_piece = 2.0 / tl0.fps.as_f64();
            let mut keep = vec![];
            let mut cur = tc.src_in;
            for (a, b) in &remove {
                if *a > cur {
                    keep.push((cur, *a));
                }
                cur = cur.max(*b);
            }
            if tc.src_out > cur {
                keep.push((cur, tc.src_out));
            }
            keep.retain(|(a, b)| b - a >= min_piece);
            notes.push(Diagnostic::warning(
                "I_AUTO_EXPANDED",
                format!("/main/{}", tc.main_index),
                format!("clip `{}` split into {} pieces by {generated}", c.id, keep.len()),
            ));
            for (k, (a, b)) in keep.iter().enumerate() {
                let mut nc: Clip = (**c).clone();
                if k > 0 {
                    nc.id = format!("{}~{}", c.id, k + 1);
                }
                nc.in_ = Some(cutline_core::time::TimeValue::Seconds(cutline_core::time::round6(*a)));
                nc.out = Some(cutline_core::time::TimeValue::Seconds(cutline_core::time::round6(*b)));
                nc.duration = None;
                nc.generated_by = Some(generated.to_string());
                // effects follow their source time
                nc.effects = c
                    .effects
                    .iter()
                    .filter_map(|e| {
                        let at = effect_at(e)?;
                        let src = tc.src_in + at * tc.speed;
                        (src >= *a && src < *b).then(|| with_at(e, (src - a) / tc.speed))
                    })
                    .chain(c.effects.iter().filter(|e| effect_at(e).is_none() && k == 0).cloned())
                    .collect();
                new_main.push(MainItem::Clip(Box::new(nc)));
            }
        }
        q.main = new_main;
    }
    if let Some(sfx) = &auto.sfx_on_transitions {
        for m in &mut q.main {
            if let MainItem::Transition(t) = m {
                if t.transition.sfx.is_none() {
                    t.transition.sfx = Some(sfx.clone());
                    t.transition.generated_by = Some("sfx_on_transitions".into());
                }
            }
        }
    }
    if let Some(pi) = &auto.punch_ins {
        let (tl, _) = resolve(&q, &l.media, &context(&l.dir));
        if let Some(tl) = tl {
            let dur = tl.duration();
            let mut k = 1;
            let mut t = pi.every;
            let mut add: BTreeMap<usize, Vec<Effect>> = BTreeMap::new();
            while t < dur - pi.duration {
                if let Some(c) = tl.clip_at(tl.fps.secs_to_frames(t)) {
                    let local = t - tl.secs(c.start);
                    let clip_len = tl.secs(c.frames);
                    let motion = c.clip.effects.iter().any(|e| {
                        matches!(
                            e,
                            Effect::PunchIn { .. }
                                | Effect::Zoom { .. }
                                | Effect::KenBurns { .. }
                                | Effect::Shake { .. }
                        )
                    });
                    let already = add.contains_key(&c.main_index);
                    if !(pi.skip_if_other_motion && motion) && !already && local + pi.duration < clip_len {
                        let scale = pi.scale[(k - 1) % 2];
                        add.entry(c.main_index).or_default().push(Effect::PunchIn {
                            at: cutline_core::time::round6(local),
                            scale,
                            duration: pi.duration,
                            ease: Default::default(),
                            hold: None,
                            focus: None,
                            generated_by: Some("punch_ins".into()),
                        });
                        k += 1;
                    }
                }
                t += pi.every;
            }
            for (mi, effs) in add {
                if let MainItem::Clip(c) = &mut q.main[mi] {
                    c.effects.extend(effs);
                }
            }
        }
    }
    Ok((q, notes))
}

fn effect_at(e: &Effect) -> Option<f64> {
    match e {
        Effect::PunchIn { at, .. }
        | Effect::Shake { at, .. }
        | Effect::Flash { at, .. }
        | Effect::Glitch { at, .. }
        | Effect::MotionBlur { at, .. }
        | Effect::Freeze { at, .. } => Some(*at),
        _ => None,
    }
}

fn with_at(e: &Effect, v: f64) -> Effect {
    let mut e = e.clone();
    match &mut e {
        Effect::PunchIn { at, .. }
        | Effect::Shake { at, .. }
        | Effect::Flash { at, .. }
        | Effect::Glitch { at, .. }
        | Effect::MotionBlur { at, .. }
        | Effect::Freeze { at, .. } => *at = cutline_core::time::round6(v),
        _ => {}
    }
    e
}
