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

pub fn compute(l: &Loaded, p: &Project, tl: &Timeline, _draft: bool) -> Result<PlanExtras> {
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
    let texts: Vec<_> = tl.layers.iter().filter(|l| matches!(l.layer, Layer::Text { .. })).collect();
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

/// Apply `auto` directives, producing an explicit project (M5).
pub fn expand_project(l: &Loaded) -> Result<Project> {
    Ok(l.project.clone())
}
