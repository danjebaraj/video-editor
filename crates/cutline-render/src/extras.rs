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

/// Extra validation warnings that need media analysis (faces, captions in safe zone, …).
pub fn warnings(_l: &Loaded) -> Vec<cutline_core::Diagnostic> {
    vec![]
}

/// Apply `auto` directives, producing an explicit project (M5).
pub fn expand_project(l: &Loaded) -> Result<Project> {
    Ok(l.project.clone())
}
