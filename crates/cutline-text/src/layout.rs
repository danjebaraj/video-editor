//! Line wrapping and auto-fit (shrink until the block fits a box).

use crate::fonts::{FontEntry, Measure};

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub size: f64,
    pub lines: Vec<String>,
    /// Block width/height in px including outline.
    pub width: f64,
    pub height: f64,
}

/// Greedy word wrap at `max_w`. Words longer than a line stay on their own line.
pub fn wrap(m: &Measure, text: &str, max_w: f64) -> Vec<String> {
    let mut lines = vec![];
    for para in text.split('\n') {
        let mut cur = String::new();
        for w in para.split_whitespace() {
            let cand = if cur.is_empty() { w.to_string() } else { format!("{cur} {w}") };
            if m.width(&cand) <= max_w || cur.is_empty() {
                cur = cand;
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = w.to_string();
            }
        }
        lines.push(cur);
    }
    lines
}

/// Balance a 2-line wrap so lines have similar widths (nicer titles).
pub fn balance(m: &Measure, lines: Vec<String>, max_w: f64) -> Vec<String> {
    if lines.len() != 2 {
        return lines;
    }
    let words: Vec<&str> = lines.iter().flat_map(|l| l.split_whitespace()).collect();
    let mut best = lines.clone();
    let mut best_d = f64::MAX;
    for k in 1..words.len() {
        let a = words[..k].join(" ");
        let b = words[k..].join(" ");
        let (wa, wb) = (m.width(&a), m.width(&b));
        if wa <= max_w && wb <= max_w && (wa - wb).abs() < best_d {
            best_d = (wa - wb).abs();
            best = vec![a, b];
        }
    }
    best
}

/// Find the largest size ≤ `size` (down to `min_size`) at which `text` wraps into at most
/// `max_lines` lines fitting `box_w × box_h`. `outline` is added on every side.
pub fn fit(
    font: &FontEntry,
    text: &str,
    size: f64,
    min_size: f64,
    box_w: f64,
    box_h: f64,
    max_lines: usize,
    outline: f64,
    line_gap: f64,
) -> Block {
    let mut s = size;
    loop {
        let m = Measure { font, size: s, spacing: 0.0 };
        let inner_w = (box_w - 2.0 * outline).max(1.0);
        let mut lines = wrap(&m, text, inner_w);
        lines = balance(&m, lines, inner_w);
        let w = lines.iter().map(|l| m.width(l)).fold(0.0, f64::max) + 2.0 * outline;
        let lh = m.line_height() * line_gap;
        let h = lh * lines.len() as f64 + 2.0 * outline;
        let ok = lines.len() <= max_lines && w <= box_w + 0.5 && h <= box_h + 0.5;
        if ok || s <= min_size {
            return Block { size: s, lines, width: w, height: h };
        }
        s = (s * 0.94).max(min_size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::FontDb;
    #[test]
    fn long_title_fits() {
        let db = FontDb::bundled().unwrap();
        let f = db.get("Anton", false).unwrap();
        let b = fit(
            f,
            "Your knife is dull because of THIS one tiny mistake everybody makes",
            150.0,
            40.0,
            780.0,
            600.0,
            3,
            6.0,
            1.0,
        );
        assert!(b.width <= 780.5 && b.height <= 600.5 && b.lines.len() <= 3, "{b:?}");
        assert!(b.size < 150.0);
        let b2 = fit(f, "Short", 150.0, 40.0, 780.0, 600.0, 3, 6.0, 1.0);
        assert_eq!(b2.size, 150.0);
    }
}
