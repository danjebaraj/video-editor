//! Beat references: timeline times named by the music's own grid, replaced by seconds before
//! the project is typed (like cue references).
//!
//! - `"bar:9"`: the first beat of the 9th bar; `"bar:9.3"`: its third beat.
//! - `"beat:33"`: the 33rd beat.
//! - Optional offset in seconds: `"bar:9+0.1"`, `"beat:33-0.05"`.
//!
//! Bars and beats are counted from 1 at the first one that falls on or after timeline 0, on
//! the project's single music layer (`cutline beats` lists them).

use cutline_core::diag::{ptr_token, Diagnostic};
use cutline_core::time::round6;
use serde_json::Value;
use std::path::Path;

/// `bar_confidence` from which bar starts are trusted.
pub use cutline_audio::beats::BAR_CONFIDENT;

/// The music layer's beats in timeline seconds.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BeatGrid {
    pub asset: String,
    pub bpm: f64,
    pub beats: Vec<f64>,
    pub bars: Vec<f64>,
    pub bar_confidence: f64,
    pub steady: bool,
}

fn is_ref(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with("bar:") || t.starts_with("beat:")
}

fn collect(v: &Value, ptr: &str, key: Option<&str>, out: &mut Vec<(String, String)>) {
    match v {
        Value::String(s) if is_ref(s) && key.is_some_and(|k| crate::cues::TIME_FIELDS.contains(&k)) => {
            out.push((ptr.to_string(), s.trim().to_string()))
        }
        Value::Object(o) => {
            for (k, x) in o {
                if ptr.is_empty() && k == "cues" {
                    continue;
                }
                collect(x, &format!("{ptr}/{}", ptr_token(k)), Some(k), out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                collect(x, &format!("{ptr}/{i}"), key, out);
            }
        }
        _ => {}
    }
}

/// The grid of the project's music layer, from the raw project.
pub fn grid(v: &Value, dir: &Path) -> Result<BeatGrid, String> {
    let role_of = |l: &Value, asset: &str| -> Option<String> {
        l.get("role")
            .and_then(|r| r.as_str())
            .or_else(|| v.pointer(&format!("/assets/{}/role", ptr_token(asset))).and_then(|r| r.as_str()))
            .map(String::from)
    };
    let music: Vec<(&Value, &str)> = v
        .get("layers")
        .and_then(|l| l.as_array())
        .into_iter()
        .flatten()
        .filter(|l| l.get("type").and_then(|t| t.as_str()) == Some("audio"))
        .filter_map(|l| l.get("asset").and_then(|a| a.as_str()).map(|a| (l, a)))
        .filter(|(l, a)| role_of(l, a).as_deref() == Some("music"))
        .collect();
    let [(layer, asset)] = music.as_slice() else {
        return Err(if music.is_empty() {
            "beat references need an audio layer with role `music`".to_string()
        } else {
            "beat references need exactly one music layer".to_string()
        });
    };
    if layer.get("align").is_some() || layer.get("anchor").is_some() {
        return Err(
            "the music layer is placed by `align`/`anchor`, so its beats move with the edit; give it a numeric `start` (and `src_in`) to use beat references"
                .to_string(),
        );
    }
    let start = crate::cues::plain_secs(layer.get("start")).map_err(|e| format!("music layer start: {e}"))?;
    let src_in = crate::cues::plain_secs(layer.get("src_in")).map_err(|e| format!("music layer src_in: {e}"))?;
    let path = v
        .pointer(&format!("/assets/{}/path", ptr_token(asset)))
        .and_then(|p| p.as_str())
        .ok_or_else(|| format!("unknown asset `{asset}`"))?;
    let path = dir.join(path);
    let fp = cutline_media::fingerprint_any(&path).map_err(|e| format!("{e:#}"))?;
    let b = crate::auto::beats_of(&path, &fp)
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("no steady beat found in `{asset}`"))?;
    let on_tl = |ts: &[f64]| -> Vec<f64> {
        ts.iter().map(|t| start + t - src_in).filter(|t| *t >= -0.02).map(|t| round6(t.max(0.0))).collect()
    };
    Ok(BeatGrid {
        asset: asset.to_string(),
        bpm: b.bpm,
        beats: on_tl(&b.times),
        bars: on_tl(&b.downbeats),
        bar_confidence: b.bar_confidence,
        steady: b.steady,
    })
}

/// `bar:9`, `bar:9.3`, `beat:33`, each with an optional `±seconds` → timeline seconds.
fn lookup(r: &str, g: &BeatGrid) -> Result<f64, String> {
    let (kind, body) = r.split_once(':').ok_or("not a beat reference")?;
    let (pos, off) = match body[1.min(body.len())..].find(['+', '-']) {
        Some(i) => {
            let (a, b) = body.split_at(i + 1);
            (a, b.parse::<f64>().map_err(|_| format!("invalid offset `{b}`"))?)
        }
        None => (body, 0.0),
    };
    let t = match kind.trim() {
        "beat" => {
            let n: usize = pos.trim().parse().map_err(|_| format!("`{pos}` is not a beat number"))?;
            *g.beats.get(n.wrapping_sub(1)).ok_or(format!("the music has beats 1–{}", g.beats.len()))?
        }
        _ => {
            let (bar, beat) = match pos.split_once('.') {
                Some((a, b)) => (a, b.trim().parse::<usize>().map_err(|_| format!("`{b}` is not a beat of the bar"))?),
                None => (pos, 1),
            };
            let n: usize = bar.trim().parse().map_err(|_| format!("`{bar}` is not a bar number"))?;
            let per = cutline_audio::beats::BEATS_PER_BAR;
            if !(1..=per).contains(&beat) {
                return Err(format!("a bar has beats 1–{per}"));
            }
            let t0 = *g.bars.get(n.wrapping_sub(1)).ok_or(format!("the music has bars 1–{}", g.bars.len()))?;
            let i = g.beats.iter().position(|b| (b - t0).abs() < 1e-3).ok_or("bar start is not on the grid")?;
            *g.beats.get(i + beat - 1).ok_or("that beat is past the end of the music")?
        }
    };
    Ok(t + off)
}

/// Replace every beat reference in `v` by timeline seconds.
pub fn resolve(v: &mut Value, dir: &Path) -> Vec<Diagnostic> {
    let mut refs = vec![];
    collect(v, "", None, &mut refs);
    if refs.is_empty() {
        return vec![];
    }
    let mut d = vec![];
    let g = match grid(v, dir) {
        Ok(g) => g,
        Err(e) => {
            let (ptr, r) = &refs[0];
            return vec![Diagnostic::error("E_BEAT_REF", ptr.clone(), format!("`{r}`: {e}"))];
        }
    };
    let mut uses_bars = false;
    for (ptr, r) in &refs {
        if !crate::cues::is_timeline_field(ptr) {
            d.push(
                Diagnostic::error("E_BEAT_REF", ptr.clone(), format!("`{r}`: beat references are timeline times"))
                    .hint("use them in clip end_at, layer start/end, sfx at or marker at"),
            );
            continue;
        }
        uses_bars |= r.starts_with("bar:");
        match lookup(r, &g) {
            Ok(t) if t >= 0.0 => {
                if let Some(slot) = v.pointer_mut(ptr) {
                    *slot = serde_json::json!(round6(t));
                }
            }
            Ok(t) => d.push(Diagnostic::error("E_BEAT_REF", ptr.clone(), format!("`{r}` resolves to {t:.3}s, before 0"))),
            Err(e) => d.push(
                Diagnostic::error("E_BEAT_REF", ptr.clone(), format!("`{r}`: {e}"))
                    .hint("`cutline beats project.json5` lists the bars and beats"),
            ),
        }
    }
    if uses_bars && g.bar_confidence < cutline_audio::beats::BAR_CONFIDENT {
        d.push(
            Diagnostic::warning(
                "W_BARS_UNCERTAIN",
                "/layers",
                format!("the bar starts of `{}` are not clear (confidence {:.2}); `bar:` times may sit on the wrong beat of the bar", g.asset, g.bar_confidence),
            )
            .hint("listen to a draft; if the cuts feel off the bar, use `beat:N` times instead"),
        );
    }
    d
}
