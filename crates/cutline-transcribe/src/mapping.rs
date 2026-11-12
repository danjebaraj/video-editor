//! Map source-time words through the edit (in/out, speed, ramps, freezes, transitions) into
//! timeline time. Words cut out of the edit are dropped.

use crate::model::Word;
use cutline_core::model::{Effect, SpeedKey};
use cutline_core::timeline::{TClip, Timeline};

/// Timeline seconds elapsed for source offset `x` (seconds after the clip's in point).
pub fn source_to_local(x: f64, speed: f64, ramp: &[SpeedKey]) -> f64 {
    if ramp.is_empty() {
        return x / speed;
    }
    cutline_core::timeline::ramp_duration(x, ramp)
}

/// Freeze insertions (clip-relative timeline at, duration).
fn freezes(c: &TClip) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = c
        .clip
        .effects
        .iter()
        .filter_map(|e| if let Effect::Freeze { at, duration } = e { Some((*at, *duration)) } else { None })
        .collect();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    v
}

/// Words of one clip, in timeline seconds. A word is kept when its midpoint lies inside the
/// clip's source range; it is clamped to the clip's timeline span.
pub fn map_clip(words: &[Word], c: &TClip, tl: &Timeline) -> Vec<Word> {
    if c.clip.reverse {
        return vec![];
    }
    let (t0, t1) = (tl.secs(c.start), tl.secs(c.end()));
    let fr = freezes(c);
    let local = |src: f64| -> f64 {
        let mut l = source_to_local(src - c.src_in, c.speed, &c.clip.speed_ramp);
        for (at, d) in &fr {
            if l > *at {
                l += d;
            }
        }
        l
    };
    words
        .iter()
        .filter(|w| {
            let mid = (w.start + w.end) / 2.0;
            mid >= c.src_in && mid < c.src_out
        })
        .map(|w| {
            let s = (t0 + local(w.start.max(c.src_in))).clamp(t0, t1);
            let e = (t0 + local(w.end.min(c.src_out))).clamp(s, t1);
            Word { start: s, end: e, ..w.clone() }
        })
        .collect()
}

/// Words of an audio layer (VO) placed at `layer_start` (timeline s) reading from `src_in`.
pub fn map_layer(words: &[Word], layer_start: f64, layer_end: f64, src_in: f64) -> Vec<Word> {
    words
        .iter()
        .filter_map(|w| {
            let s = layer_start + (w.start - src_in);
            let e = layer_start + (w.end - src_in);
            let mid = (s + e) / 2.0;
            (mid >= layer_start && mid < layer_end).then(|| Word {
                start: s.max(layer_start),
                end: e.min(layer_end),
                ..w.clone()
            })
        })
        .collect()
}

/// Merge word spans into speech activity spans (gaps < `join` joined).
pub fn speech_spans(words: &[Word], join: f64) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = words.iter().map(|w| (w.start, w.end)).collect();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut out: Vec<(f64, f64)> = vec![];
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 + join => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}
