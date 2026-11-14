//! ASS-based text/graphics templates.

use crate::ass::{color_tag, escape_text, Doc, Event, Style};
use crate::fonts::FontDb;
use crate::layout::fit;
use anyhow::{bail, Result};
use cutline_core::brand::Brand;
use cutline_core::model::Position;
use serde::Serialize;
use serde_json::{Map, Value};

/// Output frame + safe zone (pixels at the full target size).
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub safe: (f64, f64, f64, f64),
}

impl Frame {
    pub fn is_vertical(&self) -> bool {
        self.height > self.width
    }
    /// Reference unit: 1/100 of the shorter side (scales typography across targets).
    pub fn u(&self) -> f64 {
        self.width.min(self.height) as f64 / 100.0
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BoxOut {
    pub start: f64,
    pub end: f64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub text: String,
}

/// An image overlay a template needs (emoji).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImageOut {
    pub path: std::path::PathBuf,
    pub start: f64,
    pub end: f64,
    /// Center in output pixels.
    pub cx: f64,
    pub cy: f64,
    /// Width in output pixels.
    pub w: f64,
    pub pop: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TemplateOut {
    pub boxes: Vec<BoxOut>,
    pub images: Vec<ImageOut>,
}

pub struct Input<'a> {
    pub template: &'a str,
    pub text: Option<&'a str>,
    pub subtitle: Option<&'a str>,
    pub params: &'a Map<String, Value>,
    pub start: f64,
    pub end: f64,
    pub position: Option<&'a Position>,
    pub index: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [&'static str],
}

pub fn list() -> Vec<TemplateInfo> {
    vec![
        TemplateInfo {
            name: "hook_title",
            description: "Big centered title with pop-in, stroke and shadow; auto-wraps and shrinks to the safe zone",
            params: &["text", "emphasis[]", "color", "accent", "font", "size", "uppercase", "position"],
        },
        TemplateInfo {
            name: "lower_third",
            description: "Name + subtitle with an accent bar that slides in",
            params: &["text (name)", "subtitle", "accent", "font", "position"],
        },
        TemplateInfo {
            name: "callout",
            description: "Label in an accent pill with an arrow",
            params: &["text", "arrow (down|up|left|right|none)", "accent", "position {x,y}"],
        },
        TemplateInfo {
            name: "step_counter",
            description: "\"Step 2/5\" pill",
            params: &["step", "total", "text (overrides)", "accent", "position"],
        },
        TemplateInfo {
            name: "big_number",
            description: "Huge number counting up, with a label",
            params: &["number", "prefix", "suffix", "decimals", "subtitle (label)", "accent", "position"],
        },
        TemplateInfo {
            name: "quote",
            description: "Quote with large accent quote marks",
            params: &["text", "subtitle (attribution)", "accent", "position"],
        },
        TemplateInfo {
            name: "cta_subscribe",
            description: "Red SUBSCRIBE pill with a pop",
            params: &["text", "color", "position"],
        },
        TemplateInfo {
            name: "end_card",
            description: "Thanks title + end-screen placeholders (long form) or CTA (shorts)",
            params: &["text", "subtitle", "placeholders (bool)"],
        },
        TemplateInfo {
            name: "emoji_pop",
            description: "Noto emoji with a scale pop",
            params: &["text|emoji (name)", "size (fraction of width)", "position"],
        },
        TemplateInfo {
            name: "title_card",
            description: "Business title: brand logo, product name, one-line promise (full-frame card)",
            params: &["text (name)", "subtitle (promise)", "logo (false hides it)", "background", "color", "accent"],
        },
        TemplateInfo {
            name: "section_card",
            description: "Chapter divider with a number; also starts a chapter in `cutline chapters`",
            params: &["text (title)", "subtitle", "number", "background"],
        },
        TemplateInfo {
            name: "feature_list",
            description: "Heading and up to 5 bullets that build one by one",
            params: &["text (heading)", "items[]", "reveal[] (times or cues, one per item)", "background (scrim)"],
        },
        TemplateInfo {
            name: "stat",
            description: "Number counting up with a label and a source line",
            params: &[
                "number",
                "prefix",
                "suffix",
                "decimals",
                "subtitle (label)",
                "source",
                "position",
                "background (scrim)",
            ],
        },
        TemplateInfo {
            name: "testimonial",
            description: "Customer quote with name, title, company, photo and logo",
            params: &[
                "text (quote)",
                "name",
                "title",
                "company",
                "photo (image asset)",
                "logo (image asset)",
                "background",
            ],
        },
        TemplateInfo {
            name: "compare",
            description: "Two columns: before/after or them/us (the right column is highlighted)",
            params: &["text (heading)", "left {title, items[]}", "right {title, items[]}", "background"],
        },
        TemplateInfo {
            name: "logo_wall",
            description: "Customer logos in a grid",
            params: &["text (heading)", "logos[] (image assets, up to 12)", "monochrome", "background"],
        },
        TemplateInfo {
            name: "cta_link",
            description: "Closing call to action: headline, URL pill and a QR code of the URL",
            params: &["text (headline)", "subtitle", "url", "qr (false hides it)", "background"],
        },
    ]
}

pub fn emoji_names() -> Vec<String> {
    let dir = cutline_deps::paths::assets_dir().join("emoji");
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".png")).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn ps<'a>(p: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    p.get(k).and_then(|v| v.as_str())
}
fn pf(p: &Map<String, Value>, k: &str) -> Option<f64> {
    p.get(k).and_then(|v| v.as_f64())
}

/// Center point for a position (preset or normalized point) inside the safe zone.
pub fn anchor(pos: Option<&Position>, default: &str, fr: &Frame) -> (f64, f64) {
    let (sx, sy, sw, sh) = fr.safe;
    let (w, h) = (fr.width as f64, fr.height as f64);
    let preset = match pos {
        Some(Position::Point(p)) => return (p.x * w, p.y * h),
        Some(Position::Preset(s)) => s.as_str(),
        None => default,
    };
    match preset {
        "top" => (sx + sw / 2.0, sy + sh * 0.12),
        "upper_mid" => (sx + sw / 2.0, sy + sh * 0.25),
        "lower_mid" => (sx + sw / 2.0, sy + sh * 0.72),
        "bottom" => (sx + sw / 2.0, sy + sh * 0.9),
        "left" => (sx + sw * 0.25, sy + sh / 2.0),
        "right" => (sx + sw * 0.75, sy + sh / 2.0),
        "top_left" => (sx + sw * 0.25, sy + sh * 0.12),
        "top_right" => (sx + sw * 0.75, sy + sh * 0.12),
        "bottom_left" => (sx + sw * 0.25, sy + sh * 0.88),
        "bottom_right" => (sx + sw * 0.75, sy + sh * 0.88),
        _ => (sx + sw / 2.0, sy + sh / 2.0),
    }
}

/// Clamp a centered box into the safe zone.
fn clamp_center(cx: f64, cy: f64, w: f64, h: f64, fr: &Frame) -> (f64, f64) {
    let (sx, sy, sw, sh) = fr.safe;
    let x = cx.clamp(sx + w / 2.0, (sx + sw - w / 2.0).max(sx + w / 2.0));
    let y = cy.clamp(sy + h / 2.0, (sy + sh - h / 2.0).max(sy + h / 2.0));
    (x, y)
}

/// Rounded rectangle ASS drawing path (origin top-left).
pub fn rounded_rect(w: f64, h: f64, r: f64) -> String {
    let r = r.min(w / 2.0).min(h / 2.0);
    let k = 0.4477 * r; // bezier handle offset for quarter circles
    let n = |v: f64| format!("{:.0}", v);
    format!(
        "m {r} 0 l {wr} 0 b {wk} 0 {w} {k} {w} {r} l {w} {hr} b {w} {hk} {wk} {h} {wr} {h} l {r} {h} b {k} {h} 0 {hk} 0 {hr} l 0 {r} b 0 {k} {k} 0 {r} 0",
        r = n(r),
        wr = n(w - r),
        wk = n(w - k),
        w = n(w),
        k = n(k),
        hr = n(h - r),
        hk = n(h - k),
        h = n(h)
    )
}

fn emph_text(text: &str, emphasis: &[String], accent: &str, base: &str) -> String {
    text.split(' ')
        .map(|w| {
            let bare: String = w.chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect();
            let e = escape_text(w);
            if emphasis.iter().any(|x| x.eq_ignore_ascii_case(&bare) && !bare.is_empty()) {
                format!("{{\\c{}}}{e}{{\\c{}}}", color_tag(accent), color_tag(base))
            } else {
                e
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Add a template's styles/events to `doc`. Returns QA boxes and image overlays.
pub fn render(doc: &mut Doc, db: &FontDb, fr: &Frame, brand: &Brand, inp: &Input) -> Result<TemplateOut> {
    let p = inp.params;
    let u = fr.u();
    let (s, e) = (inp.start, inp.end);
    let dur_ms = ((e - s) * 1000.0).round() as i64;
    let accent = ps(p, "accent").unwrap_or(&brand.colors.accent).to_string();
    let txt_color = ps(p, "color").unwrap_or(&brand.colors.text).to_string();
    let stroke = brand.colors.stroke.clone();
    let (sx, sy, sw, sh) = fr.safe;
    let mut out = TemplateOut::default();
    let sname = |n: &str| format!("{n}_{}", inp.index);
    let emphasis: Vec<String> = p
        .get("emphasis")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    match inp.template {
        "hook_title" => {
            let font = ps(p, "font").unwrap_or(&brand.fonts.title).to_string();
            let fe = db.get(&font, false).ok_or_else(|| anyhow::anyhow!("font {font} missing"))?;
            let mut text = inp.text.unwrap_or("").to_string();
            if p.get("uppercase").and_then(|v| v.as_bool()).unwrap_or(true) {
                text = text.to_uppercase();
            }
            let size = pf(p, "size").unwrap_or(u * if fr.is_vertical() { 13.5 } else { 11.0 });
            let outline = (size * 0.06).max(2.0);
            // pop overshoot to 110 %: fit at 110 % so the peak also stays inside the safe zone
            let b = fit(fe, &text, size, size * 0.3, sw / 1.1, sh * 0.4 / 1.1, 3, outline, 1.0);
            let (cx, cy) = anchor(inp.position, "upper_mid", fr);
            let (cx, cy) = clamp_center(cx, cy, b.width * 1.1, b.height * 1.1, fr);
            let mut st = Style::new(&sname("hook"), &font, b.size);
            st.primary = txt_color.clone();
            st.outline_color = stroke.clone();
            st.outline = outline;
            st.shadow = (size * 0.035).max(2.0);
            st.back = "#00000090".into();
            doc.add_style(st);
            let body =
                b.lines.iter().map(|l| emph_text(l, &emphasis, &accent, &txt_color)).collect::<Vec<_>>().join("\\N");
            doc.events.push(Event {
                layer: 10,
                start: s,
                end: e,
                style: sname("hook"),
                text: format!(
                    "{{\\an5\\pos({:.0},{:.0})\\fad(80,{})\\fscx55\\fscy55\\t(0,140,\\fscx110\\fscy110)\\t(140,240,\\fscx100\\fscy100)}}{body}",
                    cx,
                    cy,
                    150.min(dur_ms / 3)
                ),
            });
            out.boxes.push(BoxOut {
                start: s,
                end: e,
                x: cx - b.width / 2.0,
                y: cy - b.height / 2.0,
                w: b.width,
                h: b.height,
                text: text.clone(),
            });
        }
        "lower_third" => {
            let font = ps(p, "font").unwrap_or(&brand.fonts.body).to_string();
            let name = inp.text.unwrap_or("");
            let sub = inp.subtitle.unwrap_or("");
            let fb = db.get(&font, true).ok_or_else(|| anyhow::anyhow!("font {font} missing"))?;
            let ns = u * if fr.is_vertical() { 6.0 } else { 5.0 };
            let max_w = sw * 0.85;
            let nb = fit(fb, name, ns, ns * 0.4, max_w, ns * 1.3, 1, 0.0, 1.0);
            let sb = fit(fb, sub, ns * 0.62, ns * 0.3, max_w, ns, 1, 0.0, 1.0);
            let pad = u * 2.0;
            let bar_w = u * 1.2;
            let text_w = nb.width.max(sb.width);
            let total_h = nb.height + if sub.is_empty() { 0.0 } else { sb.height + u * 0.6 };
            let (ax, ay) = anchor(inp.position, "bottom_left", fr);
            let left = (ax - (sw * 0.25)).max(sx);
            let top = (ay - total_h / 2.0).min(sy + sh - total_h - pad).max(sy);
            let x_text = left + bar_w + pad;
            for (nm, sz, bold) in [(sname("lt_name"), nb.size, true), (sname("lt_sub"), sb.size, false)] {
                let mut st = Style::new(&nm, &font, sz);
                st.bold = bold;
                st.primary = txt_color.clone();
                st.outline_color = "#000000".into();
                st.outline = 0.0;
                st.shadow = (sz * 0.05).max(1.5);
                st.back = "#00000099".into();
                st.alignment = 7;
                doc.add_style(st);
            }
            let fade_out = 200.min(dur_ms / 3);
            // accent bar slides in from the left
            doc.events.push(Event {
                layer: 9,
                start: s,
                end: e,
                style: sname("lt_name"),
                text: format!(
                    "{{\\an7\\move({:.0},{:.0},{:.0},{:.0},0,250)\\fad(0,{fade_out})\\p1\\bord0\\shad0\\c{}}}{}{{\\p0}}",
                    left - u * 30.0,
                    top,
                    left,
                    top,
                    color_tag(&accent),
                    rounded_rect(bar_w, total_h, bar_w / 2.0)
                ),
            });
            doc.events.push(Event {
                layer: 10,
                start: s + 0.12,
                end: e,
                style: sname("lt_name"),
                text: format!("{{\\an7\\pos({:.0},{:.0})\\fad(200,{fade_out})}}{}", x_text, top, escape_text(name)),
            });
            if !sub.is_empty() {
                doc.events.push(Event {
                    layer: 10,
                    start: s + 0.22,
                    end: e,
                    style: sname("lt_sub"),
                    text: format!(
                        "{{\\an7\\pos({:.0},{:.0})\\fad(200,{fade_out})}}{}",
                        x_text,
                        top + nb.height + u * 0.6,
                        escape_text(sub)
                    ),
                });
            }
            out.boxes.push(BoxOut {
                start: s,
                end: e,
                x: left,
                y: top,
                w: bar_w + pad + text_w,
                h: total_h,
                text: format!("{name} {sub}").trim().to_string(),
            });
        }
        "callout" | "step_counter" | "cta_subscribe" => {
            let (label, font, bg, fg, default_pos) = match inp.template {
                "callout" => (
                    inp.text.unwrap_or("").to_string(),
                    brand.fonts.body.clone(),
                    accent.clone(),
                    "#111111".to_string(),
                    "center",
                ),
                "step_counter" => {
                    let t = match (inp.text, pf(p, "step"), pf(p, "total")) {
                        (Some(t), _, _) => t.to_string(),
                        (None, Some(a), Some(b)) => format!("Step {}/{}", a as i64, b as i64),
                        (None, Some(a), None) => format!("Step {}", a as i64),
                        _ => bail!("step_counter needs `step` (and optionally `total`) or `text`"),
                    };
                    (t, brand.fonts.body.clone(), accent.clone(), "#111111".to_string(), "top_left")
                }
                _ => (
                    inp.text.unwrap_or("SUBSCRIBE").to_uppercase(),
                    brand.fonts.body.clone(),
                    ps(p, "color").unwrap_or("#FF0033").to_string(),
                    "#FFFFFF".to_string(),
                    "lower_mid",
                ),
            };
            let font = ps(p, "font").map(str::to_string).unwrap_or(font);
            let fe = db.get(&font, true).ok_or_else(|| anyhow::anyhow!("font {font} missing"))?;
            let size = pf(p, "size").unwrap_or(u * if inp.template == "cta_subscribe" { 6.0 } else { 4.5 });
            let b = fit(fe, &label, size, size * 0.4, sw * 0.9, size * 1.4, 1, 0.0, 1.0);
            let padx = b.size * 0.6;
            let pady = b.size * 0.3;
            let (bw, bh) = (b.width + 2.0 * padx, b.height + 2.0 * pady);
            let arrow = if inp.template == "callout" { ps(p, "arrow").unwrap_or("down") } else { "none" };
            let ah = if arrow == "none" { 0.0 } else { b.size * 0.7 };
            let (cx, cy) = anchor(inp.position, default_pos, fr);
            let (cx, cy) = clamp_center(cx, cy, bw * 1.1, (bh + ah) * 1.1, fr);
            let (x0, y0) = (cx - bw / 2.0, cy - bh / 2.0);
            let mut st = Style::new(&sname(inp.template), &font, b.size);
            st.bold = true;
            st.primary = fg.clone();
            st.alignment = 5;
            doc.add_style(st);
            let pop = "\\fscx50\\fscy50\\t(0,120,\\fscx110\\fscy110)\\t(120,220,\\fscx100\\fscy100)";
            let fo = 150.min(dur_ms / 3);
            doc.events.push(Event {
                layer: 9,
                start: s,
                end: e,
                style: sname(inp.template),
                text: format!(
                    "{{\\an7\\pos({:.0},{:.0})\\fad(80,{fo})\\p1\\bord0\\shad3\\4c&H000000&\\c{}}}{}{{\\p0}}",
                    x0,
                    y0,
                    color_tag(&bg),
                    rounded_rect(bw, bh, bh / 2.0)
                ),
            });
            if arrow != "none" {
                let (aw, ahh) = (ah * 1.2, ah);
                let (ax, ay, path) = match arrow {
                    "up" => {
                        (cx - aw / 2.0, y0 - ahh, format!("m {:.0} 0 l {:.0} {:.0} l 0 {:.0}", aw / 2.0, aw, ahh, ahh))
                    }
                    "left" => {
                        (x0 - ahh, cy - aw / 2.0, format!("m 0 {:.0} l {:.0} 0 l {:.0} {:.0}", aw / 2.0, ahh, ahh, aw))
                    }
                    "right" => (x0 + bw, cy - aw / 2.0, format!("m 0 0 l {:.0} {:.0} l 0 {:.0}", ahh, aw / 2.0, aw)),
                    _ => (cx - aw / 2.0, y0 + bh, format!("m 0 0 l {:.0} 0 l {:.0} {:.0}", aw, aw / 2.0, ahh)),
                };
                doc.events.push(Event {
                    layer: 9,
                    start: s,
                    end: e,
                    style: sname(inp.template),
                    text: format!(
                        "{{\\an7\\pos({:.0},{:.0})\\fad(80,{fo})\\p1\\bord0\\shad0\\c{}}}{path}{{\\p0}}",
                        ax,
                        ay,
                        color_tag(&bg)
                    ),
                });
            }
            doc.events.push(Event {
                layer: 10,
                start: s,
                end: e,
                style: sname(inp.template),
                text: format!(
                    "{{\\an5\\pos({:.0},{:.0})\\fad(80,{fo}){}}}{}",
                    cx,
                    cy,
                    if inp.template == "step_counter" { "" } else { pop },
                    escape_text(&label)
                ),
            });
            out.boxes.push(BoxOut {
                start: s,
                end: e,
                x: x0,
                y: y0 - if arrow == "up" { ah } else { 0.0 },
                w: bw,
                h: bh + ah,
                text: label,
            });
        }
        "big_number" => {
            let font = ps(p, "font").unwrap_or(&brand.fonts.title).to_string();
            let fe = db.get(&font, false).ok_or_else(|| anyhow::anyhow!("font {font} missing"))?;
            let target = pf(p, "number")
                .or_else(|| inp.text.and_then(|t| t.replace(',', "").parse().ok()))
                .ok_or_else(|| anyhow::anyhow!("big_number needs `number`"))?;
            let dec = pf(p, "decimals").unwrap_or(if target.fract() == 0.0 { 0.0 } else { 1.0 }) as usize;
            let (pre, suf) = (ps(p, "prefix").unwrap_or(""), ps(p, "suffix").unwrap_or(""));
            let fmt = |v: f64| format!("{pre}{v:.dec$}{suf}");
            let final_txt = fmt(target);
            let size = pf(p, "size").unwrap_or(u * 24.0);
            let b = fit(fe, &final_txt, size, size * 0.3, sw * 0.9, sh * 0.3, 1, size * 0.04, 1.0);
            let (cx, cy) = anchor(inp.position, "center", fr);
            let label = inp.subtitle.unwrap_or("");
            let lf = db.get(&brand.fonts.body, true).ok_or_else(|| anyhow::anyhow!("body font missing"))?;
            let lb = fit(lf, label, u * 5.0, u * 2.0, sw * 0.9, u * 7.0, 1, 0.0, 1.0);
            let total_h = b.height + if label.is_empty() { 0.0 } else { lb.height + u };
            let (cx, cy) = clamp_center(cx, cy, b.width, total_h, fr);
            let top = cy - total_h / 2.0;
            let mut st = Style::new(&sname("num"), &font, b.size);
            st.primary = accent.clone();
            st.outline = b.size * 0.04;
            st.outline_color = stroke.clone();
            st.shadow = 4.0;
            doc.add_style(st);
            let count = 0.6f64.min((e - s) * 0.5);
            let steps = 18;
            for i in 0..=steps {
                let k = i as f64 / steps as f64;
                let ease = 1.0 - (1.0 - k).powi(3);
                let st_t = s + count * k;
                let en_t = if i == steps { e } else { s + count * (i + 1) as f64 / steps as f64 };
                doc.events.push(Event {
                    layer: 10,
                    start: st_t,
                    end: en_t,
                    style: sname("num"),
                    text: format!(
                        "{{\\an8\\pos({:.0},{:.0}){}}}{}",
                        cx,
                        top,
                        if i == steps { format!("\\fad(0,{})", 150.min(dur_ms / 3)) } else { String::new() },
                        escape_text(&fmt(target * ease))
                    ),
                });
            }
            if !label.is_empty() {
                let mut ls = Style::new(&sname("numlbl"), &brand.fonts.body, lb.size);
                ls.bold = true;
                ls.primary = txt_color.clone();
                ls.shadow = 2.0;
                doc.add_style(ls);
                doc.events.push(Event {
                    layer: 10,
                    start: s + 0.2,
                    end: e,
                    style: sname("numlbl"),
                    text: format!(
                        "{{\\an8\\pos({:.0},{:.0})\\fad(200,150)}}{}",
                        cx,
                        top + b.height + u,
                        escape_text(label)
                    ),
                });
            }
            out.boxes.push(BoxOut {
                start: s,
                end: e,
                x: cx - b.width.max(lb.width) / 2.0,
                y: top,
                w: b.width.max(lb.width),
                h: total_h,
                text: final_txt,
            });
        }
        "quote" => {
            let font = ps(p, "font").unwrap_or(&brand.fonts.body).to_string();
            let fe = db.get(&font, false).ok_or_else(|| anyhow::anyhow!("font {font} missing"))?;
            let text = inp.text.unwrap_or("");
            let size = pf(p, "size").unwrap_or(u * 6.0);
            let b = fit(fe, text, size, size * 0.4, sw * 0.82, sh * 0.5, 6, 0.0, 1.0);
            let (cx, cy) = anchor(inp.position, "center", fr);
            let att = inp.subtitle.unwrap_or("");
            let total_h = b.height + if att.is_empty() { 0.0 } else { b.size * 1.6 };
            let (cx, cy) = clamp_center(cx, cy, b.width, total_h + b.size * 2.0, fr);
            let top = cy - total_h / 2.0;
            let mut st = Style::new(&sname("quote"), &font, b.size);
            st.italic = true;
            st.primary = txt_color.clone();
            st.shadow = 3.0;
            doc.add_style(st);
            let mut qs = Style::new(&sname("qmark"), &brand.fonts.title, b.size * 3.0);
            qs.primary = accent.clone();
            doc.add_style(qs);
            doc.events.push(Event {
                layer: 9,
                start: s,
                end: e,
                style: sname("qmark"),
                text: format!("{{\\an2\\pos({:.0},{:.0})\\fad(200,150)}}\u{201C}", cx, top + b.size * 0.6),
            });
            doc.events.push(Event {
                layer: 10,
                start: s + 0.1,
                end: e,
                style: sname("quote"),
                text: format!(
                    "{{\\an8\\pos({:.0},{:.0})\\fad(250,150)}}{}",
                    cx,
                    top,
                    b.lines.iter().map(|l| escape_text(l)).collect::<Vec<_>>().join("\\N")
                ),
            });
            if !att.is_empty() {
                doc.events.push(Event {
                    layer: 10,
                    start: s + 0.3,
                    end: e,
                    style: sname("quote"),
                    text: format!(
                        "{{\\an8\\pos({:.0},{:.0})\\fad(250,150)\\i0\\fs{:.0}\\c{}}}— {}",
                        cx,
                        top + b.height + b.size * 0.5,
                        b.size * 0.7,
                        color_tag(&accent),
                        escape_text(att)
                    ),
                });
            }
            out.boxes.push(BoxOut {
                start: s,
                end: e,
                x: cx - b.width / 2.0,
                y: top - b.size * 1.4,
                w: b.width,
                h: total_h + b.size * 1.4,
                text: text.into(),
            });
        }
        "end_card" => {
            let title = inp.text.unwrap_or("Thanks for watching!");
            let font = brand.fonts.title.clone();
            let fe = db.get(&font, false).ok_or_else(|| anyhow::anyhow!("font {font} missing"))?;
            let b = fit(fe, &title.to_uppercase(), u * 9.0, u * 3.0, sw * 0.9, sh * 0.2, 2, u * 0.5, 1.0);
            let mut st = Style::new(&sname("end"), &font, b.size);
            st.primary = txt_color.clone();
            st.outline = u * 0.5;
            st.outline_color = stroke.clone();
            st.shadow = 3.0;
            doc.add_style(st);
            let top = sy + sh * 0.08;
            let cx = sx + sw / 2.0;
            doc.events.push(Event {
                layer: 10,
                start: s,
                end: e,
                style: sname("end"),
                text: format!(
                    "{{\\an8\\pos({:.0},{:.0})\\fad(250,0)}}{}",
                    cx,
                    top,
                    b.lines.iter().map(|l| escape_text(l)).collect::<Vec<_>>().join("\\N")
                ),
            });
            let mut bh_total = b.height;
            if !fr.is_vertical() && p.get("placeholders").and_then(|v| v.as_bool()).unwrap_or(true) {
                // two video placeholders + subscribe circle (YouTube end screen layout)
                let bw = sw * 0.36;
                let bh = bw * 9.0 / 16.0;
                let y = top + b.height + u * 4.0;
                for (i, x) in [sx + sw * 0.06, sx + sw * 0.58].iter().enumerate() {
                    doc.events.push(Event {
                        layer: 9,
                        start: s + 0.15 * i as f64,
                        end: e,
                        style: sname("end"),
                        text: format!(
                            "{{\\an7\\pos({:.0},{:.0})\\fad(250,0)\\p1\\bord{:.0}\\shad0\\1a&HFF&\\3c{}}}{}{{\\p0}}",
                            x,
                            y,
                            u * 0.4,
                            color_tag(&accent),
                            rounded_rect(bw, bh, u * 2.0)
                        ),
                    });
                }
                bh_total = y + bh - top;
            } else if let Some(sub) = inp.subtitle {
                doc.events.push(Event {
                    layer: 10,
                    start: s + 0.3,
                    end: e,
                    style: sname("end"),
                    text: format!(
                        "{{\\an8\\pos({:.0},{:.0})\\fad(250,0)\\fs{:.0}\\c{}}}{}",
                        cx,
                        top + b.height + u * 2.0,
                        b.size * 0.5,
                        color_tag(&accent),
                        escape_text(sub)
                    ),
                });
                bh_total += u * 2.0 + b.size * 0.5;
            }
            out.boxes.push(BoxOut {
                start: s,
                end: e,
                x: sx + sw * 0.05,
                y: top,
                w: sw * 0.9,
                h: bh_total,
                text: title.into(),
            });
        }
        "emoji_pop" => {
            let name = ps(p, "emoji").or(inp.text).unwrap_or("fire");
            let path = cutline_deps::paths::assets_dir().join("emoji").join(format!("{name}.png"));
            if !path.exists() {
                bail!("unknown emoji `{name}` (available: {})", emoji_names().join(", "));
            }
            let w = pf(p, "size").map(|f| f * fr.width as f64).unwrap_or(u * 18.0);
            let (cx, cy) = anchor(inp.position, "center", fr);
            let (cx, cy) = clamp_center(cx, cy, w * 1.1, w * 1.1, fr);
            out.images.push(ImageOut { path, start: s, end: e, cx, cy, w, pop: true });
            out.boxes.push(BoxOut {
                start: s,
                end: e,
                x: cx - w / 2.0,
                y: cy - w / 2.0,
                w,
                h: w,
                text: format!(":{name}:"),
            });
        }
        t => bail!("unknown template `{t}`"),
    }
    Ok(out)
}
