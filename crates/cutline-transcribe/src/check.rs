//! Script check: compare what the recogniser heard with the script the voice was generated
//! from, and report the places where they differ. Captions use the script's words, so a word
//! the voice got wrong ("Dolos" for "Todos") is otherwise invisible until someone listens.
//!
//! Both sides are reduced to spoken units (lowercase letters and digits; numbers spelled
//! out; `[audio tags]` dropped), the brand dictionary is applied to the heard side, and the
//! two are aligned word by word (edit distance). A run of differing words is a mismatch
//! unless the two sides are the same letters joined differently ("JustNotes"/"Just Notes",
//! "to-dos"/"to dos"), nearly the same letters, or sound alike ("panes"/"pains").

use crate::align::script_words;
use crate::model::{Transcript, Word};
use crate::text::similarity;
use serde::Serialize;
use std::collections::BTreeMap;

/// Default for [`CheckOptions::similarity`].
pub const DEFAULT_SIMILARITY: f64 = 0.8;
/// Similarity from which words that sound alike pass.
pub const SOUNDS_ALIKE_FLOOR: f64 = 0.6;

#[derive(Debug, Clone, PartialEq)]
pub struct CheckOptions {
    /// Letter similarity (0–1) from which a difference passes. `1`: only equal letters pass.
    pub similarity: f64,
    /// Smallest difference to report, in words on its longer side.
    pub min_span_words: usize,
    /// Script words or phrases never reported.
    pub ignore: Vec<String>,
    /// Heard phrase → intended spelling (the brand `dictionary` and caption `corrections`).
    pub dictionary: BTreeMap<String, String>,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self { similarity: DEFAULT_SIMILARITY, min_span_words: 1, ignore: vec![], dictionary: BTreeMap::new() }
    }
}

/// A place where the audio does not say what the script says.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mismatch {
    /// The script's words (empty: the audio has words the script does not).
    pub script: String,
    /// The words heard instead (empty: the script's words were not heard).
    pub heard: String,
    /// Where it is in the audio (seconds in the asset).
    pub start: f64,
    pub end: f64,
    /// Index of the first script word involved (or of the next one, for extra heard words).
    pub script_index: usize,
    /// The script around it, with the words in question in brackets.
    pub context: String,
    /// What was heard around it, with the words in question in brackets.
    pub heard_context: String,
}

/// Lowercase letters and digits only (apostrophes and hyphens go: "to-do's" → "todos").
fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

/// A whole number as English words ("24" → twenty four).
fn number_words(n: u64, out: &mut Vec<String>) {
    match n {
        0..=19 => out.push(ONES[n as usize].into()),
        20..=99 => {
            out.push(TENS[(n / 10) as usize].into());
            if n % 10 > 0 {
                out.push(ONES[(n % 10) as usize].into());
            }
        }
        100..=999 => {
            out.push(ONES[(n / 100) as usize].into());
            out.push("hundred".into());
            if n % 100 > 0 {
                number_words(n % 100, out);
            }
        }
        _ => {
            let (unit, name) = if n >= 1_000_000_000 {
                (1_000_000_000, "billion")
            } else if n >= 1_000_000 {
                (1_000_000, "million")
            } else {
                (1_000, "thousand")
            };
            number_words(n / unit, out);
            out.push(name.into());
            if n % unit > 0 {
                number_words(n % unit, out);
            }
        }
    }
}

/// The spoken units of one written token: "24-hour" → twenty, four, hour; "1.8" → one, point,
/// eight; "50%" → fifty, percent; "Just Notes'" → justnotes. A token without digits is one
/// unit (its letters), so hyphenated and joined spellings compare by letters.
fn units(token: &str) -> Vec<String> {
    // "One-oh-eight", "four-oh-four": digits read one by one, written as a number
    let parts: Vec<String> = token.split('-').map(letters).filter(|p| !p.is_empty()).collect();
    if parts.len() >= 2 && token.contains('-') && parts.iter().all(|p| spoken_digit(p).is_some()) {
        let digits: String = parts.iter().filter_map(|p| spoken_digit(p)).collect();
        return units(&digits);
    }
    if !token.chars().any(|c| c.is_ascii_digit()) {
        let mut l = letters(token);
        if token.contains('&') && l.is_empty() {
            l = "and".into();
        }
        return if l.is_empty() { vec![] } else { vec![l] };
    }
    let chars: Vec<char> = token.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let mut digits = String::new();
            while i < chars.len() && (chars[i].is_ascii_digit() || is_group_comma(&chars, i)) {
                if chars[i].is_ascii_digit() {
                    digits.push(chars[i]);
                }
                i += 1;
            }
            match digits.parse::<u64>() {
                Ok(n) if digits.len() <= 12 => number_words(n, &mut out),
                _ => out.extend(digits.chars().map(|d| ONES[d.to_digit(10).unwrap_or(0) as usize].to_string())),
            }
            // a decimal point between digits is spoken; digits after it one by one
            if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                out.push("point".into());
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    out.push(ONES[chars[i].to_digit(10).unwrap_or(0) as usize].into());
                    i += 1;
                }
            }
        } else if c.is_alphanumeric() {
            let mut word = String::new();
            while i < chars.len() && chars[i].is_alphanumeric() && !chars[i].is_ascii_digit() {
                word.extend(chars[i].to_lowercase());
                i += 1;
            }
            out.push(word);
        } else {
            if c == '%' {
                out.push("percent".into());
            } else if c == '&' {
                out.push("and".into());
            }
            i += 1;
        }
    }
    out
}

/// A comma grouping thousands: digits on both sides ("1,000").
fn is_group_comma(chars: &[char], i: usize) -> bool {
    chars[i] == ','
        && i > 0
        && chars[i - 1].is_ascii_digit()
        && chars.get(i + 1..i + 4).is_some_and(|n| n.len() == 3 && n.iter().all(|c| c.is_ascii_digit()))
}

/// A digit read on its own: "oh" and "o" are zero, as in "one-oh-eight".
fn spoken_digit(u: &str) -> Option<char> {
    match u {
        "oh" | "o" => Some('0'),
        _ => ONES[..10].iter().position(|w| *w == u).map(|d| char::from(b'0' + d as u8)),
    }
}

fn is_number_word(u: &str) -> bool {
    ONES.contains(&u)
        || TENS.contains(&u) && !u.is_empty()
        || matches!(u, "hundred" | "thousand" | "million" | "billion")
}

/// The same cleanup for the script's and the heard spoken units, so the two spellings of one
/// spoken thing compare equal (TASK-honest-cedar-A73E): digits read one by one ("one oh eight")
/// become the number ("108" → one hundred eight); "and" inside a number goes ("one hundred and
/// eight"); single letters in a row are joined ("a i" → "ai"); "dot" between two words joins
/// them, as in a domain read aloud ("thinkask dot a i" → "thinkaskai", which is how
/// "thinkask.ai" is written). Each unit keeps the written word it came from.
fn cleanup(u: Vec<String>, owner: Vec<usize>) -> (Vec<String>, Vec<usize>) {
    let mut out: Vec<(String, usize)> = Vec::with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        // digits read one by one: at least two, with an "oh" among them
        let run = u[i..].iter().take_while(|x| spoken_digit(x).is_some()).count();
        if run >= 2 && u[i..i + run].iter().any(|x| x == "oh" || x == "o") {
            let digits: String = u[i..i + run].iter().filter_map(|x| spoken_digit(x)).collect();
            out.extend(units(&digits).into_iter().map(|x| (x, owner[i])));
            i += run;
            continue;
        }
        // "hundred and eight": the "and" is part of the number
        if u[i] == "and"
            && i > 0
            && matches!(u[i - 1].as_str(), "hundred" | "thousand" | "million" | "billion")
            && u.get(i + 1).is_some_and(|n| is_number_word(n))
        {
            i += 1;
            continue;
        }
        out.push((u[i].clone(), owner[i]));
        i += 1;
    }
    // single letters in a row: "a i" → "ai"
    let single = |x: &str| x.chars().count() == 1 && x.chars().all(|c| c.is_alphabetic());
    let mut joined: Vec<(String, usize)> = Vec::with_capacity(out.len());
    let mut k = 0;
    while k < out.len() {
        let run = out[k..].iter().take_while(|(x, _)| single(x)).count();
        if run >= 2 {
            joined.push((out[k..k + run].iter().map(|(x, _)| x.as_str()).collect(), out[k].1));
            k += run;
        } else {
            joined.push(out[k].clone());
            k += 1;
        }
    }
    // "dot" between two words: "thinkask dot ai" → "thinkaskai"
    let mut res: Vec<(String, usize)> = Vec::with_capacity(joined.len());
    let mut k = 0;
    while k < joined.len() {
        if joined[k].0 == "dot" && !res.is_empty() && k + 1 < joined.len() {
            let next = joined[k + 1].0.clone();
            res.last_mut().unwrap().0.push_str(&next);
            k += 2;
            continue;
        }
        res.push(joined[k].clone());
        k += 1;
    }
    res.into_iter().unzip()
}

/// Spoken units of written words, after [`cleanup`], each pointing back at its word.
fn spoken(tokens: &mut dyn Iterator<Item = &str>) -> (Vec<String>, Vec<usize>) {
    let (mut u, mut owner) = (vec![], vec![]);
    for (k, tok) in tokens.enumerate() {
        for x in tok.split_whitespace().flat_map(units) {
            u.push(x);
            owner.push(k);
        }
    }
    cleanup(u, owner)
}

/// How much of a script was heard, comparing spoken units after the same cleanup on both sides.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coverage {
    /// Spoken units in the script.
    pub script_units: usize,
    /// Script units heard as the same or a tolerable spelling.
    pub matched: usize,
    /// Script units with no heard counterpart, or heard as something else.
    pub missing: usize,
    /// Heard units that aren't in the script.
    pub extra: usize,
}

impl Coverage {
    pub fn match_ratio(&self) -> f64 {
        if self.script_units == 0 {
            1.0
        } else {
            self.matched as f64 / self.script_units as f64
        }
    }
}

/// How much of `script` was heard in `t` (the brand dictionary applied to the heard side).
pub fn coverage(t: &Transcript, script: &str, opts: &CheckOptions) -> Coverage {
    let sw = script_words(script);
    let mut heard = t.clone();
    heard.apply_corrections(&opts.dictionary);
    let (su, _) = spoken(&mut sw.iter().map(|s| s.as_str()));
    let (hu, _) = spoken(&mut heard.words.iter().map(|w| w.w.as_str()));
    let ops = align_units(&su, &hu);
    let mut c = Coverage { script_units: su.len(), matched: 0, missing: 0, extra: 0 };
    for op in &ops {
        match op {
            Op::Pair(i, j) if tolerable(&su[*i], &hu[*j], opts.similarity) => c.matched += 1,
            Op::Pair(..) | Op::Missing(_) => c.missing += 1,
            Op::Extra(_) => c.extra += 1,
        }
    }
    c
}

/// A rough key for how a string of letters sounds: consonant classes as in Soundex (b f p v ·
/// c g j k q s x z · d t · l · m n · r), vowels dropped, repeats merged. Equal keys: the two
/// may be the same spoken word spelled differently ("panes"/"pains", "Claude"/"Cloud").
pub fn sound_key(s: &str) -> String {
    let mut out = String::new();
    let mut last = '\0';
    for c in s.chars().flat_map(char::to_lowercase) {
        let k = match c {
            'b' | 'f' | 'p' | 'v' => '1',
            'c' | 'g' | 'j' | 'k' | 'q' | 's' | 'x' | 'z' => '2',
            'd' | 't' => '3',
            'l' => '4',
            'm' | 'n' => '5',
            'r' => '6',
            // h and w do not separate equal consonants; vowels do
            'h' | 'w' => continue,
            'a' | 'e' | 'i' | 'o' | 'u' | 'y' => {
                if out.is_empty() {
                    out.push('a');
                }
                last = '\0';
                continue;
            }
            other if other.is_alphanumeric() => other,
            _ => continue,
        };
        if k != last {
            out.push(k);
        }
        last = k;
    }
    out
}

/// Whether script letters `a` and heard letters `b` (both joined, normalised) are the same
/// words as far as a recogniser can tell.
pub fn tolerable(a: &str, b: &str, min_similarity: f64) -> bool {
    if a == b {
        return true;
    }
    if a.is_empty() || b.is_empty() || min_similarity >= 1.0 {
        return false;
    }
    let sim = similarity(a, b);
    sim >= min_similarity || (sim >= SOUNDS_ALIKE_FLOOR.min(min_similarity) && sound_key(a) == sound_key(b))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    /// script unit i with heard unit j
    Pair(usize, usize),
    /// script unit i with nothing heard
    Missing(usize),
    /// heard unit j the script does not have
    Extra(usize),
}

/// Word-level edit-distance alignment of script units `s` and heard units `h`.
fn align_units(s: &[String], h: &[String]) -> Vec<Op> {
    let (n, m) = (s.len(), h.len());
    const GAP: f64 = 0.7;
    let sub = |i: usize, j: usize| -> f64 {
        if s[i] == h[j] {
            0.0
        } else {
            1.0 - 0.6 * similarity(&s[i], &h[j])
        }
    };
    let w = m + 1;
    let mut cost = vec![0.0f64; (n + 1) * w];
    for (j, c) in cost.iter_mut().enumerate().take(m + 1) {
        *c = j as f64 * GAP;
    }
    for i in 1..=n {
        cost[i * w] = i as f64 * GAP;
        for j in 1..=m {
            let d = cost[(i - 1) * w + j - 1] + sub(i - 1, j - 1);
            let up = cost[(i - 1) * w + j] + GAP;
            let left = cost[i * w + j - 1] + GAP;
            cost[i * w + j] = d.min(up).min(left);
        }
    }
    let mut ops = vec![];
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        let here = cost[i * w + j];
        if i > 0 && j > 0 && (here - (cost[(i - 1) * w + j - 1] + sub(i - 1, j - 1))).abs() < 1e-9 {
            ops.push(Op::Pair(i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if i > 0 && (j == 0 || (here - (cost[(i - 1) * w + j] + GAP)).abs() < 1e-9) {
            ops.push(Op::Missing(i - 1));
            i -= 1;
        } else {
            ops.push(Op::Extra(j - 1));
            j -= 1;
        }
    }
    ops.reverse();
    ops
}

/// Compare the transcript `t` of a narration with its `script`. Returns the differences in
/// the order they are spoken.
pub fn check(t: &Transcript, script: &str, opts: &CheckOptions) -> Vec<Mismatch> {
    let sw = script_words(script);
    let mut heard = t.clone();
    heard.apply_corrections(&opts.dictionary);
    let hw: Vec<&Word> = heard.words.iter().collect();
    // spoken units, each pointing back at its written word
    let (su, s_owner) = spoken(&mut sw.iter().map(|s| s.as_str()));
    let (hu, h_owner) = spoken(&mut hw.iter().map(|w| w.w.as_str()));
    if su.is_empty() {
        return vec![];
    }
    let ops = align_units(&su, &hu);
    let ignore: Vec<String> = opts.ignore.iter().map(|p| letters(p)).filter(|p| !p.is_empty()).collect();
    let same = |op: &Op| matches!(op, Op::Pair(i, j) if su[*i] == hu[*j]);
    let ok = |op: &Op| matches!(op, Op::Pair(i, j) if tolerable(&su[*i], &hu[*j], opts.similarity));
    let mut out = vec![];
    let mut k = 0;
    while k < ops.len() {
        if same(&ops[k]) {
            k += 1;
            continue;
        }
        // a run of words that are not letter for letter the same
        let from = k;
        while k < ops.len() && !same(&ops[k]) {
            k += 1;
        }
        let (from, to) = (from, k);
        if ops[from..to].iter().all(ok) {
            continue;
        }
        // near-matches at its ends are not part of the problem, unless what is left would be
        // only missing or only extra words (then they say what was heard instead)
        let (mut a, mut b) = (from, to);
        while a < b && ok(&ops[a]) {
            a += 1;
        }
        while b > a && ok(&ops[b - 1]) {
            b -= 1;
        }
        let has = |script: bool| {
            ops[a..b].iter().any(|o| match o {
                Op::Pair(..) => true,
                Op::Missing(_) => script,
                Op::Extra(_) => !script,
            })
        };
        let (from, k_end) = if has(true) && has(false) { (a, b) } else { (from, to) };
        let run = &ops[from..k_end];
        let si: Vec<usize> = run
            .iter()
            .filter_map(|o| match o {
                Op::Pair(i, _) | Op::Missing(i) => Some(*i),
                _ => None,
            })
            .collect();
        let hi: Vec<usize> = run
            .iter()
            .filter_map(|o| match o {
                Op::Pair(_, j) | Op::Extra(j) => Some(*j),
                _ => None,
            })
            .collect();
        let joined_s: String = si.iter().map(|i| su[*i].as_str()).collect();
        let joined_h: String = hi.iter().map(|j| hu[*j].as_str()).collect();
        // the same letters split differently, or near enough
        if tolerable(&joined_s, &joined_h, opts.similarity) {
            continue;
        }
        // written words the run touches
        let mut s_words: Vec<usize> = si.iter().map(|i| s_owner[*i]).collect();
        s_words.dedup();
        let mut h_words: Vec<usize> = hi.iter().map(|j| h_owner[*j]).collect();
        h_words.dedup();
        if s_words.len().max(h_words.len()) < opts.min_span_words.max(1) {
            continue;
        }
        if !joined_s.is_empty()
            && (ignore.contains(&joined_s) || s_words.iter().all(|w| ignore.contains(&letters(&sw[*w]))))
        {
            continue;
        }
        // where: the heard words, or the gap between the neighbours of words that are missing
        let prev_h = run_edge(&ops[..from], true).map(|j| h_owner[j]);
        let next_h = run_edge(&ops[k_end..], false).map(|j| h_owner[j]);
        let (start, end) = match (h_words.first(), h_words.last()) {
            (Some(a), Some(b)) => (hw[*a].start, hw[*b].end),
            _ => {
                let s = prev_h.map(|p| hw[p].end).unwrap_or(0.0);
                (s, next_h.map(|p| hw[p].start).unwrap_or(s).max(s))
            }
        };
        let script_index = s_words.first().copied().unwrap_or_else(|| {
            // extra heard words: they sit before the next script word
            ops[k_end..]
                .iter()
                .find_map(|o| match o {
                    Op::Pair(i, _) | Op::Missing(i) => Some(s_owner[*i]),
                    _ => None,
                })
                .unwrap_or(sw.len())
        });
        let (c0, c1) = match (s_words.first(), s_words.last()) {
            (Some(a), Some(b)) => (*a, *b + 1),
            _ => (script_index, script_index),
        };
        let lead = sw[c0.saturating_sub(3)..c0].join(" ");
        let trail = sw[c1..(c1 + 3).min(sw.len())].join(" ");
        let mid = sw[c0..c1].join(" ");
        let context =
            [lead, format!("[{mid}]"), trail].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        let heard_text = h_words.iter().map(|w| hw[*w].w.as_str()).collect::<Vec<_>>().join(" ");
        let (h0, h1) = match (h_words.first(), h_words.last()) {
            (Some(a), Some(b)) => (*a, *b + 1),
            _ => {
                let at = next_h.unwrap_or(hw.len());
                (at, at)
            }
        };
        let word = |k: usize| hw[k].w.as_str();
        let heard_context = [
            (h0.saturating_sub(3)..h0).map(word).collect::<Vec<_>>().join(" "),
            format!("[{heard_text}]"),
            (h1..(h1 + 3).min(hw.len())).map(word).collect::<Vec<_>>().join(" "),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
        out.push(Mismatch { heard_context, script: mid, heard: heard_text, start, end, script_index, context });
    }
    out
}

/// The heard unit nearest to a run: the last one in `ops` (`last`) or the first.
fn run_edge(ops: &[Op], last: bool) -> Option<usize> {
    let pick = |o: &Op| match o {
        Op::Pair(_, j) | Op::Extra(j) => Some(*j),
        _ => None,
    };
    if last {
        ops.iter().rev().find_map(pick)
    } else {
        ops.iter().find_map(pick)
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

    fn run(heard: &str, script: &str) -> Vec<(String, String)> {
        check(&t(heard), script, &CheckOptions::default()).into_iter().map(|m| (m.script, m.heard)).collect()
    }

    fn pair(s: &str, h: &str) -> (String, String) {
        (s.to_string(), h.to_string())
    }

    #[test]
    fn todos_heard_as_dolos() {
        let m = check(
            &t("Dolos live right in your notes."),
            "[warmly] To-dos live right in your notes.",
            &CheckOptions::default(),
        );
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!((m[0].script.as_str(), m[0].heard.as_str()), ("To-dos", "Dolos"));
        assert_eq!((m[0].start, m[0].end, m[0].script_index), (0.0, 0.8, 0));
        assert_eq!(m[0].context, "[To-dos] live right in");
        assert_eq!(m[0].heard_context, "[Dolos] live right in");
    }

    #[test]
    fn todos_heard_as_several_words() {
        // the second real case: "Todos" came out as "it to do"
        let m = check(
            &t("Extract it to do into its own note."),
            "Extract Todos into its own note.",
            &CheckOptions::default(),
        );
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!((m[0].script.as_str(), m[0].heard.as_str()), ("Todos", "it to do"));
        assert_eq!((m[0].start, m[0].end), (1.0, 3.8));
        // even with the brand dictionary joining "to do"
        let dict: BTreeMap<String, String> = [("to do".to_string(), "todo".to_string())].into();
        let m = check(
            &t("Extract it to do into its own note."),
            "Extract Todos into its own note.",
            &CheckOptions { dictionary: dict, ..Default::default() },
        );
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!((m[0].script.as_str(), m[0].heard.as_str()), ("Todos", "it todo"));
    }

    #[test]
    fn joined_split_and_hyphenated_words_match() {
        assert_eq!(run("Meet JustNotes today", "Meet Just Notes today."), []);
        assert_eq!(run("Meet Just Notes today", "Meet JustNotes today."), []);
        assert_eq!(run("your to dos live here", "Your to-dos live here."), []);
        assert_eq!(run("your todos live here", "Your to-dos live here."), []);
        assert_eq!(run("your to-do's live here", "Your todos live here."), []);
        assert_eq!(run("a key stroke away", "A keystroke away"), []);
    }

    #[test]
    fn case_punctuation_tags_and_dashes_are_ignored() {
        assert_eq!(run("seven panes each one keystroke away", "[pause] Seven panes — each one, keystroke away!"), []);
        assert_eq!(run("It's Rust's fault", "its rusts fault"), []);
    }

    #[test]
    fn numbers_compare_as_spoken() {
        assert_eq!(run("a twenty four hour clock", "A 24-hour clock"), []);
        assert_eq!(run("a 24 hour clock", "A twenty-four hour clock"), []);
        assert_eq!(run("version one point eight is out", "Version 1.8 is out"), []);
        assert_eq!(run("up fifty percent", "Up 50%"), []);
        assert_eq!(run("over one thousand two hundred notes", "Over 1,200 notes"), []);
        assert_eq!(run("a twelve hour clock", "A 24-hour clock"), [pair("24-hour", "twelve")]);
        let mut w = vec![];
        number_words(1_234_567, &mut w);
        assert_eq!(w.join(" "), "one million two hundred thirty four thousand five hundred sixty seven");
    }

    /// TASK-honest-cedar-A73E: numbers read digit by digit and a domain read aloud compare
    /// with the way they are written.
    #[test]
    fn spoken_numbers_and_domains_match_their_written_form() {
        let script = "Read the whole dossier. One-oh-eight, at thinkask dot A I.";
        let heard = "Read the whole dossier. 108, at thinkask.ai.";
        assert_eq!(run(heard, script), []);
        assert_eq!(run(script, heard), [], "the same cleanup on both sides");
        assert_eq!(run("one oh eight at thinkask dot a i", "108 at thinkask.ai"), []);
        assert_eq!(run("one hundred and eight", "108"), []);
        assert_eq!(run("error four oh four", "Error 404"), []);
        let c = coverage(&t(heard), script, &CheckOptions::default());
        assert_eq!(c.match_ratio(), 1.0, "{c:?}");
        // a real skipped word still counts
        assert_eq!(run("Read the dossier. 108, at thinkask.ai.", script).len(), 1);
        assert!(coverage(&t("Read the dossier."), script, &CheckOptions::default()).match_ratio() < 0.7);
        // a plain "dot" and single letters still compare as before when both sides agree
        assert_eq!(run("a dot on the map", "A dot on the map"), []);
        assert_eq!(run("a twelve hour clock", "A 24-hour clock"), [pair("24-hour", "twelve")]);
    }

    #[test]
    fn words_that_sound_alike_pass_unless_strict() {
        assert_eq!(run("seven pains", "Seven panes"), []);
        assert_eq!(run("ask Cloud to help", "Ask Claude to help"), []);
        let strict = CheckOptions { similarity: 1.0, ..Default::default() };
        let m = check(&t("ask Cloud to help"), "Ask Claude to help", &strict);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].script.as_str(), m[0].heard.as_str()), ("Claude", "Cloud"));
        // alike in letters but not in sound
        assert!(!tolerable("todos", "dolos", DEFAULT_SIMILARITY));
        assert_eq!(sound_key("panes"), sound_key("pains"));
        assert_ne!(sound_key("todos"), sound_key("dolos"));
    }

    #[test]
    fn brand_dictionary_maps_heard_phrases() {
        let script = "Open the Due tab in Just Notes.";
        assert_eq!(run("Open the do tab in just notes", script), [pair("Due", "do")]);
        let dict: BTreeMap<String, String> =
            [("do tab".to_string(), "Due tab".to_string()), ("just notes".to_string(), "Just Notes".to_string())]
                .into();
        let m = check(
            &t("Open the do tab in just notes"),
            script,
            &CheckOptions { dictionary: dict, ..Default::default() },
        );
        assert_eq!(m, []);
    }

    #[test]
    fn missing_and_extra_words_are_placed_between_their_neighbours() {
        let m = check(&t("the brown fox jumps"), "The quick brown fox jumps.", &CheckOptions::default());
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!((m[0].script.as_str(), m[0].heard.as_str()), ("quick", ""));
        assert_eq!((m[0].start, m[0].end), (0.8, 1.0), "between \"the\" and \"brown\"");
        let m = check(&t("the brown fox really jumps"), "The brown fox jumps.", &CheckOptions::default());
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!((m[0].script.as_str(), m[0].heard.as_str(), m[0].script_index), ("", "really", 3));
        assert_eq!(m[0].context, "The brown fox [] jumps.");
        assert_eq!(m[0].heard_context, "the brown fox [really] jumps");
        let m = check(&t("the brown fox jumps"), "The quick brown fox jumps.", &CheckOptions::default());
        assert_eq!(m[0].heard_context, "the [] brown fox jumps");
    }

    #[test]
    fn ignore_and_min_span_words() {
        let heard = t("Dolos live in plants now");
        let script = "Todos live in Glance now";
        let all = check(&heard, script, &CheckOptions::default());
        assert_eq!(all.len(), 2, "{all:?}");
        let ig = CheckOptions { ignore: vec!["Glance".into()], ..Default::default() };
        let m = check(&heard, script, &ig);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].script, "Todos");
        let two = CheckOptions { min_span_words: 2, ..Default::default() };
        assert_eq!(check(&heard, script, &two), []);
        assert_eq!(check(&t("Extract it to do now"), "Extract Todos now", &two).len(), 1);
    }

    #[test]
    fn two_separate_slips_are_two_mismatches() {
        let m = run("Dolos live right in your boats", "Todos live right in your notes");
        assert_eq!(m, [pair("Todos", "Dolos"), pair("notes", "boats")]);
    }
}
