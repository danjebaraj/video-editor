//! Caption styles (all ASS): bold_pop, karaoke, word_by_word, clean_sentence, boxed. Plus SRT.

use crate::ass::{color_tag, escape_text, Doc, Event, Style};
use crate::fonts::FontDb;
use crate::layout::fit;
use crate::templates::{anchor, rounded_rect, BoxOut, Frame, ImageOut};
use anyhow::Result;
use cutline_core::brand::Brand;
use cutline_core::model::Position;
use std::collections::BTreeMap;

/// A timed word in timeline seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct CWord {
    pub w: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Default)]
pub struct CaptionCfg {
    pub style: String,
    pub position: Option<Position>,
    pub max_words: Option<usize>,
    pub max_chars: Option<usize>,
    pub max_lines: Option<usize>,
    pub emphasis: Vec<String>,
    pub emoji: BTreeMap<String, String>,
    pub font: Option<String>,
    pub size: Option<f64>,
    pub color: Option<String>,
    pub accent: Option<String>,
    pub uppercase: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct CaptionOut {
    pub boxes: Vec<BoxOut>,
    pub images: Vec<ImageOut>,
    pub chunks: usize,
}

fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect::<String>().to_lowercase()
}

/// Group words into caption chunks. Breaks at the word/char limits, sentence ends and pauses
/// (> 0.5 s). Cuts are handled by snapping the display span (see `chunk_span`).
pub fn chunk(words: &[CWord], max_words: usize, max_chars: usize, cuts: &[f64]) -> Vec<Vec<CWord>> {
    let mut out: Vec<Vec<CWord>> = vec![];
    let mut cur: Vec<CWord> = vec![];
    for (i, w) in words.iter().enumerate() {
        if !cur.is_empty() {
            let prev = cur.last().unwrap();
            let chars = cur.iter().map(|x| x.w.chars().count() + 1).sum::<usize>() + w.w.chars().count();
            // cuts don't force a break (jump cuts would shred sentences); spans snap to cuts instead
            let _ = cuts;
            if cur.len() >= max_words
                || chars > max_chars
                || prev.w.ends_with(['.', '?', '!'])
                || w.start - prev.end > 0.5
            {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.push(w.clone());
        let _ = i;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Chunk display span: first word start → last word end (held up to 0.5 s into a pause), min
/// 0.3 s; snapped to cuts closer than two frames.
pub fn chunk_span(c: &[CWord], next_start: Option<f64>, cuts: &[f64], frame: f64) -> (f64, f64) {
    let s = c[0].start;
    let mut e = c.last().unwrap().end;
    let lim = next_start.unwrap_or(f64::MAX);
    e = (e + 0.5).min(lim).max(e.min(lim));
    if e - s < 0.3 {
        e = (s + 0.3).min(lim.max(s + 0.05));
    }
    let mut s2 = s;
    for &cut in cuts {
        if cut > s2 && cut < e {
            if e - cut < 2.0 * frame {
                e = cut;
            } else if cut - s2 < 2.0 * frame {
                s2 = cut;
            }
        }
    }
    (s2, e.max(s2 + frame))
}

pub fn render(
    doc: &mut Doc,
    db: &FontDb,
    fr: &Frame,
    brand: &Brand,
    words: &[CWord],
    cfg: &CaptionCfg,
    cuts: &[f64],
    frame: f64,
) -> Result<CaptionOut> {
    let mut out = CaptionOut::default();
    if words.is_empty() {
        return Ok(out);
    }
    let style = if cfg.style.is_empty() {
        brand.caption_style.clone().unwrap_or_else(|| "bold_pop".into())
    } else {
        cfg.style.clone()
    };
    let u = fr.u();
    let vertical = fr.is_vertical();
    let accent = cfg.accent.clone().unwrap_or_else(|| brand.colors.accent.clone());
    let color = cfg.color.clone().unwrap_or_else(|| brand.colors.text.clone());
    let (default_words, default_chars) = match style.as_str() {
        "word_by_word" => (1, 40),
        "karaoke" => (8, if vertical { 26 } else { 42 }),
        "clean_sentence" => (14, if vertical { 44 } else { 84 }),
        _ => (3, if vertical { 22 } else { 32 }),
    };
    let max_words = cfg.max_words.unwrap_or(default_words).max(1);
    let max_chars = cfg.max_chars.unwrap_or(default_chars).max(4);
    let upper = cfg.uppercase.unwrap_or(matches!(style.as_str(), "bold_pop" | "word_by_word" | "boxed"));
    let font = cfg.font.clone().unwrap_or_else(|| {
        if style == "clean_sentence" {
            brand.fonts.body.clone()
        } else {
            brand.fonts.caption.clone()
        }
    });
    let bold = style == "clean_sentence";
    let fe = db.get(&font, bold).ok_or_else(|| anyhow::anyhow!("caption font {font} missing"))?;
    let base_size = cfg.size.unwrap_or(match style.as_str() {
        "word_by_word" => u * 13.0,
        "clean_sentence" => u * if vertical { 5.6 } else { 4.6 },
        "karaoke" => u * if vertical { 7.5 } else { 6.0 },
        _ => u * if vertical { 10.0 } else { 7.0 },
    });
    let max_lines = cfg.max_lines.unwrap_or(2);
    let default_pos = if vertical { "lower_mid" } else { "bottom" };
    let (sx, sy, sw, sh) = fr.safe;
    let outline = if style == "clean_sentence" { base_size * 0.3 } else { (base_size * 0.09).max(2.0) };
    let sname = format!("cap_{style}");
    let mut st = Style::new(&sname, &font, base_size);
    st.bold = bold;
    st.primary = color.clone();
    st.outline_color = brand.colors.stroke.clone();
    st.outline = outline;
    st.shadow = if style == "clean_sentence" { 0.0 } else { (base_size * 0.05).max(2.0) };
    st.back = "#000000A0".into();
    if style == "clean_sentence" {
        st.border_style = 3;
        st.outline_color = "#000000A0".into();
    }
    if style == "karaoke" {
        st.primary = accent.clone();
        st.secondary = color.clone();
    }
    if style == "boxed" {
        st.primary = "#111111".into();
        st.outline = 0.0;
        st.shadow = 0.0;
    }
    doc.add_style(st);

    let cw: Vec<CWord> =
        words.iter().map(|w| CWord { w: if upper { w.w.to_uppercase() } else { w.w.clone() }, ..w.clone() }).collect();
    let chunks = chunk(&cw, max_words, max_chars, cuts);
    out.chunks = chunks.len();
    let is_emph = |w: &str| cfg.emphasis.iter().any(|e| norm(e) == norm(w));
    for (ci, c) in chunks.iter().enumerate() {
        let next = chunks.get(ci + 1).map(|n| n[0].start);
        let (s, e) = chunk_span(c, next, cuts, frame);
        let text: String = c.iter().map(|w| w.w.as_str()).collect::<Vec<_>>().join(" ");
        // the popped word is ~110 %: fit with that headroom
        let headroom = if matches!(style.as_str(), "bold_pop" | "word_by_word" | "boxed") { 1.1 } else { 1.0 };
        let b = fit(fe, &text, base_size, base_size * 0.45, sw * 0.92 / headroom, sh * 0.3, max_lines, outline, 1.0);
        let (ax, ay) = anchor(cfg.position.as_ref(), default_pos, fr);
        let bw = b.width * headroom;
        let bh = b.height * headroom;
        let cx = ax.clamp(sx + bw / 2.0, sx + sw - bw / 2.0);
        let cy = ay.clamp(sy + bh / 2.0, sy + sh - bh / 2.0);
        let fs = if (b.size - base_size).abs() > 0.5 { format!("\\fs{:.0}", b.size) } else { String::new() };
        // word index → line break positions from the fitted layout
        let mut breaks = vec![];
        let mut acc = 0;
        for l in &b.lines {
            acc += l.split_whitespace().count();
            breaks.push(acc);
        }
        let join_words = |hl: Option<usize>| -> String {
            let mut s = String::new();
            for (k, w) in c.iter().enumerate() {
                if k > 0 {
                    s.push_str(if breaks.contains(&k) { "\\N" } else { " " });
                }
                let e = escape_text(&w.w);
                let emph = is_emph(&w.w);
                match (hl == Some(k), style.as_str()) {
                    (true, "boxed") => {
                        s.push_str(&format!("{{\\c{}}}{e}{{\\c{}}}", color_tag("#FFFFFF"), color_tag("#111111")))
                    }
                    (true, _) => s.push_str(&format!(
                        "{{\\c{}\\fscx112\\fscy112\\t(0,110,\\fscx106\\fscy106)}}{e}{{\\c{}\\fscx100\\fscy100}}",
                        color_tag(&accent),
                        color_tag(&color)
                    )),
                    (false, _) if emph => s.push_str(&format!(
                        "{{\\c{}}}{e}{{\\c{}}}",
                        color_tag(&accent),
                        color_tag(if style == "boxed" { "#111111" } else { &color })
                    )),
                    _ => s.push_str(&e),
                }
            }
            s
        };
        let pos = format!("\\an5\\pos({:.0},{:.0}){fs}", cx, cy);
        match style.as_str() {
            "bold_pop" | "boxed" => {
                if style == "boxed" {
                    let pad = b.size * 0.35;
                    let (rw, rh) = (b.width + 2.0 * pad, b.height + pad);
                    doc.events.push(Event {
                        layer: 4,
                        start: s,
                        end: e,
                        style: sname.clone(),
                        text: format!(
                            "{{\\an7\\pos({:.0},{:.0})\\p1\\bord0\\shad0\\c{}}}{}{{\\p0}}",
                            cx - rw / 2.0,
                            cy - rh / 2.0,
                            color_tag(&accent),
                            rounded_rect(rw, rh, b.size * 0.3)
                        ),
                    });
                }
                for (k, w) in c.iter().enumerate() {
                    let t0 = if k == 0 { s } else { w.start.max(s) };
                    let t1 = if k + 1 < c.len() { c[k + 1].start.min(e) } else { e };
                    if t1 <= t0 {
                        continue;
                    }
                    let intro = if k == 0 { "\\fad(40,0)" } else { "" };
                    doc.events.push(Event {
                        layer: 5,
                        start: t0,
                        end: t1,
                        style: sname.clone(),
                        text: format!("{{{pos}{intro}}}{}", join_words(Some(k))),
                    });
                }
            }
            "word_by_word" => {
                for (k, w) in c.iter().enumerate() {
                    let t0 = if k == 0 { s } else { w.start };
                    let t1 = if k + 1 < c.len() { c[k + 1].start } else { e };
                    let col = if is_emph(&w.w) { format!("\\c{}", color_tag(&accent)) } else { String::new() };
                    doc.events.push(Event {
                        layer: 5,
                        start: t0,
                        end: t1.max(t0 + frame),
                        style: sname.clone(),
                        text: format!("{{{pos}{col}\\fscx80\\fscy80\\t(0,80,\\fscx112\\fscy112)\\t(80,150,\\fscx100\\fscy100)}}{}", escape_text(&w.w)),
                    });
                }
            }
            "karaoke" => {
                let mut t = String::new();
                let mut cursor = s;
                for (k, w) in c.iter().enumerate() {
                    if k > 0 {
                        t.push_str(if breaks.contains(&k) { "\\N" } else { " " });
                    }
                    let d_cs = ((w.end.max(cursor) - cursor) * 100.0).round().max(1.0) as i64;
                    let emph = if is_emph(&w.w) { "\\b1" } else { "" };
                    t.push_str(&format!("{{\\kf{d_cs}{emph}}}{}", escape_text(&w.w)));
                    cursor = w.end.max(cursor);
                    if let Some(n) = c.get(k + 1) {
                        // gap before next word: hold the fill
                        let gap = ((n.start - cursor) * 100.0).round() as i64;
                        if gap > 0 {
                            t.push_str(&format!("{{\\k{gap}}}"));
                            cursor = n.start;
                        }
                    }
                }
                doc.events.push(Event {
                    layer: 5,
                    start: s,
                    end: e,
                    style: sname.clone(),
                    text: format!("{{{pos}\\fad(60,60)}}{t}"),
                });
            }
            _ => {
                // clean_sentence: soft box (BorderStyle 3)
                doc.events.push(Event {
                    layer: 5,
                    start: s,
                    end: e,
                    style: sname.clone(),
                    text: format!("{{{pos}\\fad(80,80)}}{}", join_words(None)),
                });
            }
        }
        let pad = if style == "boxed" { b.size * 0.35 } else { 0.0 };
        out.boxes.push(BoxOut {
            start: s,
            end: e,
            x: cx - bw / 2.0 - pad,
            y: cy - bh / 2.0,
            w: bw + 2.0 * pad,
            h: bh,
            text: text.clone(),
        });
        // emoji for emphasis words
        for w in c {
            if let Some(name) = cfg
                .emoji
                .get(&norm(&w.w))
                .or_else(|| cfg.emoji.iter().find(|(k, _)| norm(k) == norm(&w.w)).map(|(_, v)| v))
            {
                let path = cutline_deps::paths::assets_dir().join("emoji").join(format!("{name}.png"));
                if path.exists() {
                    let ew = b.size * 1.3;
                    let ey = (cy - bh / 2.0 - ew * 0.6).max(sy + ew / 2.0);
                    out.images.push(ImageOut { path, start: w.start.max(s), end: e, cx, cy: ey, w: ew, pop: true });
                }
            }
        }
    }
    Ok(out)
}

/// SRT text from words (sentence-level cues, ≤ 42 chars/line, ≤ 2 lines).
pub fn srt(words: &[CWord], cuts: &[f64]) -> String {
    let chunks = chunk(words, 16, 84, cuts);
    let mut s = String::new();
    let tc = |t: f64| {
        let ms = (t.max(0.0) * 1000.0).round() as i64;
        format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000)
    };
    for (i, c) in chunks.iter().enumerate() {
        let next = chunks.get(i + 1).map(|n| n[0].start);
        let (a, b) = chunk_span(c, next, &[], 0.001);
        let text = c.iter().map(|w| w.w.as_str()).collect::<Vec<_>>().join(" ");
        // wrap at ~42 chars into ≤ 2 lines
        let mut lines = vec![String::new()];
        for w in text.split(' ') {
            let n = lines.len();
            let l = lines.last_mut().unwrap();
            if !l.is_empty() && l.len() + 1 + w.len() > 42 && n < 2 {
                lines.push(w.to_string());
            } else {
                if !l.is_empty() {
                    l.push(' ');
                }
                l.push_str(w);
            }
        }
        s.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, tc(a), tc(b), lines.join("\n")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    fn w(s: &str, a: f64, b: f64) -> CWord {
        CWord { w: s.into(), start: a, end: b }
    }
    #[test]
    fn chunking_rules() {
        let ws = vec![
            w("one", 0.0, 0.2),
            w("two", 0.25, 0.4),
            w("three.", 0.45, 0.7),
            w("four", 0.75, 0.9),
            w("five", 1.6, 1.8),
            w("six", 1.85, 2.0),
        ];
        let c = chunk(&ws, 3, 22, &[]);
        assert_eq!(c.iter().map(|x| x.len()).collect::<Vec<_>>(), vec![3, 1, 2]);
        let c = chunk(&ws, 3, 22, &[1.95]);
        assert_eq!(c.iter().map(|x| x.len()).collect::<Vec<_>>(), vec![3, 1, 2], "cut inside a word doesn't split");
        let c = chunk(&ws, 3, 22, &[1.82]);
        assert_eq!(c.iter().map(|x| x.len()).collect::<Vec<_>>(), vec![3, 1, 2], "jump cuts don't shred chunks");
    }
    #[test]
    fn spans_snap_to_cuts() {
        let c = vec![w("a", 1.0, 1.5)];
        let f = 1.0 / 30.0;
        // cut 1 frame before the natural end → end snaps to the cut
        let (s, e) = chunk_span(&c, None, &[1.98], f);
        assert_eq!(s, 1.0);
        assert!((e - 1.98).abs() < 1e-9, "{e}");
        let (s, _) = chunk_span(&c, None, &[1.02], f);
        assert!((s - 1.02).abs() < 1e-9);
    }
    #[test]
    fn srt_format() {
        let s = srt(&[w("Hello", 0.0, 0.5), w("world.", 0.6, 1.0), w("Next", 2.0, 2.3)], &[]);
        assert!(s.starts_with("1\n00:00:00,000 --> 00:00:01,500\nHello world.\n\n2\n"), "{s}");
    }
}
