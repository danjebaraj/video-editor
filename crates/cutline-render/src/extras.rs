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
    compute_for(l, p, tl, draft, None)
}

/// `compute`, for a render that will only be looked at at `stills` (timeline seconds): HTML
/// clips and layers on screen at none of them are captured at 1 fps.
pub fn compute_for(l: &Loaded, p: &Project, tl: &Timeline, draft: bool, stills: Option<&[f64]>) -> Result<PlanExtras> {
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
    extras.watermark = watermark(p, tl, &brand, &mut extras.warnings);
    text_layers(p, tl, &brand, &mut extras)?;
    html_layers(l, p, tl, &brand, &mut extras, draft, stills)?;
    captions(l, p, tl, &brand, &mut extras)?;
    extras.warnings.extend(crate::auto::script_warnings(l, p));
    let full = crate::Geometry { width: tl.target.width, height: tl.target.height, fps: tl.fps };
    let g = if draft { full.draft() } else { full };
    for c in &tl.clips {
        let mut x = ClipExtras {
            pad_color: c
                .clip
                .reframe
                .as_ref()
                .and_then(|r| r.pad_color.as_deref())
                .map(crate::overlay::ff_color)
                .unwrap_or_else(|| pad.clone()),
            chroma444: tl.target.chroma444 && !draft,
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
            x.source_override = Some(html_clip(l, p, tl, c, draft, stills)?);
        }
        if let (Some(fr), Some(m)) = (&c.frame, l.media.get(&c.asset)) {
            let bg = crate::frame::background(fr.background.as_ref(), &brand.colors.background, |id| {
                l.media.get(id).map(|a| (a.path.clone(), a.fingerprint.clone()))
            });
            let zg = crate::segment::zoom_geometry(c, m, &g, &x);
            let src = if x.source_override.is_some() { (zg.width, zg.height) } else { (m.facts.width, m.facts.height) };
            let cache = crate::cutline_dir(&l.dir).join("cache").join("frames");
            x.frame = Some(
                crate::frame::ensure(fr, src, (zg.width, zg.height), &bg, &cache)
                    .map_err(|e| anyhow::anyhow!("clip `{}` frame: {e:#}", c.id))?,
            );
        } else if let Some(path) = crate::reframe::apply(l, p, tl, &g, c, &mut x, &mut extras.warnings)? {
            extras.crop_paths.insert(c.id.clone(), path);
        }
        extras.clip.insert(c.id.clone(), x);
    }
    presenter_focus(l, tl, &mut extras);
    annotations(l, tl, &g, &brand, &mut extras)?;
    Ok(extras)
}

/// The brand logo as a watermark over the edit (not over the intro/outro), unless the project
/// says `watermark: false`.
fn watermark(
    p: &Project,
    tl: &Timeline,
    brand: &Brand,
    warnings: &mut Vec<cutline_core::Diagnostic>,
) -> Option<crate::WatermarkSpec> {
    if p.watermark == Some(false) {
        return None;
    }
    let path = brand.file(&brand.logo)?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !path.exists() || !["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) {
        warnings.push(
            cutline_core::Diagnostic::warning(
                "W_BRAND_LOGO",
                "/brand",
                format!("brand logo {} is missing or not a PNG/JPEG/WebP; no watermark drawn", path.display()),
            )
            .hint("use a PNG with transparency; set `watermark: false` in the project to silence this"),
        );
        return None;
    }
    let wm = brand.watermark.clone().unwrap_or_default();
    let (o, e) = (tl.secs(tl.origin), tl.secs(tl.body_end));
    Some(crate::WatermarkSpec {
        path,
        position: brand.watermark_position.clone().unwrap_or_else(|| "top_right".into()),
        opacity: wm.opacity,
        scale: wm.scale.clamp(0.01, 1.0),
        start: (o + wm.from.unwrap_or(0.0)).min(e),
        end: wm.to.map(|t| (o + t).min(e)).unwrap_or(e),
    })
}

/// Find the face in each presenter camera (a `focus` on the layer wins; without a detector the
/// bubble is centred on the middle of the upper half, where a seated presenter usually is).
fn presenter_focus(l: &Loaded, tl: &Timeline, extras: &mut PlanExtras) {
    use cutline_core::model::Layer;
    for x in &tl.layers {
        let Layer::Presenter { asset, src_in, focus, .. } = &x.layer else { continue };
        if let Some(f) = focus {
            extras.presenter_focus.insert(x.index, (f.x, f.y));
            continue;
        }
        let Some(m) = l.media.get(asset) else { continue };
        let from = src_in.map(|t| t.seconds(tl.fps, m.facts.duration)).unwrap_or(0.0);
        // the first 20 s tell where the presenter sits
        let to = (from + tl.secs(x.end - x.start).min(20.0)).min(m.facts.duration.max(from + 0.1));
        let cache = cutline_media::cache::Cache::new(crate::cutline_dir(&l.dir).join("cache").join("reframe"));
        let key = cutline_media::hash_key(&[
            m.fingerprint.as_bytes(),
            format!("{from:.3}-{to:.3}").as_bytes(),
            b"presenter-v1",
        ]);
        let found: Option<(f64, f64)> = cache.get_json::<Option<(f64, f64)>>(&key).unwrap_or_else(|| {
            let res = cutline_reframe::default_detector().and_then(|mut det| {
                let tc = cutline_deps::toolchain().ok()?;
                let a = cutline_reframe::analyze(tc, &m.path, from, to, false, det.as_mut(), 2.0, 640).ok()?;
                // the largest face of each sampled frame; the median is where the presenter is
                let mut pts: Vec<(f64, f64)> = a
                    .dets
                    .iter()
                    .filter_map(|d| d.faces.iter().max_by(|p, q| (p.w * p.h).partial_cmp(&(q.w * q.h)).unwrap()))
                    .map(|f| (f.cx(), f.cy()))
                    .collect();
                if pts.is_empty() {
                    return None;
                }
                pts.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
                let mx = pts[pts.len() / 2].0;
                pts.sort_by(|p, q| p.1.partial_cmp(&q.1).unwrap());
                Some((mx, pts[pts.len() / 2].1))
            });
            let _ = cache.put_json(&key, &res);
            res
        });
        if let Some(f) = found {
            extras.presenter_focus.insert(x.index, f);
        }
    }
}

/// Rasterize annotation layers into cached alpha videos.
fn annotations(l: &Loaded, tl: &Timeline, g: &crate::Geometry, brand: &Brand, extras: &mut PlanExtras) -> Result<()> {
    if !tl.layers.iter().any(|x| x.layer.is_annotation() || x.layer.is_card()) {
        return Ok(());
    }
    let cards = tl.layers.iter().any(|x| x.layer.is_card()).then(|| {
        let assets = l.media.assets.iter().map(|(id, m)| (id.clone(), m.path.clone())).collect();
        crate::cards::CardCtx::new(brand, assets)
    });
    let k = g.scale_to(tl.target.width);
    let (sx, sy, sw, sh) = tl.target.safe_rect();
    let font = cutline_text::FontDb::bundled().ok().and_then(|db| db.get("Inter", false).map(|f| f.font.clone()));
    let st = crate::annot::Style {
        accent: crate::overlay::rgb_of(&brand.colors.accent),
        safe: (sx * k, sy * k, sw * k, sh * k),
        k,
        font,
        cards: cards.flatten(),
        // cards draw brand fonts, colors, the logo and image assets
        look_key: format!(
            "{} {} {:?}",
            serde_json::to_string(brand).unwrap_or_default(),
            brand.file(&brand.logo).and_then(|p| cutline_media::fingerprint(&p).ok()).unwrap_or_default(),
            l.media
                .assets
                .values()
                .filter(|m| m.facts.kind == cutline_core::model::AssetKind::Image)
                .map(|m| m.fingerprint.as_str())
                .collect::<Vec<_>>()
        ),
    };
    // QA: cards count as on-screen text from their start (hook check); the box is the safe zone
    for x in tl.layers.iter().filter(|x| x.layer.is_card()) {
        if let cutline_core::model::Layer::Text { text, .. } = &x.layer {
            extras.text_boxes.push(crate::TextBox {
                start: tl.secs(x.start),
                end: tl.secs(x.end),
                x: sx,
                y: sy,
                w: sw,
                h: sh,
                text: text.clone().unwrap_or_default(),
                kind: "card".into(),
            });
        }
    }
    let cache = crate::cutline_dir(&l.dir).join("cache").join("annot");
    extras.annot = crate::annot::render(cutline_deps::toolchain()?, tl, g, &st, &cache)?;
    Ok(())
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
        .filter(|l| !l.layer.is_card())
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
    if source == CaptionSource::Auto && cw.is_empty() && !crate::auto::speech_assets(p, l).is_empty() {
        let why = match cutline_transcribe::whisper::available(&crate::auto::whisper_model()) {
            Err(e) => format!("{e:#}"),
            Ok(_) => "no words were recognized".into(),
        };
        extras.warnings.push(
            cutline_core::Diagnostic::warning(
                "W_CAPTIONS_NO_TRANSCRIPT",
                "/captions",
                format!("captions are `auto` but no transcript is available: {why}"),
            )
            .hint("run `cutline transcribe <file>` (or `cutline deps install whisper-model`), or set an asset `transcript`"),
        );
    }
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

/// HTML capture settings: drafts and previews capture at half pixel density (same CSS layout)
/// and at most 15 fps, which cuts Chrome's frame capture, the slowest part of a cold preview,
/// by about 8×. Final renders capture every frame at full size.
pub fn html_capture(tl: &Timeline, draft: bool, span: (i64, i64), stills: Option<&[f64]>) -> (f64, f64) {
    let fps = tl.fps.as_f64();
    let unseen = stills.is_some_and(|ts| {
        let (a, b) = (tl.secs(span.0), tl.secs(span.1));
        !ts.iter().any(|t| *t >= a - 0.5 && *t <= b + 0.5)
    });
    if unseen {
        (0.5, 1.0)
    } else if draft {
        (0.5, fps.min(15.0))
    } else {
        (1.0, fps)
    }
}

/// Render an HTML/SVG/Lottie main clip to an opaque mezzanine.
fn html_clip(
    l: &Loaded,
    p: &Project,
    tl: &Timeline,
    c: &cutline_core::timeline::TClip,
    draft: bool,
    stills: Option<&[f64]>,
) -> Result<std::path::PathBuf> {
    let (device_scale, fps) = html_capture(tl, draft, (c.start, c.end()), stills);
    let a = &p.assets[&c.asset];
    let m = l.media.get(&c.asset).ok_or_else(|| anyhow::anyhow!("no media for {}", c.asset))?;
    let spec = cutline_html::RenderSpec {
        kind: html_kind(m.facts.kind),
        source: m.path.clone(),
        width: a.width.unwrap_or(tl.target.width),
        height: a.height.unwrap_or(tl.target.height),
        fps,
        duration: tl.secs(c.frames),
        alpha: false,
        params: a.params.clone().unwrap_or(serde_json::json!({})),
        allow_dirs: vec![l.dir.clone()],
        device_scale,
    };
    cutline_html::render_cached(cutline_deps::toolchain()?, &spec, &html_cache(l))
}

/// Render HTML/SVG/Lottie layers and HTML text templates to alpha mezzanines.
pub fn html_layers(
    l: &Loaded,
    p: &Project,
    tl: &Timeline,
    brand: &Brand,
    extras: &mut PlanExtras,
    draft: bool,
    stills: Option<&[f64]>,
) -> Result<()> {
    use cutline_core::model::Layer;
    let tc = cutline_deps::toolchain()?;
    for tlay in &tl.layers {
        let (device_scale, fps) = html_capture(tl, draft, (tlay.start, tlay.end), stills);
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
                    fps,
                    duration: dur,
                    alpha: a.alpha.unwrap_or(true),
                    params,
                    allow_dirs: vec![l.dir.clone()],
                    device_scale,
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
                    fps,
                    duration: dur,
                    alpha: true,
                    params: serde_json::Value::Object(pm),
                    allow_dirs: vec![],
                    device_scale,
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
                // the box is a rough guess (the template lays itself out in Chrome)
                kind: "text_html".into(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod capture_tests {
    use super::html_capture;
    use cutline_core::model::AssetKind;
    use cutline_core::time::Fps;
    use cutline_core::timeline::{resolve, Context, MediaFacts, StaticLookup, Timeline};

    fn timeline() -> Timeline {
        let mut m = StaticLookup::default();
        let f = MediaFacts {
            exists: true,
            kind: AssetKind::Video,
            duration: 20.0,
            has_video: true,
            has_audio: false,
            alpha: false,
            width: 1920,
            height: 1080,
            fps: Some(Fps::new(30, 1)),
            rotation: 0,
        };
        m.0.insert("v".into(), f);
        let p = cutline_core::parse::parse_project(
            "{version: 1, target: 'long_1080p', fps: 30, assets: {v: {path: 'v.mp4'}}, main: [{id: 'c', clip: 'v'}]}",
        )
        .unwrap();
        resolve(&p, &m, &Context::default()).0.unwrap()
    }

    #[test]
    fn drafts_capture_half_density_at_15_fps() {
        let tl = timeline();
        assert_eq!(html_capture(&tl, false, (0, 300), None), (1.0, 30.0));
        assert_eq!(html_capture(&tl, true, (0, 300), None), (0.5, 15.0));
    }

    #[test]
    fn stills_skip_frame_capture_for_unseen_spans() {
        let tl = timeline();
        // span 0–10 s; a still at 4 s sees it, stills at 12 and 15 s do not
        assert_eq!(html_capture(&tl, true, (0, 300), Some(&[4.0])), (0.5, 15.0));
        assert_eq!(html_capture(&tl, true, (0, 300), Some(&[12.0, 15.0])), (0.5, 1.0));
    }
}
