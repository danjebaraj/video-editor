//! Align a known script to a transcript: the script supplies the words (spelling, capitals,
//! punctuation), the transcript supplies the timing. Script words the recogniser missed get
//! times interpolated from their neighbours; transcript words absent from the script are
//! dropped.

use crate::model::{Transcript, Word};
use crate::text::{norm, same_word, similarity};
use serde::Serialize;

/// How well the script matched the audio.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AlignStats {
    pub script_words: usize,
    /// Script words paired with an identical transcript word.
    pub exact: usize,
    /// Paired with a close transcript word ("panes"/"pains").
    pub close: usize,
    /// Paired with a different word (e.g. "twenty-four" for "24").
    pub substituted: usize,
    /// Script words with no transcript word (timing interpolated).
    pub inserted: usize,
    /// Transcript words not in the script (dropped).
    pub dropped: usize,
}

impl AlignStats {
    /// Share of script words matched exactly or closely.
    pub fn match_ratio(&self) -> f64 {
        if self.script_words == 0 {
            return 1.0;
        }
        (self.exact + self.close) as f64 / self.script_words as f64
    }
}

/// Spoken words of a script: whitespace-separated tokens, without `[audio tags]` such as
/// ElevenLabs' `[warm]` or `[laughs]`, and without tokens that are only punctuation.
pub fn script_words(script: &str) -> Vec<String> {
    let mut out = String::with_capacity(script.len());
    let mut depth = 0usize;
    for c in script.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace()
        .map(|s| s.trim_matches(|c: char| c == '—' || c == '–'))
        .filter(|s| !norm(s).is_empty())
        .map(String::from)
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Pair,
    /// script word without a transcript word
    Insert,
    /// transcript word without a script word
    Drop,
}

const MATCH: i32 = 3;
const CLOSE: i32 = 2;
const SUB: i32 = -1;
const GAP: i32 = -2;

/// Align `script` to `t`. Returns the transcript with the script's words (segments rebuilt).
pub fn align(t: &Transcript, script: &str) -> (Transcript, AlignStats) {
    let sw = script_words(script);
    let tw: Vec<&Word> = t.words.iter().filter(|w| !norm(&w.w).is_empty()).collect();
    let (n, m) = (sw.len(), tw.len());
    let mut stats = AlignStats { script_words: n, ..Default::default() };
    if n == 0 {
        return (t.clone(), stats);
    }
    let sn: Vec<String> = sw.iter().map(|s| norm(s)).collect();
    let tn: Vec<String> = tw.iter().map(|w| norm(&w.w)).collect();
    let score = |i: usize, j: usize| -> i32 {
        if sn[i] == tn[j] {
            MATCH
        } else if same_word(&sn[i], &tn[j]) {
            CLOSE
        } else {
            SUB
        }
    };
    // Banded Needleman–Wunsch around the diagonal (full DP for scripts of normal length).
    let band = (n.abs_diff(m) + 200).max(400);
    let lo = |i: usize| -> usize { (i * m / n.max(1)).saturating_sub(band) };
    let hi = |i: usize| -> usize { (i * m / n.max(1) + band).min(m) };
    let width = 2 * band + 2;
    let neg = i32::MIN / 4;
    let mut h = vec![neg; (n + 1) * width];
    let mut tr = vec![Op::Drop; (n + 1) * width];
    let at = |i: usize, j: usize| -> Option<usize> {
        let l = lo(i);
        (j >= l && j <= hi(i)).then(|| i * width + (j - l))
    };
    for j in lo(0)..=hi(0) {
        let k = at(0, j).unwrap();
        h[k] = GAP * j as i32;
        tr[k] = Op::Drop;
    }
    for i in 1..=n {
        for j in lo(i)..=hi(i) {
            let k = at(i, j).unwrap();
            let mut best = (neg, Op::Insert);
            if let Some(u) = at(i - 1, j) {
                best = (h[u].saturating_add(GAP), Op::Insert);
            }
            if j > 0 {
                if let Some(dg) = at(i - 1, j - 1) {
                    let v = h[dg].saturating_add(score(i - 1, j - 1));
                    if v > best.0 {
                        best = (v, Op::Pair);
                    }
                }
                if let Some(lf) = at(i, j - 1) {
                    let v = h[lf].saturating_add(GAP);
                    if v > best.0 {
                        best = (v, Op::Drop);
                    }
                }
            }
            h[k] = best.0;
            tr[k] = best.1;
        }
    }
    // traceback: pair[i] = transcript index for script word i
    let mut pair: Vec<Option<usize>> = vec![None; n];
    let (mut i, mut j) = (n, m.min(hi(n)));
    while i > 0 || j > 0 {
        let op = if i == 0 { Op::Drop } else { at(i, j).map(|k| tr[k]).unwrap_or(Op::Insert) };
        match op {
            Op::Pair if i > 0 && j > 0 => {
                pair[i - 1] = Some(j - 1);
                i -= 1;
                j -= 1;
            }
            Op::Insert | Op::Pair if i > 0 => i -= 1,
            _ => j -= 1,
        }
    }
    let used: usize = pair.iter().flatten().count();
    stats.dropped = m - used;
    let mut words: Vec<Word> = Vec::with_capacity(n);
    for (i, p) in pair.iter().enumerate() {
        match p {
            Some(j) => {
                let w = tw[*j];
                let sim = similarity(&sn[i], &tn[*j]);
                if sn[i] == tn[*j] {
                    stats.exact += 1;
                } else if same_word(&sn[i], &tn[*j]) {
                    stats.close += 1;
                } else {
                    stats.substituted += 1;
                }
                words.push(Word {
                    w: sw[i].clone(),
                    start: w.start,
                    end: w.end,
                    conf: if sn[i] == tn[*j] { w.conf } else { w.conf.min(sim.max(0.3)) },
                    speaker: w.speaker.clone(),
                });
            }
            None => {
                stats.inserted += 1;
                words.push(Word { w: sw[i].clone(), start: f64::NAN, end: f64::NAN, conf: 0.5, speaker: None });
            }
        }
    }
    interpolate(&mut words);
    let mut out = Transcript { words, segments: vec![], ..t.clone() };
    out.rebuild_segments();
    (out, stats)
}

/// Give unpaired words (NaN times) times between their paired neighbours.
fn interpolate(words: &mut [Word]) {
    let n = words.len();
    let mut i = 0;
    while i < n {
        if !words[i].start.is_nan() {
            i += 1;
            continue;
        }
        let a = i;
        while i < n && words[i].start.is_nan() {
            i += 1;
        }
        let b = i; // first paired word after the run (or n)
        let k = (b - a) as f64;
        let prev = a.checked_sub(1).map(|p| (words[p].start, words[p].end));
        let next = (b < n).then(|| words[b].start);
        let per = 0.25; // seconds per word when there is no room
        let (s, e) = match (prev, next) {
            (Some((_, pe)), Some(ns)) if ns - pe >= 0.08 * k => (pe, ns),
            // no gap: share the previous word's span with the run
            (Some((ps, pe)), Some(_)) => {
                let share = (pe - ps) / (k + 1.0);
                words[a - 1].end = ps + share;
                (ps + share, pe)
            }
            (Some((_, pe)), None) => (pe, pe + per * k),
            (None, Some(ns)) => ((ns - per * k).max(0.0), ns),
            (None, None) => (0.0, per * k),
        };
        let step = (e - s) / k;
        for (q, w) in words[a..b].iter_mut().enumerate() {
            w.start = s + step * q as f64;
            w.end = s + step * (q + 1) as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Transcript {
        let words = s
            .split_whitespace()
            .enumerate()
            .map(|(i, w)| Word { w: w.into(), start: i as f64, end: i as f64 + 0.8, conf: 0.9, speaker: None })
            .collect();
        Transcript { words, ..Default::default() }
    }

    #[test]
    fn script_words_drop_audio_tags_and_dashes() {
        assert_eq!(
            script_words("[warm, upbeat] Meet Glance — a dashboard. [confident] Go!"),
            ["Meet", "Glance", "a", "dashboard.", "Go!"]
        );
    }

    #[test]
    fn script_spelling_with_transcript_timing() {
        let tr = t("seven pains each one keystroke away written in rust");
        let (out, st) = align(&tr, "Seven panes, each one keystroke away. Written in Rust.");
        let ws: Vec<&str> = out.words.iter().map(|w| w.w.as_str()).collect();
        assert_eq!(ws, ["Seven", "panes,", "each", "one", "keystroke", "away.", "Written", "in", "Rust."]);
        // timing comes from the transcript word each script word is paired with
        assert_eq!((out.words[1].start, out.words[1].end), (1.0, 1.8));
        assert_eq!((out.words[8].start, out.words[8].end), (8.0, 8.8));
        assert_eq!((st.exact, st.close, st.inserted, st.dropped), (8, 1, 0, 0));
        assert_eq!(out.segments.len(), 2, "segments rebuilt from the script's sentences");
    }

    #[test]
    fn misheard_word_takes_the_heard_words_time() {
        // the recogniser heard "um" where the script says "quick": one substitution
        let tr = t("the um brown fox jumps");
        let (out, st) = align(&tr, "The quick brown fox jumps.");
        let ws: Vec<&str> = out.words.iter().map(|w| w.w.as_str()).collect();
        assert_eq!(ws, ["The", "quick", "brown", "fox", "jumps."]);
        assert_eq!((st.substituted, st.inserted, st.dropped), (1, 0, 0));
        assert_eq!((out.words[1].start, out.words[1].end), (1.0, 1.8));
    }

    #[test]
    fn missing_and_extra_words() {
        // the recogniser missed "quick" and added "really" (both absent on the other side)
        let tr = t("the brown fox jumps really high");
        let (out, st) = align(&tr, "The quick brown fox jumps high.");
        let ws: Vec<&str> = out.words.iter().map(|w| w.w.as_str()).collect();
        assert_eq!(ws, ["The", "quick", "brown", "fox", "jumps", "high."]);
        assert_eq!((st.inserted, st.dropped), (1, 1));
        // "quick" is squeezed in after "The" (0–0.8) and before "brown" (1–1.8)
        assert!(out.words[1].start >= 0.0 && out.words[1].end <= 1.0 + 1e-9, "{:?}", out.words[1]);
        assert_eq!(out.words[5].start, 5.0, "\"high\" keeps its own time");
        for w in out.words.windows(2) {
            assert!(w[1].start >= w[0].start, "times stay ordered");
        }
    }

    #[test]
    fn no_gap_shares_the_previous_word() {
        let mut tr = t("twenty hour clock");
        tr.words[1].start = 0.8; // "hour" starts right after "twenty"
        let (out, _) = align(&tr, "twenty four hour clock");
        assert!(out.words[1].start < out.words[1].end && out.words[1].end <= out.words[2].start + 1e-9);
        assert!(out.words[0].end <= out.words[1].start + 1e-9);
    }

    #[test]
    fn wrong_script_has_a_low_match_ratio() {
        let tr = t("completely different audio about something else entirely here");
        let (_, st) = align(&tr, "Meet Glance, a terminal dashboard you can leave open all day.");
        assert!(st.match_ratio() < 0.3, "{st:?}");
    }
}
