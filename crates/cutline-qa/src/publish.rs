//! Publishing helpers: YouTube chapters and thumbnails.

use anyhow::{Context, Result};
use cutline_core::diag::Diagnostic;
use cutline_core::time::Fps;
use cutline_render::pipeline::{Loaded, Prepared};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Chapter {
    pub at: f64,
    pub title: String,
}

fn yt_tc(t: f64) -> String {
    let s = t.round() as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn prettify(id: &str) -> String {
    let s = id.replace(['_', '-'], " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Chapters from markers, else clips with `chapter`, else every clip (merged to ≥ 10 s).
pub fn chapters(pr: &Prepared) -> (Vec<Chapter>, Vec<Diagnostic>) {
    let tl = &pr.timeline;
    let fps: Fps = tl.fps;
    let mut v: Vec<Chapter> = if !pr.project.markers.is_empty() {
        pr.project
            .markers
            .iter()
            .map(|m| Chapter { at: m.at.seconds(fps, tl.duration()), title: m.title.clone() })
            .collect()
    } else if tl.clips.iter().any(|c| c.clip.chapter.is_some()) {
        tl.clips
            .iter()
            .filter_map(|c| c.clip.chapter.as_ref().map(|t| Chapter { at: tl.secs(c.start), title: t.clone() }))
            .collect()
    } else {
        // every clip (jump-cut pieces share their parent's id prefix), merged so each ≥ 10 s
        let mut out: Vec<Chapter> = vec![];
        for c in &tl.clips {
            let base = c.id.split('~').next().unwrap_or(&c.id).to_string();
            let t = tl.secs(c.start);
            match out.last() {
                Some(l) if prettify(&base) == l.title || t - l.at < 10.0 => {}
                _ => out.push(Chapter { at: t, title: prettify(&base) }),
            }
        }
        out
    };
    v.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap());
    let mut d = vec![];
    if v.first().map(|c| c.at > 0.5).unwrap_or(true) {
        v.insert(0, Chapter { at: 0.0, title: "Intro".into() });
    } else if let Some(f) = v.first_mut() {
        f.at = 0.0;
    }
    if v.len() < 3 {
        d.push(Diagnostic::warning(
            "W_CHAPTERS_TOO_FEW",
            "/markers",
            format!("YouTube needs at least 3 chapters; found {}", v.len()),
        ));
    }
    let dur = tl.duration();
    for (i, c) in v.iter().enumerate() {
        let end = v.get(i + 1).map(|n| n.at).unwrap_or(dur);
        if end - c.at < 10.0 {
            d.push(Diagnostic::warning(
                "W_CHAPTER_TOO_SHORT",
                "/markers",
                format!("chapter \"{}\" at {} is {:.1}s (YouTube minimum 10 s)", c.title, yt_tc(c.at), end - c.at),
            ));
        }
    }
    (v, d)
}

pub fn chapters_text(v: &[Chapter]) -> String {
    v.iter().map(|c| format!("{} {}", yt_tc(c.at), c.title)).collect::<Vec<_>>().join("\n")
}

/// Thumbnail: frame at timeline time `t` (cover-cropped to 1280×720 or 1080×1920), color pop,
/// template text (ASS, bundled fonts) → JPG/PNG.
pub fn thumbnail(l: &Loaded, pr: &Prepared, t: f64, text: Option<&str>, template: &str, out: &Path) -> Result<PathBuf> {
    let tc = cutline_deps::toolchain()?;
    let tl = &pr.timeline;
    let (w, h) = if tl.target.is_vertical() { (1080u32, 1920u32) } else { (1280, 720) };
    let f = tl.fps.secs_to_frames(t.clamp(0.0, tl.duration()));
    let c = tl.clip_at(f).context("no clip at that time")?;
    let m = l.media.get(&c.asset).context("no media")?;
    let local = tl.secs(f - c.start);
    let src_t = (c.src_in + local * c.speed).min(c.src_out - 0.01);
    // face-aware crop when a crop path exists
    let crop = pr.extras.crop_paths.get(&c.id).map(|p| {
        let i = ((local * p.fps) as usize).min(p.len().saturating_sub(1));
        format!("crop={}:{}:{}:{},", p.cw[i], p.ch[i], p.x[i], p.y[i])
    });
    let work = std::env::temp_dir().join(format!("cutline-thumb-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    let mut vf = format!(
        "{}scale={w}:{h}:force_original_aspect_ratio=increase:flags=lanczos:{},crop={w}:{h},eq=saturation=1.15:contrast=1.06",
        crop.unwrap_or_default(),
        cutline_render::segment::color_in(m)
    );
    if let Some(txt) = text {
        let db = cutline_text::FontDb::bundled()?;
        let brand = cutline_render::extras::load_brand(l, &pr.project)?;
        let safe = if tl.target.is_vertical() {
            tl.target.safe_rect()
        } else {
            (w as f64 * 0.05, h as f64 * 0.06, w as f64 * 0.9, h as f64 * 0.88)
        };
        let fr = cutline_text::templates::Frame { width: w, height: h, safe };
        let mut doc = cutline_text::ass::Doc::new(w, h);
        let mut params = serde_json::Map::new();
        // thumbnails want bigger type than on-video titles
        let u = w.min(h) as f64 / 100.0;
        params.insert("size".into(), serde_json::json!(u * if tl.target.is_vertical() { 20.0 } else { 16.0 }));
        let inp = cutline_text::templates::Input {
            template,
            text: Some(txt),
            subtitle: None,
            params: &params,
            start: 0.0,
            end: 10.0,
            position: None,
            index: 0,
        };
        cutline_text::templates::render(&mut doc, &db, &fr, &brand, &inp)?;
        // static thumbnail: drop entrance animations (\fad / \t / \move)
        for e in &mut doc.events {
            e.text = strip_anim(&e.text);
        }
        let ass = work.join("thumb.ass");
        std::fs::write(&ass, doc.render())?;
        let fonts = cutline_deps::paths::assets_dir().join("fonts");
        vf.push_str(&format!(
            ",subtitles=filename={}:fontsdir={}",
            cutline_render::escape::filter_path(&ass),
            cutline_render::escape::filter_path(&fonts)
        ));
        let conf = work.join("fonts.conf");
        std::fs::write(&conf, cutline_render::fontconfig_isolated(&fonts, &work.join("fc")))?;
        std::env::set_var("FONTCONFIG_FILE", &conf);
    }
    let mut cmd = tc.ffmpeg_cmd();
    // a still has one frame and any `-ss` (even 0) makes ffmpeg output nothing
    if c.kind != cutline_core::model::AssetKind::Image {
        cmd.args(["-ss", &format!("{src_t:.6}")]);
    }
    cmd.arg("-i").arg(&m.path).args(["-frames:v", "1", "-vf", &vf, "-q:v", "2"]).arg(out);
    let o = cmd.output()?;
    std::env::remove_var("FONTCONFIG_FILE");
    let _ = std::fs::remove_dir_all(&work);
    anyhow::ensure!(o.status.success(), "thumbnail failed: {}", String::from_utf8_lossy(&o.stderr));
    Ok(out.to_path_buf())
}

fn strip_anim(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(p) = rest.find('{') {
        out.push_str(&rest[..p]);
        let end = rest[p..].find('}').map(|e| p + e).unwrap_or(rest.len() - 1);
        let block = &rest[p..=end];
        let mut cleaned = String::from("{");
        for tag in block.trim_matches(|c| c == '{' || c == '}').split('\\').filter(|t| !t.is_empty()) {
            if tag.starts_with("fad") || tag.starts_with("t(") || tag.starts_with("move") {
                continue;
            }
            // \t(...) may contain nested tags; drop leftovers of the transform
            if tag.contains(')') && !tag.starts_with("pos") && !tag.starts_with("clip") {
                continue;
            }
            if tag.starts_with("fscx") || tag.starts_with("fscy") {
                continue;
            }
            cleaned.push('\\');
            cleaned.push_str(tag);
        }
        cleaned.push('}');
        out.push_str(&cleaned);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strip() {
        assert_eq!(
            strip_anim(
                r"{\an5\pos(1,2)\fad(80,150)\fscx55\fscy55\t(0,140,\fscx110\fscy110)\t(140,240,\fscx100\fscy100)}HI"
            ),
            r"{\an5\pos(1,2)}HI"
        );
        assert_eq!(yt_tc(65.4), "01:05");
        assert_eq!(yt_tc(3725.0), "1:02:05");
    }
}
