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
    /// Cue references resolved while loading (see `cues`).
    pub cues: Vec<crate::cues::ResolvedCue>,
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
    parse_file_with_cues(path).map(|(p, _, _)| p)
}

/// Parse a project file, resolving cue references against the voice-over transcripts.
/// Returns the project, warnings from cue resolution, and the resolved cues.
pub fn parse_file_with_cues(
    path: &Path,
) -> Result<(Project, Vec<Diagnostic>, Vec<crate::cues::ResolvedCue>), LoadError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| LoadError::Io(anyhow::anyhow!("cannot read {}: {e}", path.display())))?;
    let mut v = cutline_core::parse::parse_json5(&text).map_err(LoadError::Invalid)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let beat_notes = crate::beatrefs::resolve(&mut v, dir);
    if has_errors(&beat_notes) {
        return Err(LoadError::Invalid(beat_notes));
    }
    let (mut notes, cues) = crate::cues::resolve(&mut v, dir);
    notes.extend(beat_notes);
    if has_errors(&notes) {
        return Err(LoadError::Invalid(notes));
    }
    let mut p = cutline_core::parse::parse_value(&v).map_err(LoadError::Invalid)?;
    add_brand_bumpers(&mut p, dir);
    Ok((p, notes, cues))
}

/// Rounds allowed for anchored voice layers to settle (each round: substitute cues, resolve).
const CUE_ROUNDS: usize = 6;

/// Load + probe + resolve with the project's own target.
///
/// When cues are measured from a voice layer pinned to a clip (`anchor`), the layer's start
/// depends on the timeline, which may depend on the cues. The cues are then resolved with a
/// provisional start, the timeline is built, and the anchor clip's real start is fed back until
/// it stops moving (normally the second round). If it never settles, the anchor clip's own
/// position depends on those cues (`E_CUE_CYCLE`).
pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    let path = std::fs::canonicalize(path)
        .map_err(|e| LoadError::Io(anyhow::anyhow!("cannot open {}: {e}", path.display())))?;
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let text = std::fs::read_to_string(&path)
        .map_err(|e| LoadError::Io(anyhow::anyhow!("cannot read {}: {e}", path.display())))?;
    let mut v0 = cutline_core::parse::parse_json5(&text).map_err(LoadError::Invalid)?;
    let tc = cutline_deps::toolchain().map_err(LoadError::Tool)?;
    // `bar:`/`beat:` times become seconds first (they depend only on the music layer)
    let beat_notes = crate::beatrefs::resolve(&mut v0, &dir);
    if has_errors(&beat_notes) {
        return Err(LoadError::Invalid(beat_notes));
    }
    let mut starts: std::collections::BTreeMap<usize, f64> = Default::default();
    let mut tapes: Option<std::collections::BTreeMap<String, String>> = None;
    let mut media: Option<ProjectMedia> = None;
    let mut last_anchor = None;
    for _ in 0..CUE_ROUNDS {
        let mut v = v0.clone();
        let (notes, cues, anchored) = crate::cues::resolve_with(&mut v, &dir, &starts);
        if has_errors(&notes) {
            return Err(LoadError::Invalid(notes));
        }
        let mut project = cutline_core::parse::parse_value(&v).map_err(LoadError::Invalid)?;
        add_brand_bumpers(&mut project, &dir);
        // terminal recordings (`.tape` assets) become cached videos before probing (once)
        match &tapes {
            Some(map) => {
                for (id, p) in map {
                    if let Some(a) = project.assets.get_mut(id) {
                        a.path = p.clone();
                    }
                }
            }
            None => {
                let before: Vec<(String, String)> =
                    project.assets.iter().map(|(k, a)| (k.clone(), a.path.clone())).collect();
                let errs = crate::terminal::record_tapes(
                    &mut project,
                    &dir,
                    &crate::cutline_dir(&dir).join("cache").join("terminal"),
                );
                if !errs.is_empty() {
                    return Err(LoadError::Invalid(errs));
                }
                tapes = Some(
                    before
                        .into_iter()
                        .filter_map(|(k, p)| {
                            project.assets.get(&k).filter(|a| a.path != p).map(|a| (k, a.path.clone()))
                        })
                        .collect(),
                );
            }
        }
        if media.is_none() {
            let cache = Cache::new(crate::cutline_dir(&dir).join("cache").join("probe"));
            media = Some(probe_project(tc, &project, &dir, &cache).map_err(LoadError::Tool)?);
        }
        let m = media.as_ref().unwrap();
        let (timeline, mut diags) = resolve(&project, m, &context(&dir));
        let next = anchor_starts(&project, m, &dir, &anchored);
        let settled = next.len() < anchored.len()
            || next.iter().all(|(k, v)| starts.get(k).is_some_and(|s| (s - v).abs() < 1e-6));
        if anchored.is_empty() || settled {
            diags.extend(notes);
            diags.extend(beat_notes);
            return Ok(Loaded { path, dir, project, media: media.unwrap(), diags, timeline, cues });
        }
        last_anchor = anchored.first().map(|a| a.layer);
        starts = next;
    }
    let layer = last_anchor.unwrap_or(0);
    Err(LoadError::Invalid(vec![Diagnostic::error(
        "E_CUE_CYCLE",
        format!("/layers/{layer}/anchor"),
        "the voice layer is anchored to a clip whose position depends on cues from this voice layer, so it never settles",
    )
    .hint("anchor the voice layer to an earlier clip (one not fitted to its cues), or give it a numeric `start`")]))
}

/// Add the brand's `intro` and `outro` around `main` as explicit clips (`generated_by:
/// "brand_intro"` / `"brand_outro"`), each joined to the edit by a fade (`bumper_fade`).
/// Times written in the project keep counting from the start of the edit (see
/// `Timeline::origin`). Does nothing with `brand_bumpers: false`, without a brand intro/outro,
/// or when the clips are already there (an expanded project).
pub fn add_brand_bumpers(p: &mut Project, dir: &Path) {
    use cutline_core::model::{MainItem, BRAND_INTRO, BRAND_OUTRO};
    if p.brand_bumpers == Some(false) || p.main.is_empty() || p.clips().any(|c| c.is_bumper()) {
        return;
    }
    let Ok(brand) = cutline_core::brand::Brand::load(p.brand.as_deref(), dir, &cutline_deps::paths::assets_dir())
    else {
        return; // reported when the brand is loaded for rendering
    };
    let fade = brand.bumper_fade.unwrap_or(0.4).clamp(0.0, 5.0);
    let item = |v: serde_json::Value| -> Option<MainItem> { serde_json::from_value(v).ok() };
    let add = |p: &mut Project, id: &str, file: std::path::PathBuf, front: bool| {
        let asset = serde_json::from_value(serde_json::json!({"path": file.to_string_lossy()}));
        let clip = item(serde_json::json!({"id": id, "clip": id, "generated_by": id}));
        let (Ok(asset), Some(clip)) = (asset, clip) else { return };
        if p.assets.contains_key(id) || p.clips().any(|c| c.id == id) {
            return;
        }
        p.assets.insert(id.to_string(), asset);
        let tr = (fade > 0.0)
            .then(|| item(serde_json::json!({"transition": {"type": "fade", "duration": fade, "generated_by": id}})))
            .flatten();
        if front {
            if let Some(t) = tr {
                p.main.insert(0, t);
            }
            p.main.insert(0, clip);
        } else {
            p.main.extend(tr);
            p.main.push(clip);
        }
    };
    if let Some(f) = brand.file(&brand.intro) {
        add(p, BRAND_INTRO, f, true);
    }
    if let Some(f) = brand.file(&brand.outro) {
        add(p, BRAND_OUTRO, f, false);
    }
}

/// Timeline start (seconds) of each anchored voice layer: its anchor clip's start (+ offset),
/// from the project cut off after that clip, so problems later in the edit (or cues that only
/// fit once the start is right) do not hide it. Layers whose anchor does not resolve are left
/// out (validation reports them).
fn anchor_starts(
    p: &Project,
    media: &ProjectMedia,
    dir: &Path,
    anchored: &[crate::cues::AnchoredVoice],
) -> std::collections::BTreeMap<usize, f64> {
    use cutline_core::model::MainItem;
    let mut out = std::collections::BTreeMap::new();
    for a in anchored {
        let Some(idx) = p.main.iter().position(|m| matches!(m, MainItem::Clip(c) if c.id == a.anchor)) else {
            continue;
        };
        let mut q = p.clone();
        q.main.truncate(idx + 1);
        // its own end does not move its start (and may still use a provisional cue)
        if let Some(MainItem::Clip(c)) = q.main.last_mut() {
            c.end_at = None;
        }
        q.layers.clear();
        q.markers.clear();
        q.captions = None;
        if let (Some(tl), _) = resolve(&q, media, &context(dir)) {
            if let Some(c) = tl.clip_by_id(&a.anchor) {
                out.insert(a.layer, tl.secs(c.start + tl.fps.secs_to_frames(a.offset) - tl.origin));
            }
        }
    }
    out
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
    if e.chroma.is_some() {
        q.chroma = e.chroma;
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
    prepare_for(l, export_name, e, draft, None)
}

/// `prepare` for a render only looked at at `stills` (timeline seconds): see `extras::compute_for`.
pub fn prepare_for(
    l: &Loaded,
    export_name: Option<String>,
    e: &Export,
    draft: bool,
    stills: Option<&[f64]>,
) -> Result<Prepared> {
    let brand = crate::extras::load_brand(l, &l.project)?;
    let (expanded, notes) = crate::auto::expand(l, &l.project, &brand)?;
    let p = for_export(&expanded, e);
    let (tl, mut diags) = resolve(&p, &l.media, &context(&l.dir));
    diags.extend(notes);
    if has_errors(&diags) {
        bail!(LoadError::Invalid(diags));
    }
    let tl = tl.context("timeline did not resolve")?;
    let extras = crate::extras::compute_for(l, &p, &tl, draft, stills)?;
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
