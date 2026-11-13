//! Internal transcript format: `{asset, lang, model, words:[{w, start, end, conf, speaker?}], segments}`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Word {
    pub w: String,
    pub start: f64,
    pub end: f64,
    #[serde(default = "one")]
    pub conf: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}
fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Transcript {
    #[serde(default)]
    pub asset: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub model: String,
    pub words: Vec<Word>,
    #[serde(default)]
    pub segments: Vec<Segment>,
}

impl Transcript {
    /// Rebuild segments from words (split at sentence punctuation or pauses > 0.6 s).
    pub fn rebuild_segments(&mut self) {
        let mut segs = vec![];
        let mut cur: Vec<&Word> = vec![];
        for (i, w) in self.words.iter().enumerate() {
            cur.push(w);
            let end_sent = w.w.ends_with(['.', '?', '!']);
            let pause = self.words.get(i + 1).map(|n| n.start - w.end > 0.6).unwrap_or(true);
            if end_sent || pause {
                segs.push(Segment {
                    start: cur[0].start,
                    end: w.end,
                    text: cur.iter().map(|x| x.w.as_str()).collect::<Vec<_>>().join(" "),
                    speaker: cur[0].speaker.clone(),
                });
                cur.clear();
            }
        }
        self.segments = segs;
    }

    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.w.as_str()).collect::<Vec<_>>().join(" ")
    }

    /// Apply a corrections map (case-insensitive, multi-word phrases allowed). The replacement
    /// spans the timing of the matched words.
    pub fn apply_corrections(&mut self, map: &BTreeMap<String, String>) {
        if map.is_empty() {
            return;
        }
        let norm = |s: &str| s.to_lowercase().trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_string();
        let mut entries: Vec<(Vec<String>, &String)> =
            map.iter().map(|(k, v)| (k.split_whitespace().map(norm).collect(), v)).collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.0.len()));
        let mut out: Vec<Word> = vec![];
        let mut i = 0;
        'outer: while i < self.words.len() {
            for (pat, rep) in &entries {
                let n = pat.len();
                if n > 0 && i + n <= self.words.len() && (0..n).all(|k| norm(&self.words[i + k].w) == pat[k]) {
                    // keep trailing punctuation of the last word
                    let last = &self.words[i + n - 1].w;
                    let punct: String = last
                        .chars()
                        .rev()
                        .take_while(|c| !c.is_alphanumeric())
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    out.push(Word {
                        w: format!("{rep}{punct}"),
                        start: self.words[i].start,
                        end: self.words[i + n - 1].end,
                        conf: self.words[i..i + n].iter().map(|w| w.conf).fold(1.0, f64::min),
                        speaker: self.words[i].speaker.clone(),
                    });
                    i += n;
                    continue 'outer;
                }
            }
            out.push(self.words[i].clone());
            i += 1;
        }
        self.words = out;
        self.rebuild_segments();
    }
}

/// Word error rate (Levenshtein over normalized words).
pub fn wer(reference: &str, hypothesis: &str) -> f64 {
    let n = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(|w| w.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect::<String>())
            .filter(|w| !w.is_empty())
            .collect()
    };
    let (r, h) = (n(reference), n(hypothesis));
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let mut d: Vec<Vec<usize>> = vec![vec![0; h.len() + 1]; r.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, v) in d[0].iter_mut().enumerate() {
        *v = j;
    }
    for i in 1..=r.len() {
        for j in 1..=h.len() {
            let c = if r[i - 1] == h[j - 1] { 0 } else { 1 };
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + c);
        }
    }
    d[r.len()][h.len()] as f64 / r.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    fn w(s: &str, a: f64, b: f64) -> Word {
        Word { w: s.into(), start: a, end: b, conf: 1.0, speaker: None }
    }
    #[test]
    fn corrections_multiword() {
        let mut t = Transcript {
            words: vec![w("the", 0.0, 0.2), w("chefs", 0.2, 0.5), w("knife.", 0.5, 0.9), w("ok", 1.0, 1.2)],
            ..Default::default()
        };
        let mut m = BTreeMap::new();
        m.insert("chefs knife".to_string(), "chef's knife".to_string());
        t.apply_corrections(&m);
        assert_eq!(t.text(), "the chef's knife. ok");
        assert_eq!(t.words[1].start, 0.2);
        assert_eq!(t.words[1].end, 0.9);
        assert_eq!(t.segments.len(), 2);
    }
    #[test]
    fn wer_basic() {
        assert_eq!(wer("the quick brown fox", "the quick brown fox"), 0.0);
        assert_eq!(wer("the quick brown fox", "the quick fox"), 0.25);
        assert!((wer("a b c d", "a x c d e") - 0.5).abs() < 1e-9);
    }
}
