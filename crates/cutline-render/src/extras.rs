//! Compute plan extras from other subsystems (brand, text, captions, reframe, speech).

use crate::pipeline::Loaded;
use crate::segment::ClipExtras;
use crate::PlanExtras;
use anyhow::Result;
use cutline_core::brand::Brand;
use cutline_core::model::Project;
use cutline_core::timeline::Timeline;
use std::path::PathBuf;

pub fn load_brand(l: &Loaded, p: &Project) -> Result<Brand> {
    Brand::load(p.brand.as_deref(), &l.dir, &cutline_deps::paths::assets_dir())
}

/// Resolve a LUT name (bundled) or path (relative to the project).
pub fn lut_path(name: &str, l: &Loaded) -> Option<PathBuf> {
    if cutline_core::catalog::BUNDLED_LUTS.contains(&name) {
        let p = cutline_deps::paths::assets_dir().join("luts").join(format!("{name}.cube"));
        return p.exists().then_some(p);
    }
    let p = l.dir.join(name);
    p.exists().then_some(p)
}

pub fn compute(l: &Loaded, p: &Project, tl: &Timeline, draft: bool) -> Result<PlanExtras> {
    let brand = load_brand(l, p)?;
    let pad = crate::overlay::ff_color(&brand.colors.background);
    let mut extras = PlanExtras {
        pad_color: pad.clone(),
        accent: crate::overlay::ff_color(&brand.colors.accent),
        fonts_dir: Some(cutline_deps::paths::assets_dir().join("fonts")),
        sfx_files: brand.sfx_files(),
        luts_dir: Some(cutline_deps::paths::assets_dir().join("luts")),
        ..Default::default()
    };
    text_layers(p, tl, &brand, &mut extras)?;
    html_layers(l, p, tl, &brand, &mut extras)?;
    captions(l, p, tl, &brand, &mut extras)?;
    for c in &tl.clips {
        let mut x = ClipExtras {
            pad_color: c
                .clip
                .reframe
                .as_ref()
                .and_then(|r| r.pad_color.as_deref())
                .map(crate::overlay::ff_color)
                .unwrap_or_else(|| pad.clone()),
            ..Default::default()
        };
        let lut = c
            .clip
            .color
            .as_ref()
            .and_then(|cg| cg.lut.clone())
            .or_else(|| c.clip.color.as_ref().and(brand.lut.clone()));
        if let Some(name) = lut {
            x.lut_path = lut_path(&name, l);
        }
        if matches!(
            c.kind,
            cutline_core::model::AssetKind::Html
                | cutline_core::model::AssetKind::Svg
                | cutline_core::model::AssetKind::Lottie
        ) {
            x.source_override = Some(html_clip(l, p, tl, c)?);
        }
        let full = crate::Geometry { width: tl.target.width, height: tl.target.height, fps: tl.fps };
        let g = if draft { full.draft() } else { full };
        if let Some(path) = crate::reframe::apply(l, p, tl, &g, c, &mut x, &mut extras.warnings)? {
            extras.crop_paths.insert(c.id.clone(), path);
        }
        extras.clip.insert(c.id.clone(), x);
    }
    Ok(extras)
}

pub fn frame_of(tl: &Timeline) -> cutline_text::templates::Frame {
    cutline_text::templates::Frame { width: tl.target.width, height: tl.target.height, safe: tl.target.safe_rect() }
}

/// Lay out text-template layers into one ASS document + QA boxes + emoji overlays.
pub fn text_layers(_p: &Project, tl: &Timeline, brand: &Brand, extras: &mut PlanExtras) -> Result<()> {
    use cutline_core::model::Layer;
    use cutline_text::templates::{render, Input};
    let texts: Vec<_> = tl
        .layers
        .iter()
        .filter(|l| matches!(&l.layer, Layer::Text { template, .. } if !template.ends_with("_html")))
        .collect();
    if texts.is_empty() {
        return Ok(());
    }
    let db = cutline_text::FontDb::bundled()?;
    let fr = frame_of(tl);
    let mut doc = cutline_text::ass::Doc::new(fr.width, fr.height);
    let empty = serde_json::Map::new();
    for l in texts {
        let Layer::Text { template, text, subtitle, params, look, .. } = &l.layer else { continue };
        let inp = Input {
            template,
            text: text.as_deref(),
            subtitle: subtitle.as_deref(),
            params: params.as_ref().unwrap_or(&empty),
            start: tl.secs(l.start),
            end: tl.secs(l.end),
            position: look.position.as_ref(),
            index: l.index,
        };
        let out = render(&mut doc, &db, &fr, brand, &inp)
            .map_err(|e| anyhow::anyhow!("layer {} ({template}): {e}", l.index))?;
        for b in out.boxes {
            extras.text_boxes.push(crate::TextBox {
                start: b.start,
                end: b.end,
                x: b.x,
                y: b.y,
                w: b.w,
                h: b.h,
                text: b.text,
                kind: "text".into(),
            });
        }
        for im in out.images {
            extras.extra_images.push(crate::ExtraImage {
                path: im.path,
                start: im.start,
                end: im.end,
                cx: im.cx,
                cy: im.cy,
                w: im.w,
                pop: im.pop,
            });
        }
    }
    if !doc.events.is_empty() {
        extras.ass_docs.push(("text.ass".into(), doc.render()));
    }
    Ok(())
}

/// Extra validation warnings that need media analysis (faces, captions in safe zone, …).
pub fn warnings(_l: &Loaded) -> Vec<cutline_core::Diagnostic> {
    vec![]
}

/// Apply `auto` directives, producing an explicit project.
pub fn expand_project(l: &Loaded) -> Result<Project> {
    let brand = load_brand(l, &l.project)?;
    Ok(crate::auto::expand(l, &l.project, &brand)?.0)
}

/// Captions, speech spans (ducking) and SRT.
pub fn captions(l: &Loaded, p: &Project, tl: &Timeline, brand: &Brand, extras: &mut PlanExtras) -> Result<()> {
    use cutline_core::model::CaptionSource;
    use cutline_text::captions::{render, srt, CWord, CaptionCfg};
    let cap = p.captions.clone();
    let source = cap.as_ref().map(|c| c.source).unwrap_or(CaptionSource::None);
    let allow_whisper = source == CaptionSource::Auto;
    let words = crate::auto::timeline_words(l, p, tl, brand, allow_whisper)?;
    extras.speech = cutline_transcribe::mapping::speech_spans(&words, 0.3);
    let cuts = crate::auto::cut_times(tl);
    let mut cw: Vec<CWord> = words.iter().map(|w| CWord { w: w.w.clone(), start: w.start, end: w.end }).collect();
    if source == CaptionSource::File {
        let f = l.dir.join(cap.as_ref().and_then(|c| c.file.clone()).unwrap_or_default());
        cw = load_caption_file(&f)?;
    }
    extras.srt = srt(&cw, &cuts);
    if source == CaptionSource::None || cw.is_empty() {
        return Ok(());
    }
    let c = cap.unwrap();
    let cfg = CaptionCfg {
        style: c.style.clone().unwrap_or_default(),
        position: c.position.clone(),
        max_words: c.max_words_per_line.map(|v| v as usize),
        max_chars: c.max_chars_per_line.map(|v| v as usize),
        max_lines: c.max_lines.map(|v| v as usize),
        emphasis: c.emphasis.clone(),
        emoji: c.emoji.clone(),
        font: c.font.clone(),
        size: c.size,
        color: c.color.clone(),
        accent: c.accent_color.clone(),
        uppercase: c.uppercase,
    };
    let db = cutline_text::FontDb::bundled()?;
    let fr = frame_of(tl);
    let mut doc = cutline_text::ass::Doc::new(fr.width, fr.height);
    let out = render(&mut doc, &db, &fr, brand, &cw, &cfg, &cuts, 1.0 / tl.fps.as_f64())?;
    for b in out.boxes {
        extras.text_boxes.push(crate::TextBox {
            start: b.start,
            end: b.end,
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
            text: b.text,
            kind: "caption".into(),
        });
    }
    for im in out.images {
        extras.extra_images.push(crate::ExtraImage {
            path: im.path,
            start: im.start,
            end: im.end,
            cx: im.cx,
            cy: im.cy,
            w: im.w,
            pop: im.pop,
        });
    }
    if !doc.events.is_empty() {
        extras.ass_docs.push(("captions.ass".into(), doc.render()));
    }
    Ok(())
}

/// Caption file in timeline time: SRT (cue text spread evenly over the cue) or transcript JSON.
pub fn load_caption_file(f: &std::path::Path) -> Result<Vec<cutline_text::captions::CWord>> {
    use cutline_text::captions::CWord;
    let text = std::fs::read_to_string(f).map_err(|e| anyhow::anyhow!("caption file {}: {e}", f.display()))?;
    if text.trim_start().starts_with('{') {
        let t = cutline_transcribe::scribe::load_any(&text, "captions")?;
        return Ok(t.words.into_iter().map(|w| CWord { w: w.w, start: w.start, end: w.end }).collect());
    }
    let parse_tc = |s: &str| -> Option<f64> {
        let s = s.trim().replace(',', ".");
        let p: Vec<&str> = s.split(':').collect();
        if p.len() != 3 {
            return None;
        }
        Some(p[0].parse::<f64>().ok()? * 3600.0 + p[1].parse::<f64>().ok()? * 60.0 + p[2].parse::<f64>().ok()?)
    };
    let mut out = vec![];
    for block in text.replace("\r", "").split("\n\n") {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        let Some(ti) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let (a, b) = lines[ti].split_once("-->").unwrap();
        let (Some(a), Some(b)) = (parse_tc(a), parse_tc(b.split_whitespace().next().unwrap_or(""))) else { continue };
        let words: Vec<&str> = lines[ti + 1..].iter().flat_map(|l| l.split_whitespace()).collect();
        let n = words.len().max(1) as f64;
        for (k, w) in words.iter().enumerate() {
            out.push(CWord {
                w: w.to_string(),
                start: a + (b - a) * k as f64 / n,
                end: a + (b - a) * (k + 1) as f64 / n,
            });
        }
    }
    Ok(out)
}

fn html_kind(k: cutline_core::model::AssetKind) -> cutline_html::Kind {
    match k {
        cutline_core::model::AssetKind::Svg => cutline_html::Kind::Svg,
        cutline_core::model::AssetKind::Lottie => cutline_html::Kind::Lottie,
        _ => cutline_html::Kind::Html,
    }
}

fn html_cache(l: &Loaded) -> std::path::PathBuf {
    crate::cutline_dir(&l.dir).join("cache").join("html")
}

/// Render an HTML/SVG/Lottie main clip to an opaque mezzanine.
fn html_clip(l: &Loaded, p: &Project, tl: &Timeline, c: &cutline_core::timeline::TClip) -> Result<std::path::PathBuf> {
    let a = &p.assets[&c.asset];
    let m = l.media.get(&c.asset).ok_or_else(|| anyhow::anyhow!("no media for {}", c.asset))?;
    let spec = cutline_html::RenderSpec {
        kind: html_kind(m.facts.kind),
        source: m.path.clone(),
        width: a.width.unwrap_or(tl.target.width),
        height: a.height.unwrap_or(tl.target.height),
        fps: tl.fps.as_f64(),
        duration: tl.secs(c.frames),
        alpha: false,
        params: a.params.clone().unwrap_or(serde_json::json!({})),
        allow_dirs: vec![l.dir.clone()],
        device_scale: 1.0,
    };
    cutline_html::render_cached(cutline_deps::toolchain()?, &spec, &html_cache(l))
}

/// Render HTML/SVG/Lottie layers and HTML text templates to alpha mezzanines.
pub fn html_layers(l: &Loaded, p: &Project, tl: &Timeline, brand: &Brand, extras: &mut PlanExtras) -> Result<()> {
    use cutline_core::model::Layer;
    let tc = cutline_deps::toolchain()?;
    for tlay in &tl.layers {
        let dur = tl.secs(tlay.end - tlay.start);
        let spec = match &tlay.layer {
            Layer::Html { asset, .. } => {
                let a = &p.assets[asset];
                let m = l.media.get(asset).ok_or_else(|| anyhow::anyhow!("no media for {asset}"))?;
                let mut params = a.params.clone().unwrap_or(serde_json::json!({}));
                if let Some(o) = params.as_object_mut() {
                    o.entry("duration").or_insert(serde_json::json!(dur));
                }
                cutline_html::RenderSpec {
                    kind: html_kind(m.facts.kind),
                    source: m.path.clone(),
                    width: a.width.unwrap_or(tl.target.width),
                    height: a.height.unwrap_or(tl.target.height),
                    fps: tl.fps.as_f64(),
                    duration: dur,
                    alpha: a.alpha.unwrap_or(true),
                    params,
                    allow_dirs: vec![l.dir.clone()],
                    device_scale: 1.0,
                }
            }
            Layer::Text { template, text, subtitle, params, .. } if template.ends_with("_html") => {
                let src = cutline_deps::paths::assets_dir().join("templates").join(format!("{template}.html"));
                let mut pm = serde_json::Map::new();
                pm.insert("text".into(), serde_json::json!(text));
                pm.insert("subtitle".into(), serde_json::json!(subtitle));
                pm.insert("accent".into(), serde_json::json!(brand.colors.accent));
                pm.insert("color".into(), serde_json::json!(brand.colors.text));
                pm.insert("duration".into(), serde_json::json!(dur));
                pm.insert("vertical".into(), serde_json::json!(tl.target.is_vertical()));
                if let Some(extra) = params {
                    for (k, v) in extra {
                        pm.insert(k.clone(), v.clone());
                    }
                }
                cutline_html::RenderSpec {
                    kind: cutline_html::Kind::Html,
                    source: src,
                    width: tl.target.width,
                    height: tl.target.height,
                    fps: tl.fps.as_f64(),
                    duration: dur,
                    alpha: true,
                    params: serde_json::Value::Object(pm),
                    allow_dirs: vec![],
                    device_scale: 1.0,
                }
            }
            _ => continue,
        };
        let mov = cutline_html::render_cached(tc, &spec, &html_cache(l))
            .map_err(|e| anyhow::anyhow!("layer {}: {e:#}", tlay.index))?;
        extras.html_renders.insert(tlay.index, mov);
        if let Layer::Text { text, .. } = &tlay.layer {
            // QA: count as on-screen text from its start (hook check)
            let (sx, sy, sw, sh) = tl.target.safe_rect();
            extras.text_boxes.push(crate::TextBox {
                start: tl.secs(tlay.start),
                end: tl.secs(tlay.end),
                x: sx,
                y: sy,
                w: sw,
                h: sh * 0.3,
                text: text.clone().unwrap_or_default(),
                kind: "text".into(),
            });
        }
    }
    Ok(())
}
