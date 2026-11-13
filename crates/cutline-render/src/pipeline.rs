//! End-to-end orchestration used by the CLI and the selftest:
//! load → probe → resolve → extras (text/captions/reframe/audio) → plan → execute.

use crate::{build_plan, execute, ExecReport, Plan, PlanExtras, RenderOptions};
use anyhow::{bail, Context as _, Result};
use cutline_core::diag::{has_errors, Diagnostic};
use cutline_core::model::{Export, Project};
use cutline_core::timeline::{resolve, Context, Timeline};
use cutline_media::assets::{probe_project, ProjectMedia};
use cutline_media::cache::Cache;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub struct Loaded {
    pub path: PathBuf,
    pub dir: PathBuf,
    pub project: Project,
    pub media: ProjectMedia,
    pub diags: Vec<Diagnostic>,
    pub timeline: Option<Timeline>,
}

#[derive(Debug)]
pub enum LoadError {
    /// Structural (parse/schema) diagnostics.
    Invalid(Vec<Diagnostic>),
    Io(anyhow::Error),
    Tool(anyhow::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Invalid(d) => write!(
                f,
                "invalid project: {}",
                d.iter().map(|x| format!("{} {} {}", x.code, x.path, x.message)).collect::<Vec<_>>().join("; ")
            ),
            LoadError::Io(e) | LoadError::Tool(e) => write!(f, "{e:#}"),
        }
    }
}
impl std::error::Error for LoadError {}

/// Bundled font family names (as libass matches them).
pub fn bundled_fonts() -> Vec<String> {
    cutline_text::FontDb::bundled().map(|d| d.families()).unwrap_or_default()
}

pub fn context(dir: &Path) -> Context {
    Context {
        project_dir: dir.to_path_buf(),
        fonts: bundled_fonts(),
        sfx_aliases: cutline_core::catalog::SFX_ALIASES.iter().map(|s| s.to_string()).collect(),
    }
}

pub fn parse_file(path: &Path) -> Result<Project, LoadError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| LoadError::Io(anyhow::anyhow!("cannot read {}: {e}", path.display())))?;
    cutline_core::parse::parse_project(&text).map_err(LoadError::Invalid)
}

/// Load + probe + resolve with the project's own target.
pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    let path = std::fs::canonicalize(path)
        .map_err(|e| LoadError::Io(anyhow::anyhow!("cannot open {}: {e}", path.display())))?;
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let project = parse_file(&path)?;
    let tc = cutline_deps::toolchain().map_err(LoadError::Tool)?;
    let cache = Cache::new(crate::cutline_dir(&dir).join("cache").join("probe"));
    let media = probe_project(tc, &project, &dir, &cache).map_err(LoadError::Tool)?;
    let (timeline, diags) = resolve(&project, &media, &context(&dir));
    Ok(Loaded { path, dir, project, media, diags, timeline })
}

/// Project variant for one export (target/fps/size override).
pub fn for_export(p: &Project, e: &Export) -> Project {
    let mut q = p.clone();
    q.target = e.target;
    if e.fps.is_some() {
        q.fps = e.fps;
    }
    if e.width.is_some() {
        q.width = e.width;
    }
    if e.height.is_some() {
        q.height = e.height;
    }
    q
}

pub fn export_name(e: &Export) -> String {
    e.name.clone().unwrap_or_else(|| cutline_core::target::target_str(e.target).to_string())
}

/// The exports to render (project.exports or one implicit export of the project target).
pub fn exports(p: &Project) -> Vec<(Option<String>, Export)> {
    if p.exports.is_empty() {
        vec![(None, Export { target: p.target, ..Default::default() })]
    } else if p.exports.len() == 1 {
        vec![(None, p.exports[0].clone())]
    } else {
        p.exports.iter().map(|e| (Some(export_name(e)), e.clone())).collect()
    }
}

/// A resolved export ready to plan.
pub struct Prepared {
    pub export_name: Option<String>,
    pub project: Project,
    pub timeline: Timeline,
    pub diags: Vec<Diagnostic>,
    pub extras: PlanExtras,
}

/// Resolve one export and compute the plan extras.
pub fn prepare(l: &Loaded, export_name: Option<String>, e: &Export, draft: bool) -> Result<Prepared> {
    let brand = crate::extras::load_brand(l, &l.project)?;
    let (expanded, notes) = crate::auto::expand(l, &l.project, &brand)?;
    let p = for_export(&expanded, e);
    let (tl, mut diags) = resolve(&p, &l.media, &context(&l.dir));
    diags.extend(notes);
    if has_errors(&diags) {
        bail!(LoadError::Invalid(diags));
    }
    let tl = tl.context("timeline did not resolve")?;
    let extras = crate::extras::compute(l, &p, &tl, draft)?;
    diags.extend(extras.warnings.clone());
    Ok(Prepared { export_name, project: p, timeline: tl, diags, extras })
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderOutcome {
    pub export: Option<String>,
    pub output: PathBuf,
    pub plan_summary: serde_json::Value,
    pub exec: ExecReport,
}

pub fn plan_for(l: &Loaded, pr: &Prepared, opts: &RenderOptions) -> Result<Plan> {
    let tc = cutline_deps::toolchain()?;
    build_plan(tc, &pr.project, &l.path, &l.media, &pr.timeline, opts, pr.export_name.as_deref(), &pr.extras)
}

pub fn render(l: &Loaded, pr: &Prepared, opts: &RenderOptions) -> Result<(Plan, ExecReport)> {
    let tc = cutline_deps::toolchain()?;
    let plan = plan_for(l, pr, opts)?;
    let rep = execute(tc, &plan, opts)?;
    Ok((plan, rep))
}
