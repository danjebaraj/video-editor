//! The Markdown spec (spec §5): `media:` links, the outline, structured blocks, live lint and
//! the New Project templates. Everything here is plain text analysis; the editor is in the shell.
//!
//! Structured blocks are ordinary Markdown so Claude reads them without any tooling:
//! - `/scene`: a heading that starts with "Scene", followed by `- **Field:** value` lines.
//! - `/vo`: a fenced block tagged `vo` holding the text to voice.
//! - `/shot`: a list line `- **Shot:** ![label](media:<id>) in 0:05, out 0:12 — notes`.
//! - `/music`: a fenced block tagged `music` with `key: value` lines.
//! - `/cta`: a "Call to action" heading and its text.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// A `media:<id>` reference in the spec.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaRef {
    pub id: String,
    /// 1-based line number.
    pub line: usize,
    /// Byte range of `media:<id>` in the spec.
    pub start: usize,
    pub end: usize,
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Every `media:<id>` in the spec, in order. Both `![alt](media:id)` links and bare mentions count.
pub fn media_refs(spec: &str) -> Vec<MediaRef> {
    let mut out = Vec::new();
    let mut line = 1;
    let mut search_from = 0;
    let bytes = spec.as_bytes();
    let mut newlines_counted_to = 0;
    while let Some(pos) = spec[search_from..].find("media:") {
        let start = search_from + pos;
        // must not be the tail of another word (e.g. "multimedia:")
        let boundary = start == 0 || !is_id_char(spec[..start].chars().next_back().unwrap_or(' '));
        let id: String = spec[start + 6..].chars().take_while(|c| is_id_char(*c)).collect();
        search_from = start + 6;
        if !boundary || id.is_empty() {
            continue;
        }
        line += bytes[newlines_counted_to..start].iter().filter(|b| **b == b'\n').count();
        newlines_counted_to = start;
        let end = start + 6 + id.len();
        out.push(MediaRef { id, line, start, end });
    }
    out
}

/// Replaces `media:<id>` link targets using `resolve`; unknown IDs are left as they are.
pub fn rewrite_media_links(spec: &str, resolve: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(spec.len());
    let mut last = 0;
    for r in media_refs(spec) {
        out.push_str(&spec[last..r.start]);
        match resolve(&r.id) {
            Some(path) => {
                // inside a Markdown link, a path with spaces or parentheses needs angle brackets
                let in_link = spec[..r.start].ends_with('(');
                if in_link && path.contains([' ', '(', ')']) {
                    out.push('<');
                    out.push_str(&path);
                    out.push('>');
                } else {
                    out.push_str(&path);
                }
            }
            None => out.push_str(&spec[r.start..r.end]),
        }
        last = r.end;
    }
    out.push_str(&spec[last..]);
    out
}

/// A line of the spec with what kind of block it is in.
struct Line<'a> {
    number: usize,
    text: &'a str,
    /// The info string of the fenced block this line is inside (`""` for an untagged fence).
    fence: Option<&'a str>,
    /// True for the ``` lines themselves.
    is_fence_marker: bool,
}

fn lines(spec: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut fence: Option<(&str, usize, char)> = None;
    for (i, text) in spec.lines().enumerate() {
        let trimmed = text.trim_start();
        let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~');
        let run = marker.map(|m| trimmed.chars().take_while(|c| *c == m).count()).unwrap_or(0);
        match (fence, marker) {
            (None, Some(m)) if run >= 3 => {
                let info = trimmed[run..].trim();
                let info = info.split_whitespace().next().unwrap_or("");
                fence = Some((info, run, m));
                out.push(Line { number: i + 1, text, fence: Some(info), is_fence_marker: true });
            }
            (Some((info, open, m0)), Some(m)) if m == m0 && run >= open && trimmed[run..].trim().is_empty() => {
                fence = None;
                out.push(Line { number: i + 1, text, fence: Some(info), is_fence_marker: true });
            }
            (Some((info, _, _)), _) => {
                out.push(Line { number: i + 1, text, fence: Some(info), is_fence_marker: false })
            }
            (None, _) => out.push(Line { number: i + 1, text, fence: None, is_fence_marker: false }),
        }
    }
    out
}

fn heading(text: &str) -> Option<(u8, &str)> {
    let t = text.trim_start();
    let level = t.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) || !t[level..].starts_with([' ', '\t']) {
        return None;
    }
    Some((level as u8, t[level..].trim().trim_end_matches('#').trim()))
}

fn is_scene_title(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    t == "scene" || t.starts_with("scene ") || t.starts_with("scene:") || t.starts_with("scene-")
}

fn is_cta_title(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    t.contains("call to action") || t == "cta" || t.starts_with("cta ") || t.starts_with("cta:")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutlineItem {
    pub level: u8,
    pub title: String,
    /// 1-based line number.
    pub line: usize,
    /// `heading` or `scene`.
    pub kind: String,
}

/// Headings and scenes for the sidebar outline.
pub fn outline(spec: &str) -> Vec<OutlineItem> {
    lines(spec)
        .iter()
        .filter(|l| l.fence.is_none())
        .filter_map(|l| {
            let (level, title) = heading(l.text)?;
            Some(OutlineItem {
                level,
                title: title.to_string(),
                line: l.number,
                kind: if is_scene_title(title) { "scene" } else { "heading" }.to_string(),
            })
        })
        .collect()
}

/// `- **Duration:** 5 s`, `- Duration: 5s`, `* **VO:** text` → (`duration`, `5 s`).
fn field(text: &str) -> Option<(String, &str)> {
    let t = text.trim_start();
    let t = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ "))?;
    let (key, value) = t.split_once(':')?;
    let key = key.replace('*', "").replace('_', " ").trim().to_ascii_lowercase();
    if key.is_empty() || key.len() > 24 || key.contains(['[', '(', '`']) {
        return None;
    }
    Some((key, value.trim().trim_start_matches('*').trim()))
}

/// `5 s`, `5s`, `45 seconds`, `0:30`, `1:05:00`, `1m 20s`, `2 min`.
pub fn parse_duration(text: &str) -> Option<f64> {
    let t = text.trim().to_ascii_lowercase();
    let t = t.trim_start_matches(['~', '≈']).trim();
    let head: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == ':' || *c == '.').collect();
    if head.contains(':') {
        let mut total = 0.0;
        for part in head.split(':') {
            total = total * 60.0 + part.parse::<f64>().ok()?;
        }
        return Some(total);
    }
    let mut total = 0.0;
    let mut found = false;
    let mut num = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            continue;
        }
        if num.is_empty() {
            if c.is_whitespace() {
                continue;
            }
            break;
        }
        let mut unit = String::new();
        if c.is_ascii_alphabetic() {
            unit.push(c);
        }
        while let Some(n) = chars.peek().copied() {
            if n.is_ascii_alphabetic() {
                unit.push(n);
                chars.next();
            } else if unit.is_empty() && n.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        let v: f64 = num.parse().ok()?;
        num.clear();
        match unit.as_str() {
            "s" | "sec" | "secs" | "second" | "seconds" => total += v,
            "m" | "min" | "mins" | "minute" | "minutes" => total += v * 60.0,
            "h" | "hr" | "hour" | "hours" => total += v * 3600.0,
            _ => return found.then_some(total),
        }
        found = true;
    }
    if !num.is_empty() {
        // a bare number is seconds
        total += num.parse::<f64>().ok()?;
        found = true;
    }
    found.then_some(total)
}

fn is_placeholder_value(v: &str) -> bool {
    let v = v.trim();
    v.is_empty() || (v.starts_with("{{") && v.ends_with("}}"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scene {
    pub title: String,
    pub line: usize,
    pub duration_s: Option<f64>,
    pub has_media: bool,
    pub has_narration: bool,
    pub has_text: bool,
}

/// Scene blocks: each "Scene …" heading with the lines up to the next heading of its level or higher.
pub fn scenes(spec: &str) -> Vec<Scene> {
    let ls = lines(spec);
    let mut out: Vec<Scene> = Vec::new();
    let mut current: Option<(Scene, u8)> = None;
    for l in &ls {
        if l.fence.is_none() {
            if let Some((level, title)) = heading(l.text) {
                if let Some((s, lvl)) = current.take() {
                    if level > lvl {
                        current = Some((s, lvl)); // a sub-heading stays inside the scene
                    } else {
                        out.push(s);
                    }
                }
                if is_scene_title(title) {
                    if let Some((s, _)) = current.take() {
                        out.push(s);
                    }
                    current = Some((
                        Scene {
                            title: title.to_string(),
                            line: l.number,
                            duration_s: None,
                            has_media: false,
                            has_narration: false,
                            has_text: false,
                        },
                        level,
                    ));
                }
                continue;
            }
        }
        let Some((scene, _)) = current.as_mut() else { continue };
        if l.is_fence_marker {
            continue;
        }
        match l.fence {
            Some("vo") => scene.has_narration |= !is_placeholder_value(l.text),
            Some(_) => {}
            None => {
                if !media_refs(l.text).is_empty() {
                    scene.has_media = true;
                }
                if let Some((key, value)) = field(l.text) {
                    let filled = !is_placeholder_value(value);
                    match key.as_str() {
                        "duration" | "length" => scene.duration_s = parse_duration(value),
                        "media" | "shot" | "footage" | "b-roll" | "broll" => scene.has_media |= filled,
                        "vo" | "voice-over" | "voice over" | "voiceover" | "narration" => scene.has_narration |= filled,
                        "on-screen text" | "on screen text" | "text" | "caption" | "title" => scene.has_text |= filled,
                        _ => {}
                    }
                }
            }
        }
    }
    if let Some((s, _)) = current {
        out.push(s);
    }
    out
}

/// One voice-over passage with where it starts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoiceOver {
    pub text: String,
    pub line: usize,
}

/// The text to be voiced: every `vo` block and every `VO:` / `Voice-over:` field.
pub fn voice_overs(spec: &str) -> Vec<VoiceOver> {
    let mut out: Vec<VoiceOver> = Vec::new();
    let mut block: Option<VoiceOver> = None;
    for l in lines(spec) {
        match (l.fence, l.is_fence_marker) {
            (Some("vo"), true) => {
                if let Some(b) = block.take() {
                    if !b.text.trim().is_empty() {
                        out.push(VoiceOver { text: b.text.trim().to_string(), line: b.line });
                    }
                } else {
                    block = Some(VoiceOver { text: String::new(), line: l.number + 1 });
                }
            }
            (Some("vo"), false) => {
                if let Some(b) = block.as_mut() {
                    if !is_placeholder_value(l.text) {
                        b.text.push_str(l.text);
                        b.text.push('\n');
                    }
                }
            }
            (None, _) => {
                if let Some((key, value)) = field(l.text) {
                    if matches!(key.as_str(), "vo" | "voice-over" | "voice over" | "voiceover" | "narration")
                        && !is_placeholder_value(value)
                    {
                        out.push(VoiceOver { text: value.to_string(), line: l.number });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

pub fn word_count(text: &str) -> usize {
    text.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count()
}

/// Seconds needed to speak `words` at `wpm`.
pub fn speaking_seconds(words: usize, wpm: u32) -> f64 {
    words as f64 / wpm.max(1) as f64 * 60.0
}

/// True when the spec has a call to action: a heading, a `cta` block, or a `CTA:` field.
pub fn has_call_to_action(spec: &str) -> bool {
    lines(spec).iter().any(|l| match l.fence {
        Some("cta") => !l.is_fence_marker && !is_placeholder_value(l.text),
        Some(_) => false,
        None => {
            heading(l.text).map(|(_, t)| is_cta_title(t)).unwrap_or(false)
                || field(l.text)
                    .map(|(k, v)| (k == "cta" || k == "call to action") && !is_placeholder_value(v))
                    .unwrap_or(false)
        }
    })
}

/// Placeholder text left in the spec: `{{…}}` from templates, TODO/TBD/FIXME markers, lorem ipsum.
pub fn placeholders(spec: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for l in lines(spec) {
        if l.is_fence_marker || matches!(l.fence, Some(f) if !matches!(f, "vo" | "music" | "cta" | "mermaid")) {
            continue;
        }
        let t = l.text;
        let mut found: Option<String> = None;
        if let Some(a) = t.find("{{") {
            if let Some(b) = t[a..].find("}}") {
                found = Some(t[a..a + b + 2].to_string());
            }
        }
        if found.is_none() {
            let lower = t.to_ascii_lowercase();
            if lower.contains("lorem ipsum") {
                found = Some("lorem ipsum".into());
            } else {
                for marker in ["TODO", "TBD", "FIXME", "XXX"] {
                    let hit = t.match_indices(marker).any(|(i, _)| {
                        let before = t[..i].chars().next_back().map(|c| !c.is_alphanumeric()).unwrap_or(true);
                        let after = t[i + marker.len()..].chars().next().map(|c| !c.is_alphanumeric()).unwrap_or(true);
                        before && after
                    });
                    if hit {
                        found = Some(marker.to_string());
                        break;
                    }
                }
            }
        }
        if let Some(f) = found {
            let f = if f.chars().count() > 60 { format!("{}…", f.chars().take(60).collect::<String>()) } else { f };
            out.push((l.number, f));
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LintIssue {
    /// `unresolved_media`, `scene_empty`, `vo_too_long`, `placeholder`, `scene_length`.
    pub rule: String,
    /// `warning` or `info`.
    pub severity: String,
    pub message: String,
    /// 1-based line to jump to.
    pub line: usize,
}

pub struct LintContext<'a> {
    pub media_ids: &'a HashSet<String>,
    pub target_length_s: Option<f64>,
    pub words_per_minute: u32,
}

/// Live, non-blocking lint (spec §5).
pub fn lint(spec: &str, ctx: &LintContext) -> Vec<LintIssue> {
    let mut out = Vec::new();
    let mut reported: HashSet<(String, usize)> = HashSet::new();
    for r in media_refs(spec) {
        if !ctx.media_ids.contains(&r.id) && reported.insert((r.id.clone(), r.line)) {
            out.push(LintIssue {
                rule: "unresolved_media".into(),
                severity: "warning".into(),
                message: format!("media:{} isn't in the project's media list", r.id),
                line: r.line,
            });
        }
    }
    let all_scenes = scenes(spec);
    for s in &all_scenes {
        if !s.has_media && !s.has_narration && !s.has_text {
            out.push(LintIssue {
                rule: "scene_empty".into(),
                severity: "warning".into(),
                message: format!("\"{}\" has no media, narration or on-screen text", s.title),
                line: s.line,
            });
        }
    }
    let vos = voice_overs(spec);
    let words: usize = vos.iter().map(|v| word_count(&v.text)).sum();
    if let Some(target) = ctx.target_length_s.filter(|t| *t > 0.0) {
        let need = speaking_seconds(words, ctx.words_per_minute);
        if words > 0 && need > target {
            out.push(LintIssue {
                rule: "vo_too_long".into(),
                severity: "warning".into(),
                message: format!("The voice-over is {words} words, about {need:.0} s at {} words per minute; the target length is {target:.0} s", ctx.words_per_minute),
                line: vos.first().map(|v| v.line).unwrap_or(1),
            });
        }
        let planned: f64 = all_scenes.iter().filter_map(|s| s.duration_s).sum();
        if planned > 0.0 && (planned - target).abs() > target * 0.2 {
            out.push(LintIssue {
                rule: "scene_length".into(),
                severity: "info".into(),
                message: format!("Scene durations add up to {planned:.0} s; the target length is {target:.0} s"),
                line: all_scenes.first().map(|s| s.line).unwrap_or(1),
            });
        }
    }
    for (line, text) in placeholders(spec) {
        out.push(LintIssue {
            rule: "placeholder".into(),
            severity: "warning".into(),
            message: format!("Placeholder text: {text}"),
            line,
        });
    }
    out.sort_by_key(|i| i.line);
    out
}

// MARK: Templates and slash blocks

/// `(id, title)` of the New Project templates, in menu order.
pub const TEMPLATES: [(&str, &str); 10] = [
    ("marketing", "Marketing"),
    ("product_demo", "Product demo"),
    ("tutorial", "Tutorial"),
    ("explainer", "Explainer"),
    ("social_clip", "Social clip"),
    ("testimonial", "Testimonial"),
    ("event_recap", "Event recap"),
    ("internal_update", "Internal update"),
    ("app_store_preview", "App Store preview"),
    ("blank", "Blank"),
];

pub fn template(id: &str) -> Option<&'static str> {
    Some(match id {
        "marketing" => include_str!("../data/templates/marketing.md"),
        "product_demo" => include_str!("../data/templates/product_demo.md"),
        "tutorial" => include_str!("../data/templates/tutorial.md"),
        "explainer" => include_str!("../data/templates/explainer.md"),
        "social_clip" => include_str!("../data/templates/social_clip.md"),
        "testimonial" => include_str!("../data/templates/testimonial.md"),
        "event_recap" => include_str!("../data/templates/event_recap.md"),
        "internal_update" => include_str!("../data/templates/internal_update.md"),
        "app_store_preview" => include_str!("../data/templates/app_store_preview.md"),
        "blank" => include_str!("../data/templates/blank.md"),
        _ => return None,
    })
}

/// A template with its title placeholder filled in.
pub fn template_for(id: &str, project_name: &str) -> String {
    let text = template(id).or_else(|| template("blank")).unwrap_or_default();
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let title = match first.find("{{").zip(first.find("}}")) {
        Some((a, b)) if b > a => format!("{}{}{}", &first[..a], project_name, &first[b + 2..]),
        _ => first.to_string(),
    };
    let rest: Vec<&str> = lines.collect();
    format!("{title}\n{}\n", rest.join("\n")).replace("\n\n\n", "\n\n")
}

/// The structured blocks offered as slash commands: `(command, title, snippet)`.
pub const SLASH_BLOCKS: [(&str, &str, &str); 5] = [
    ("scene", "Scene", "## Scene: {{name}}\n- **Duration:** {{5 s}}\n- **Media:** {{media or shot description}}\n- **On-screen text:** {{text}}\n\n```vo\n{{voice-over for this scene}}\n```\n"),
    ("vo", "Voice-over", "```vo\n{{text to be voiced}}\n```\n"),
    ("shot", "Shot", "- **Shot:** {{![label](media:id)}} in {{0:00}}, out {{0:05}} — {{notes}}\n"),
    ("music", "Music brief", "```music\nmood: {{calm, confident}}\ntempo: {{slow | medium | fast}}\nlength: {{match the video}}\nsource: {{generated | media:id}}\n```\n"),
    ("cta", "Call to action", "## Call to action\n{{What the viewer should do next, and where.}}\n"),
];

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = "# Launch teaser\n\nIntro ![hero](media:m-hero) and multimedia: no.\n\n## Scene 1: Hook\n- **Duration:** 4 s\n- **Media:** ![hero](media:m-hero)\n- **On-screen text:** Ship faster\n\n```vo\nMeet the fastest way to ship.\n```\n\n## Scene 2: Empty\n- **Duration:** 0:10\n- **Media:** {{drag a clip}}\n\n### Notes\nStill scene two. See media:m-missing.\n\n## Call to action\nStart a free trial.\n\n```js\n# not a heading TODO media:m-code\n```\n";

    #[test]
    fn finds_media_refs_with_lines() {
        let refs = media_refs(SPEC);
        let ids: Vec<(&str, usize)> = refs.iter().map(|r| (r.id.as_str(), r.line)).collect();
        assert_eq!(ids, vec![("m-hero", 3), ("m-hero", 7), ("m-missing", 19), ("m-code", 25)]);
        assert_eq!(&SPEC[refs[0].start..refs[0].end], "media:m-hero");
    }

    #[test]
    fn rewrites_links_for_export() {
        let out = rewrite_media_links("![a](media:m-a) ![b](media:m-b) see media:m-a", |id| {
            (id == "m-a").then(|| "/My Footage/a (1).mov".to_string())
        });
        assert_eq!(out, "![a](</My Footage/a (1).mov>) ![b](media:m-b) see /My Footage/a (1).mov");
    }

    #[test]
    fn outline_marks_scenes_and_skips_code() {
        let o = outline(SPEC);
        let v: Vec<(u8, &str, &str)> = o.iter().map(|i| (i.level, i.title.as_str(), i.kind.as_str())).collect();
        assert_eq!(
            v,
            vec![
                (1, "Launch teaser", "heading"),
                (2, "Scene 1: Hook", "scene"),
                (2, "Scene 2: Empty", "scene"),
                (3, "Notes", "heading"),
                (2, "Call to action", "heading")
            ]
        );
        assert_eq!(o[1].line, 5);
    }

    #[test]
    fn parses_scenes() {
        let s = scenes(SPEC);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].duration_s, s[0].has_media, s[0].has_narration, s[0].has_text), (Some(4.0), true, true, true));
        assert_eq!(s[1].duration_s, Some(10.0));
        assert!(s[1].has_media, "the media: mention under the sub-heading belongs to scene 2");
        assert!(!s[1].has_narration);
    }

    #[test]
    fn durations() {
        for (t, v) in [
            ("5 s", 5.0),
            ("5s", 5.0),
            ("45 seconds", 45.0),
            ("0:30", 30.0),
            ("1:05", 65.0),
            ("1m 20s", 80.0),
            ("2 min", 120.0),
            ("~8 s", 8.0),
            ("12", 12.0),
            ("1:00:00", 3600.0),
        ] {
            assert_eq!(parse_duration(t), Some(v), "{t}");
        }
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration("{{5 s}}"), None);
    }

    #[test]
    fn voice_over_and_cta() {
        let v = voice_overs(SPEC);
        assert_eq!(v, vec![VoiceOver { text: "Meet the fastest way to ship.".into(), line: 11 }]);
        assert_eq!(word_count(&v[0].text), 6);
        assert!(has_call_to_action(SPEC));
        assert!(!has_call_to_action("# Title\n\nNo ask here."));
        assert!(has_call_to_action("- **CTA:** Book a demo"));
        assert!(!has_call_to_action("- **CTA:** {{what next}}"));
        let inline = voice_overs("- **VO:** Hello there world\n- VO: {{later}}\n");
        assert_eq!(inline.len(), 1);
        assert!((speaking_seconds(300, 150) - 120.0).abs() < 1e-9);
    }

    #[test]
    fn lint_reports_each_rule() {
        let ids: HashSet<String> = ["m-hero".to_string()].into_iter().collect();
        let long_vo = format!("{SPEC}\n```vo\n{}\n```\n", "word ".repeat(200));
        let issues =
            lint(&long_vo, &LintContext { media_ids: &ids, target_length_s: Some(30.0), words_per_minute: 150 });
        let rules: Vec<(&str, usize)> = issues.iter().map(|i| (i.rule.as_str(), i.line)).collect();
        assert!(rules.contains(&("unresolved_media", 19)), "{rules:?}");
        assert!(rules.contains(&("unresolved_media", 25)));
        assert!(rules.contains(&("placeholder", 16)));
        assert!(rules.iter().any(|(r, _)| *r == "vo_too_long"));
        assert!(rules.iter().any(|(r, _)| *r == "scene_length"));
        assert!(!rules.iter().any(|(r, l)| *r == "placeholder" && *l == 25), "TODO inside a code block is ignored");
        // a scene with nothing in it
        let empty = lint(
            "## Scene 1\n- **Duration:** 5 s\n\n## Scene 2\n- **On-screen text:** Hi\n",
            &LintContext { media_ids: &ids, target_length_s: None, words_per_minute: 150 },
        );
        assert_eq!(empty.iter().filter(|i| i.rule == "scene_empty").map(|i| i.line).collect::<Vec<_>>(), vec![1]);
        // a clean spec has no issues
        let clean =
            "# T\n\n## Scene 1\n- **Duration:** 5 s\n- **Media:** ![h](media:m-hero)\n\n## Call to action\nGo.\n";
        assert!(
            lint(clean, &LintContext { media_ids: &ids, target_length_s: Some(5.0), words_per_minute: 150 }).is_empty()
        );
    }

    #[test]
    fn placeholder_markers_need_word_boundaries() {
        assert_eq!(
            placeholders("Our TODO app\nTODOs\nlorem IPSUM dolor\n[TBD]").iter().map(|p| p.0).collect::<Vec<_>>(),
            vec![1, 3, 4]
        );
    }

    #[test]
    fn templates_exist_and_take_the_project_name() {
        for (id, _) in TEMPLATES {
            let t = template(id).unwrap_or_else(|| panic!("missing template {id}"));
            assert!(t.starts_with("# "), "{id}");
            if id != "blank" {
                assert!(!scenes(t).is_empty(), "{id} has scenes");
            }
        }
        let t = template_for("marketing", "Spring launch");
        assert!(t.starts_with("# Spring launch: marketing video\n"), "{}", t.lines().next().unwrap());
        assert!(template_for("nope", "X").starts_with("# X\n"));
        for (_, _, snippet) in SLASH_BLOCKS {
            assert!(snippet.ends_with('\n'));
        }
        assert_eq!(scenes(SLASH_BLOCKS[0].2).len(), 1);
    }
}
