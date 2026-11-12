//! Compute plan extras from other subsystems (brand, text, captions, reframe, speech).

use crate::pipeline::Loaded;
use crate::PlanExtras;
use anyhow::Result;
use cutline_core::model::Project;
use cutline_core::timeline::Timeline;

pub fn compute(_l: &Loaded, _p: &Project, _tl: &Timeline, _draft: bool) -> Result<PlanExtras> {
    Ok(PlanExtras {
        pad_color: "0x101014".into(),
        accent: "0xFFC400".into(),
        fonts_dir: Some(cutline_deps::paths::assets_dir().join("fonts")),
        ..Default::default()
    })
}

/// Extra validation warnings that need media analysis (faces, captions in safe zone, …).
pub fn warnings(_l: &Loaded) -> Vec<cutline_core::Diagnostic> {
    vec![]
}

/// Apply `auto` directives, producing an explicit project (M5).
pub fn expand_project(l: &Loaded) -> Result<Project> {
    Ok(l.project.clone())
}
