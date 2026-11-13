//! Word normalisation and phrase search over transcript words (used by cues and script
//! alignment).

use crate::model::Word;

/// Lowercase, keep letters/digits/apostrophes; `"Panes,"` → `"panes"`, `"24-hour"` → `"24hour"`.
pub fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || *c == '\'').flat_map(char::to_lowercase).collect()
}

/// Normalised, non-empty tokens of a text.
pub fn tokens(s: &str) -> Vec<String> {
    s.split_whitespace().map(norm).filter(|t| !t.is_empty()).collect()
}

/// Levenshtein distance over chars.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Similarity 0..1 of two normalised tokens (1 = equal).
pub fn similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let n = a.chars().count().max(b.chars().count());
    if n == 0 {
        return 1.0;
    }
    1.0 - levenshtein(a, b) as f64 / n as f64
}

/// Two tokens close enough to be the same spoken word ("panes"/"pains", "rust"/"rust's").
pub fn same_word(a: &str, b: &str) -> bool {
    similarity(a, b) >= 0.6
}

/// A located phrase.
#[derive(Debug, Clone, PartialEq)]
pub struct PhraseMatch {
    /// Index of the first and last matched word.
    pub first: usize,
    pub last: usize,
    pub start: f64,
    pub end: f64,
    /// Mean token similarity (1.0 = exact).
    pub score: f64,
}

impl PhraseMatch {
    pub fn exact(&self) -> bool {
        self.score >= 1.0 - 1e-9
    }
}

/// Why a phrase was not found, with the closest windows for the error hint.
#[derive(Debug, Clone, PartialEq)]
pub struct NotFound {
    pub found: usize,
    pub nearest: Vec<String>,
}

/// Find the `occurrence`-th (1-based) occurrence of `phrase` in `words`. Exact token matches
/// win; when there are none, windows whose tokens all match fuzzily (similarity ≥ 0.6, mean
/// ≥ 0.8) are accepted, which absorbs transcription slips like "pains" for "panes".
pub fn find_phrase(words: &[Word], phrase: &str, occurrence: usize) -> Result<PhraseMatch, NotFound> {
    let want = tokens(phrase);
    let have: Vec<String> = words.iter().map(|w| norm(&w.w)).collect();
    // words that normalise to nothing (pure punctuation) are skipped in the search
    let idx: Vec<usize> = (0..have.len()).filter(|&i| !have[i].is_empty()).collect();
    let n = want.len();
    let occurrence = occurrence.max(1);
    if n == 0 || idx.len() < n {
        return Err(NotFound { found: 0, nearest: vec![] });
    }
    let mut exact = vec![];
    let mut fuzzy = vec![];
    let mut scored: Vec<(f64, usize)> = vec![];
    for s in 0..=idx.len() - n {
        let sims: Vec<f64> = (0..n).map(|k| similarity(&want[k], &have[idx[s + k]])).collect();
        let mean = sims.iter().sum::<f64>() / n as f64;
        scored.push((mean, s));
        if sims.iter().all(|&x| x >= 1.0) {
            exact.push((s, mean));
        } else if mean >= 0.8 && sims.iter().all(|&x| x >= 0.6) {
            fuzzy.push((s, mean));
        }
    }
    let pick = if exact.is_empty() { &fuzzy } else { &exact };
    if let Some(&(s, score)) = pick.get(occurrence - 1) {
        let (first, last) = (idx[s], idx[s + n - 1]);
        return Ok(PhraseMatch { first, last, start: words[first].start, end: words[last].end, score });
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let nearest = scored
        .iter()
        .take(3)
        .filter(|(m, _)| *m > 0.3)
        .map(|(_, s)| {
            let (first, last) = (idx[*s], idx[s + n - 1]);
            let text = words[first..=last].iter().map(|w| w.w.as_str()).collect::<Vec<_>>().join(" ");
            format!("\"{text}\" at {:.2}s", words[first].start)
        })
        .collect();
    Err(NotFound { found: pick.len(), nearest })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<Word> {
        s.split_whitespace()
            .enumerate()
            .map(|(i, w)| Word { w: w.into(), start: i as f64, end: i as f64 + 0.5, conf: 1.0, speaker: None })
            .collect()
    }

    #[test]
    fn exact_occurrences_and_punctuation() {
        let w = words("Meet Glance. World clocks keep cities. The world, clocks again");
        let m = find_phrase(&w, "world clocks", 1).unwrap();
        assert_eq!((m.first, m.last, m.start, m.end), (2, 3, 2.0, 3.5));
        assert!(m.exact());
        let m2 = find_phrase(&w, "World Clocks", 2).unwrap();
        assert_eq!(m2.first, 7);
        let e = find_phrase(&w, "world clocks", 3).unwrap_err();
        assert_eq!(e.found, 2);
    }

    #[test]
    fn fuzzy_match_absorbs_transcription_slips() {
        let w = words("Seven pains, each one keystroke away.");
        let m = find_phrase(&w, "seven panes", 1).unwrap();
        assert_eq!((m.first, m.last), (0, 1));
        assert!(!m.exact());
        let e = find_phrase(&w, "calendar view", 1).unwrap_err();
        assert_eq!(e.found, 0);
    }

    #[test]
    fn not_found_lists_nearest() {
        let w = words("the calendar jumps from a month to the whole year");
        let e = find_phrase(&w, "the calender walked", 1).unwrap_err();
        assert!(e.nearest.iter().any(|n| n.contains("calendar")), "{:?}", e.nearest);
    }
}
