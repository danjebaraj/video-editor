//! The `.cutproj` project file (spec §16): model, load/save, migration, duplicate, templates.
//!
//! JSON, UTF-8, LF, keys in declaration order, written atomically. Unknown top-level keys are
//! kept so a newer app's additions survive a round trip through an older one.

use std::collections::{BTreeMap, HashSet};

use camino::{Utf8Path, Utf8PathBuf};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::brand::BrandRef;
use crate::media::MediaItem;
use crate::presets::OutputConfig;
use crate::{ids, paths, timeutil, Error, Result};

pub const FORMAT: &str = "video-studio-project";
pub const VERSION: u32 = 1;
pub const EXTENSION: &str = "cutproj";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Project {
    pub format: String,
    pub version: u32,
    /// Stable ID; Duplicate assigns a new one.
    pub id: String,
    pub name: String,
    pub created: String,
    pub modified: String,
    #[serde(default)]
    pub intent: Intent,
    /// The video spec as plain Markdown.
    #[serde(default)]
    pub spec: String,
    #[serde(default)]
    pub media: Vec<MediaItem>,
    /// Folders searched for moved media, relative to the project file or absolute.
    #[serde(default)]
    pub media_roots: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<BrandRef>,
    #[serde(default)]
    pub models: Models,
    #[serde(default)]
    pub output_configs: Vec<OutputConfig>,
    #[serde(default)]
    pub run_configs: Vec<RunConfig>,
    #[serde(default)]
    pub runs: Vec<RunRecord>,
    #[serde(default)]
    pub assistant_sessions: Vec<AssistantSessionRef>,
    /// Extra folders (absolute paths) the spec assistant may read: briefs, docs, source material.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assistant_folders: Vec<String>,
    /// Whether production runs may drive and record apps, and which ones.
    #[serde(default, skip_serializing_if = "AppCapture::is_default")]
    pub app_capture: AppCapture,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// App recording: during a production run Claude Code can open a desktop app or a web app,
/// operate it and record the screen, to make footage the spec asks for.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AppCapture {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// When not empty, the only apps a run may drive (names as shown in Applications). Empty
    /// means any app except terminals, system settings, password managers and Finder.
    #[serde(default)]
    pub apps: Vec<String>,
}

impl Default for AppCapture {
    fn default() -> Self {
        AppCapture { enabled: true, apps: Vec::new() }
    }
}

impl AppCapture {
    pub fn is_default(&self) -> bool {
        *self == AppCapture::default()
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IntentKind {
    Marketing,
    ProductDemo,
    Tutorial,
    Explainer,
    SocialClip,
    Testimonial,
    Announcement,
    EventRecap,
    Internal,
    AppStorePreview,
    Other,
    /// Not chosen yet. Validation reports this as a blocker.
    #[default]
    Unset,
}

impl IntentKind {
    pub const ALL: [IntentKind; 11] = [
        IntentKind::Marketing,
        IntentKind::ProductDemo,
        IntentKind::Tutorial,
        IntentKind::Explainer,
        IntentKind::SocialClip,
        IntentKind::Testimonial,
        IntentKind::Announcement,
        IntentKind::EventRecap,
        IntentKind::Internal,
        IntentKind::AppStorePreview,
        IntentKind::Other,
    ];

    pub fn key(self) -> &'static str {
        match self {
            IntentKind::Marketing => "marketing",
            IntentKind::ProductDemo => "product_demo",
            IntentKind::Tutorial => "tutorial",
            IntentKind::Explainer => "explainer",
            IntentKind::SocialClip => "social_clip",
            IntentKind::Testimonial => "testimonial",
            IntentKind::Announcement => "announcement",
            IntentKind::EventRecap => "event_recap",
            IntentKind::Internal => "internal",
            IntentKind::AppStorePreview => "app_store_preview",
            IntentKind::Other => "other",
            IntentKind::Unset => "unset",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            IntentKind::Marketing => "Marketing",
            IntentKind::ProductDemo => "Product demo",
            IntentKind::Tutorial => "Tutorial or how-to",
            IntentKind::Explainer => "Explainer",
            IntentKind::SocialClip => "Social clip",
            IntentKind::Testimonial => "Testimonial",
            IntentKind::Announcement => "Announcement",
            IntentKind::EventRecap => "Event recap",
            IntentKind::Internal => "Internal communication",
            IntentKind::AppStorePreview => "App store preview",
            IntentKind::Other => "Other",
            IntentKind::Unset => "Not set",
        }
    }

    /// `cutline init --style` for this intent (spec §7).
    pub fn cutline_style(self) -> &'static str {
        match self {
            IntentKind::SocialClip | IntentKind::Testimonial | IntentKind::EventRecap => "creator",
            _ => "business",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Intent {
    #[serde(default)]
    pub kind: IntentKind,
    /// Free text when `kind` is `other`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_length_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cta: Option<String>,
    #[serde(default)]
    pub locales: Vec<String>,
}

impl Intent {
    pub fn describe(&self) -> String {
        match (self.kind, &self.other) {
            (IntentKind::Other, Some(o)) if !o.trim().is_empty() => o.trim().to_string(),
            (k, _) => k.label().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Models {
    /// Spec assistant model: a CLI alias (`sonnet`) or a full model ID.
    pub assistant: String,
    /// Dry run model; cheaper than production by default.
    pub dry_run: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default)]
    pub web_access: bool,
}

impl Default for Models {
    fn default() -> Self {
        Models { assistant: "sonnet".into(), dry_run: "haiku".into(), effort: None, web_access: false }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Draft,
    #[default]
    Final,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RunConfig {
    pub id: String,
    pub name: String,
    /// IDs of the selected output configs.
    #[serde(default)]
    pub outputs: Vec<String>,
    pub production_model: String,
    #[serde(default)]
    pub quality: Quality,
    #[serde(default)]
    pub locales: Vec<String>,
    /// Parent folder for this config's run folders; `~` is expanded. Empty means the default.
    #[serde(default)]
    pub output_folder: String,
    #[serde(default = "default_credit_cap")]
    pub eleven_credit_cap: u64,
    #[serde(default = "yes")]
    pub dry_run_first: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_spend_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub web_access: bool,
    /// Seconds a question may wait before Claude proceeds on its stated assumption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_timeout_s: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

fn default_credit_cap() -> u64 {
    20_000
}
fn yes() -> bool {
    true
}

impl RunConfig {
    pub fn new(name: &str) -> Self {
        RunConfig {
            id: ids::new_id("rc"),
            name: name.to_string(),
            outputs: Vec::new(),
            production_model: "opus".into(),
            quality: Quality::Final,
            locales: Vec::new(),
            output_folder: String::new(),
            eleven_credit_cap: default_credit_cap(),
            dry_run_first: true,
            max_spend_usd: None,
            max_turns: None,
            web_access: false,
            question_timeout_s: None,
            effort: None,
        }
    }

    /// The parent folder of this config's runs (spec §9.2).
    pub fn output_root(&self, project_name: &str, default_root: Option<&str>) -> Utf8PathBuf {
        if !self.output_folder.trim().is_empty() {
            return paths::expand_tilde(self.output_folder.trim());
        }
        let root = match default_root {
            Some(r) if !r.trim().is_empty() => paths::expand_tilde(r.trim()),
            _ => paths::default_output_root(),
        };
        root.join(safe_file_name(project_name))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    DryRun,
    Production,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Succeeded,
    SucceededWithWarnings,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            RunStatus::Running => "Running",
            RunStatus::Succeeded => "Succeeded",
            RunStatus::SucceededWithWarnings => "Succeeded with warnings",
            RunStatus::Failed => "Failed",
            RunStatus::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RunRecord {
    pub id: String,
    pub run_config: String,
    /// The run config's name when the run started (configs can be renamed or deleted later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_config_name: Option<String>,
    pub kind: RunKind,
    pub folder: String,
    pub status: RunStatus,
    pub started: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session: Option<String>,
    /// Model ID actually used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The run this one revises.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eleven_credits: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AssistantSessionRef {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Replaces characters that can't be used in a file name on macOS or Windows.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String =
        name.trim()
            .chars()
            .map(|c| {
                if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                    '-'
                } else {
                    c
                }
            })
            .collect();
    let cleaned = cleaned.trim_matches(|c| c == '.' || c == ' ').to_string();
    if cleaned.is_empty() {
        "Untitled".into()
    } else {
        cleaned
    }
}

impl Project {
    pub fn new(name: &str) -> Self {
        let now = timeutil::now_rfc3339();
        Project {
            format: FORMAT.into(),
            version: VERSION,
            id: ids::new_id("prj"),
            name: name.to_string(),
            created: now.clone(),
            modified: now,
            intent: Intent::default(),
            spec: String::new(),
            media: Vec::new(),
            media_roots: Vec::new(),
            brand: None,
            models: Models::default(),
            output_configs: Vec::new(),
            run_configs: Vec::new(),
            runs: Vec::new(),
            assistant_sessions: Vec::new(),
            assistant_folders: Vec::new(),
            app_capture: AppCapture::default(),
            extra: BTreeMap::new(),
        }
    }

    /// Parses project JSON, migrating older versions.
    pub fn from_json(text: &str) -> Result<Self> {
        let mut value: Value =
            serde_json::from_str(text).map_err(|e| Error::InvalidProject(format!("not valid JSON: {e}")))?;
        migrate(&mut value)?;
        let mut p: Project =
            serde_json::from_value(value).map_err(|e| Error::InvalidProject(format!("not a valid project: {e}")))?;
        p.normalize();
        Ok(p)
    }

    pub fn load(path: &Utf8Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Io(std::io::Error::new(e.kind(), format!("{path}: {e}"))))?;
        Self::from_json(&text).map_err(|e| match e {
            Error::InvalidProject(m) => Error::InvalidProject(format!("{path}: {m}")),
            other => other,
        })
    }

    /// Canonical JSON text: pretty, LF, trailing newline.
    pub fn to_json(&self) -> Result<String> {
        let s = paths::to_pretty_json(self)?;
        Ok(s.replace("\r\n", "\n"))
    }

    /// Writes the project atomically. `modified` is updated first.
    pub fn save(&mut self, path: &Utf8Path) -> Result<()> {
        self.modified = timeutil::now_rfc3339();
        self.normalize();
        paths::write_atomic(path, self.to_json()?.as_bytes())
    }

    /// Fixes up invariants after loading or editing: unique IDs, LF in the spec, dangling refs.
    pub fn normalize(&mut self) {
        if self.spec.contains('\r') {
            self.spec = self.spec.replace("\r\n", "\n").replace('\r', "\n");
        }
        let mut seen = HashSet::new();
        for m in &mut self.media {
            if m.id.is_empty() || !seen.insert(m.id.clone()) {
                m.id = ids::unique_slug_id("m", m.file_name().unwrap_or("media"), &seen);
                seen.insert(m.id.clone());
            }
        }
        let outputs: HashSet<&str> = self.output_configs.iter().map(|o| o.id.as_str()).collect();
        for rc in &mut self.run_configs {
            rc.outputs.retain(|o| outputs.contains(o.as_str()));
        }
    }

    pub fn media_by_id(&self, id: &str) -> Option<&MediaItem> {
        self.media.iter().find(|m| m.id == id)
    }

    pub fn run_config(&self, id_or_name: &str) -> Option<&RunConfig> {
        self.run_configs
            .iter()
            .find(|r| r.id == id_or_name)
            .or_else(|| self.run_configs.iter().find(|r| r.name.eq_ignore_ascii_case(id_or_name)))
            .or_else(|| self.run_configs.iter().find(|r| ids::slug(&r.name) == ids::slug(id_or_name)))
    }

    pub fn output_config(&self, id: &str) -> Option<&OutputConfig> {
        self.output_configs.iter().find(|o| o.id == id)
    }

    /// Adds or replaces a run record by ID.
    pub fn upsert_run(&mut self, record: RunRecord) {
        match self.runs.iter_mut().find(|r| r.id == record.id) {
            Some(r) => *r = record,
            None => self.runs.push(record),
        }
    }

    /// Takes run history changes from the copy on disk (the CLI may have appended runs while the
    /// app has the project open). Returns true if anything changed.
    pub fn merge_runs_from(&mut self, disk: &Project) -> bool {
        let mut changed = false;
        for r in &disk.runs {
            match self.runs.iter_mut().find(|x| x.id == r.id) {
                Some(mine) => {
                    // A finished record on disk wins over a running one in memory.
                    if mine != r && (mine.status == RunStatus::Running || r.finished.is_some()) {
                        *mine = r.clone();
                        changed = true;
                    }
                }
                None => {
                    self.runs.push(r.clone());
                    changed = true;
                }
            }
        }
        if changed {
            self.runs.sort_by(|a, b| a.started.cmp(&b.started));
        }
        changed
    }

    /// The same project without the fields that change outside editing.
    fn content_view(&self) -> Project {
        let mut p = self.clone();
        p.runs.clear();
        p.modified.clear();
        p
    }
}

/// How the file on disk differs from the copy the app last read or wrote (spec §16 autosave).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalChange {
    None,
    /// Only run history changed: merge silently.
    RunsOnly,
    /// Something else changed: offer Reload, Keep Mine or Compare when there are unsaved edits.
    Other,
}

pub fn external_change(base: &Project, disk: &Project) -> ExternalChange {
    if base.content_view() != disk.content_view() {
        ExternalChange::Other
    } else if base.runs != disk.runs {
        ExternalChange::RunsOnly
    } else {
        ExternalChange::None
    }
}

/// Upgrades older project JSON in place. Version 1 is the first format, so this only checks.
fn migrate(value: &mut Value) -> Result<()> {
    let obj =
        value.as_object_mut().ok_or_else(|| Error::InvalidProject("the project file must be a JSON object".into()))?;
    match obj.get("format").and_then(Value::as_str) {
        Some(FORMAT) => {}
        Some(other) => {
            return Err(Error::InvalidProject(format!("unknown format \"{other}\" (expected \"{FORMAT}\")")))
        }
        None => return Err(Error::InvalidProject(format!("missing \"format\": \"{FORMAT}\""))),
    }
    let version = obj.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version > VERSION as u64 {
        return Err(Error::InvalidProject(format!(
            "this project was saved by a newer version (format {version}); this app reads up to format {VERSION}"
        )));
    }
    if version == 0 {
        // Pre-release files had no version and used `production_model` at the top level.
        if let Some(m) = obj.remove("production_model") {
            if let Some(rcs) = obj.get_mut("run_configs").and_then(Value::as_array_mut) {
                for rc in rcs {
                    if let Some(o) = rc.as_object_mut() {
                        o.entry("production_model").or_insert(m.clone());
                    }
                }
            }
        }
        obj.insert("version".into(), Value::from(1));
    }
    let now = timeutil::now_rfc3339();
    for key in ["created", "modified"] {
        if !obj.get(key).map(Value::is_string).unwrap_or(false) {
            obj.insert(key.into(), Value::from(now.clone()));
        }
    }
    if !obj.get("id").map(Value::is_string).unwrap_or(false) {
        obj.insert("id".into(), Value::from(ids::new_id("prj")));
    }
    if !obj.get("name").map(Value::is_string).unwrap_or(false) {
        obj.insert("name".into(), Value::from("Untitled"));
    }
    Ok(())
}

/// The JSON Schema of the project file.
pub fn json_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(Project)).unwrap_or(Value::Null)
}

// MARK: Sidecar (pasted images, spec §5)

/// `<name>.cutproj.assets/` next to the project file.
pub fn sidecar_dir(project_path: &Utf8Path) -> Utf8PathBuf {
    let name = project_path.file_name().unwrap_or("project.cutproj");
    project_path.with_file_name(format!("{name}.assets"))
}

/// Stores a pasted image in the sidecar as `<sha256>.png` (at most 4096 px on the longest side)
/// and returns its path. Pasting the same picture twice stores it once.
pub fn store_pasted_image(project_path: &Utf8Path, bytes: &[u8]) -> Result<Utf8PathBuf> {
    use sha2::{Digest, Sha256};
    let img =
        image::load_from_memory(bytes).map_err(|e| Error::other(format!("the pasted image couldn't be read: {e}")))?;
    let img = if img.width().max(img.height()) > 4096 {
        img.resize(4096, 4096, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| Error::other(format!("the pasted image couldn't be converted to PNG: {e}")))?;
    let hash = Sha256::digest(&png);
    let name = format!("{}.png", hash.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let dir = sidecar_dir(project_path);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    if !path.exists() {
        paths::write_atomic(&path, &png)?;
    }
    Ok(path)
}

/// Sidecar images no media item used by the spec refers to (File › Clean Up Pasted Images…).
pub fn unused_sidecar_images(project: &Project, project_path: &Utf8Path) -> Vec<Utf8PathBuf> {
    let dir = sidecar_dir(project_path);
    let used_ids: HashSet<String> = crate::spec::media_refs(&project.spec).into_iter().map(|r| r.id).collect();
    let base = project_path.parent().unwrap_or(Utf8Path::new("."));
    let used_files: HashSet<Utf8PathBuf> = project
        .media
        .iter()
        .filter(|m| used_ids.contains(&m.id))
        .filter_map(|m| m.candidate_paths(base).into_iter().next())
        .map(|p| paths::canonical_or_normal(&p))
        .collect();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if let Ok(p) = paths::utf8(e.path()) {
                if p.extension().map(|x| x.eq_ignore_ascii_case("png")).unwrap_or(false)
                    && !used_files.contains(&paths::canonical_or_normal(&p))
                {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

// MARK: Duplicate, Save As, templates, export

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct DuplicateOptions {
    #[serde(default)]
    pub keep_runs: bool,
    #[serde(default)]
    pub keep_sessions: bool,
}

fn copy_dir(from: &Utf8Path, to: &Utf8Path) -> Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for e in walkdir::WalkDir::new(from).min_depth(1) {
        let e = e.map_err(|e| Error::other(e.to_string()))?;
        let p = paths::utf8(e.path().to_path_buf())?;
        let rel = p.strip_prefix(from).map_err(|e| Error::other(e.to_string()))?;
        let dest = to.join(rel);
        if e.file_type().is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else {
            std::fs::copy(&p, &dest)?;
        }
    }
    Ok(())
}

/// Rewrites media paths for a project file that now lives at `new_path`: relative paths are
/// recomputed, and items in the old sidecar point at the copied sidecar.
fn rebase_media(project: &mut Project, old_path: Option<&Utf8Path>, new_path: &Utf8Path) {
    let new_dir = new_path.parent().unwrap_or(Utf8Path::new(".")).to_owned();
    let old_dir = old_path.and_then(|p| p.parent()).map(|p| p.to_owned());
    let old_sidecar = old_path.map(sidecar_dir);
    let new_sidecar = sidecar_dir(new_path);
    for m in &mut project.media {
        let mut abs = m.paths.absolute.clone();
        if abs.is_none() {
            if let (Some(rel), Some(od)) = (&m.paths.relative, &old_dir) {
                abs = Some(paths::absolutize(Utf8Path::new(rel), od).to_string());
            }
        }
        if let (Some(a), Some(os)) = (&abs, &old_sidecar) {
            if let Ok(rest) = Utf8Path::new(a).strip_prefix(os) {
                abs = Some(new_sidecar.join(rest).to_string());
                m.paths.bookmark_macos = None;
            }
        }
        if let Some(a) = &abs {
            m.paths.relative = paths::relative_to(Utf8Path::new(a), &new_dir);
        }
        m.paths.absolute = abs;
    }
    if let Some(od) = &old_dir {
        project.media_roots = project
            .media_roots
            .iter()
            .map(|r| {
                let p = Utf8Path::new(r);
                if p.is_absolute() || r.starts_with('~') {
                    r.clone()
                } else {
                    paths::relative_to(&paths::absolutize(p, od), &new_dir).unwrap_or_else(|| r.clone())
                }
            })
            .collect();
    }
}

/// File › Duplicate: copies the project and its sidecar under a new ID (spec §16).
pub fn duplicate(src_path: &Utf8Path, dst_path: &Utf8Path, opts: DuplicateOptions) -> Result<Project> {
    let mut p = Project::load(src_path)?;
    p.id = ids::new_id("prj");
    p.created = timeutil::now_rfc3339();
    if let Some(stem) = dst_path.file_stem() {
        p.name = stem.to_string();
    }
    if !opts.keep_runs {
        p.runs.clear();
    }
    if !opts.keep_sessions {
        p.assistant_sessions.clear();
    }
    copy_dir(&sidecar_dir(src_path), &sidecar_dir(dst_path))?;
    rebase_media(&mut p, Some(src_path), dst_path);
    p.save(dst_path)?;
    Ok(p)
}

/// File › Save As: writes `project` to a new place, copying the sidecar next to it.
pub fn save_as(project: &mut Project, old_path: Option<&Utf8Path>, new_path: &Utf8Path) -> Result<()> {
    if let Some(old) = old_path {
        if old != new_path {
            copy_dir(&sidecar_dir(old), &sidecar_dir(new_path))?;
        }
    }
    rebase_media(project, old_path, new_path);
    project.save(new_path)
}

/// File › Save As Template: keeps the spec, intent, brand and configs; drops media, runs, sessions.
pub fn as_template(project: &Project, name: &str) -> Project {
    let mut t = project.clone();
    t.id = ids::new_id("prj");
    t.name = name.to_string();
    t.created = timeutil::now_rfc3339();
    t.modified = t.created.clone();
    t.media.clear();
    t.media_roots.clear();
    t.runs.clear();
    t.assistant_sessions.clear();
    t
}

/// Starts a project from a template file: a new ID and no history.
pub fn from_template(template_path: &Utf8Path, name: &str) -> Result<Project> {
    let t = Project::load(template_path)?;
    Ok(as_template(&t, name))
}

/// File › Export Spec…: the Markdown with `media:<id>` links rewritten to file paths.
pub fn export_spec(project: &Project, project_path: Option<&Utf8Path>) -> String {
    let base = project_path.and_then(|p| p.parent()).unwrap_or(Utf8Path::new("."));
    crate::spec::rewrite_media_links(&project.spec, |id| {
        let m = project.media_by_id(id)?;
        let found = m.candidate_paths(base).into_iter().find(|p| p.exists());
        found.map(|p| p.to_string()).or_else(|| m.paths.absolute.clone())
    })
}

/// Appends or updates one run record in the file on disk, under a lock, without touching anything
/// else. Used by runs so the app and the CLI can share a project (spec §13).
pub fn record_run(project_path: &Utf8Path, record: &RunRecord) -> Result<()> {
    use fs2::FileExt;
    // The lock lives in the temp folder so no lock files appear next to projects.
    let lock_dir = std::env::temp_dir().join("video-studio-locks");
    std::fs::create_dir_all(&lock_dir)?;
    let key = ids::slug(paths::canonical_or_normal(project_path).as_str());
    let key = if key.len() > 120 { key[key.len() - 120..].to_string() } else { key };
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_dir.join(format!("{key}.lock")))?;
    lock.lock_exclusive()?;
    let result = (|| {
        let mut p = Project::load(project_path)?;
        let modified = p.modified.clone();
        p.upsert_run(record.clone());
        p.normalize();
        // Run history is not an edit: keep `modified` so the app doesn't see a content change.
        p.modified = modified;
        paths::write_atomic(project_path, p.to_json()?.as_bytes())
    })();
    let _ = FileExt::unlock(&lock);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{MediaItem, MediaKind};

    fn sample() -> Project {
        let mut p = Project::new("Spring launch teaser");
        p.spec = "# Teaser\n\n![hero](media:m-hero)\n".into();
        p.intent.kind = IntentKind::Marketing;
        p.media.push(MediaItem::with_path("m-hero", MediaKind::Video, "/footage/hero.mov"));
        let mut rc = RunConfig::new("Social pack");
        rc.outputs = vec!["out_shorts".into(), "gone".into()];
        p.output_configs.push(OutputConfig {
            id: "out_shorts".into(),
            preset: "youtube_shorts".into(),
            name: None,
            overrides: Default::default(),
        });
        p.run_configs.push(rc);
        p
    }

    #[test]
    fn round_trip_is_stable() {
        let mut p = sample();
        p.normalize();
        let a = p.to_json().unwrap();
        let q = Project::from_json(&a).unwrap();
        assert_eq!(p, q);
        assert_eq!(a, q.to_json().unwrap());
        assert!(a.ends_with("}\n") && !a.contains('\r'));
        // key order follows the spec: format, version, id, name …
        let f = a.find("\"format\"").unwrap();
        let v = a.find("\"version\"").unwrap();
        let s = a.find("\"spec\"").unwrap();
        assert!(f < v && v < s);
        // dangling output reference dropped
        assert_eq!(q.run_configs[0].outputs, vec!["out_shorts".to_string()]);
    }

    #[test]
    fn unknown_keys_survive() {
        let mut v: Value = serde_json::from_str(&sample().to_json().unwrap()).unwrap();
        v["future_field"] = serde_json::json!({"a": 1});
        let p = Project::from_json(&v.to_string()).unwrap();
        assert_eq!(p.extra["future_field"]["a"], 1);
        assert!(p.to_json().unwrap().contains("future_field"));
    }

    #[test]
    fn rejects_wrong_format_and_newer_version() {
        assert!(matches!(Project::from_json("{\"format\":\"x\"}"), Err(Error::InvalidProject(_))));
        let mut v: Value = serde_json::from_str(&sample().to_json().unwrap()).unwrap();
        v["version"] = Value::from(99);
        let e = Project::from_json(&v.to_string()).unwrap_err().to_string();
        assert!(e.contains("newer version"), "{e}");
    }

    #[test]
    fn migrates_version_zero() {
        let text = r#"{"format":"video-studio-project","name":"Old","production_model":"opus",
            "run_configs":[{"id":"rc1","name":"A"}],"spec":"x\r\ny"}"#;
        let p = Project::from_json(text).unwrap();
        assert_eq!(p.version, 1);
        assert_eq!(p.run_configs[0].production_model, "opus");
        assert_eq!(p.spec, "x\ny");
        assert!(p.id.starts_with("prj_"));
    }

    #[test]
    fn save_load_duplicate_and_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("a/footage")).unwrap();
        std::fs::write(root.join("a/footage/hero.mov"), b"video").unwrap();
        let path = root.join("a/teaser.cutproj");
        let mut p = sample();
        p.media[0].paths.absolute = Some(root.join("a/footage/hero.mov").to_string());
        p.media[0].paths.relative = Some("footage/hero.mov".into());
        // a pasted image in the sidecar
        let mut png = Vec::new();
        image::DynamicImage::new_rgba8(4, 4)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let stored = store_pasted_image(&path, &png).unwrap();
        assert_eq!(store_pasted_image(&path, &png).unwrap(), stored, "same picture is stored once");
        let mut pasted = MediaItem::with_path("m-pasted", MediaKind::Image, stored.as_str());
        pasted.pasted = true;
        p.media.push(pasted);
        p.runs.push(RunRecord {
            id: "run_1".into(),
            run_config: "rc".into(),
            run_config_name: None,
            kind: RunKind::Production,
            folder: "/x".into(),
            status: RunStatus::Succeeded,
            started: "2026-11-14T10:00:00Z".into(),
            finished: None,
            claude_session: None,
            model: None,
            previous_run: None,
            cost_usd: None,
            eleven_credits: None,
        });
        p.save(&path).unwrap();
        assert_eq!(unused_sidecar_images(&p, &path), vec![stored.clone()], "pasted image isn't referenced by the spec");

        std::fs::create_dir_all(root.join("b")).unwrap();
        let dst = root.join("b/copy.cutproj");
        let d = duplicate(&path, &dst, DuplicateOptions::default()).unwrap();
        assert_ne!(d.id, p.id);
        assert!(d.runs.is_empty());
        assert_eq!(d.name, "copy");
        assert_eq!(d.media[0].paths.relative.as_deref(), Some("../a/footage/hero.mov"));
        let copied = sidecar_dir(&dst).join(stored.file_name().unwrap());
        assert!(copied.exists(), "sidecar travels with the project");
        assert_eq!(d.media[1].paths.absolute.as_deref(), Some(copied.as_str()));
        assert_eq!(Project::load(&dst).unwrap(), d);
    }

    #[test]
    fn run_history_merges() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(dir.path()).unwrap().join("p.cutproj");
        let mut p = sample();
        p.save(&path).unwrap();
        let base = Project::load(&path).unwrap();
        let rec = RunRecord {
            id: "run_a".into(),
            run_config: p.run_configs[0].id.clone(),
            run_config_name: Some("Social pack".into()),
            kind: RunKind::DryRun,
            folder: "/runs/a".into(),
            status: RunStatus::Running,
            started: "2026-11-14T10:00:00Z".into(),
            finished: None,
            claude_session: None,
            model: Some("claude-haiku-5-5".into()),
            previous_run: None,
            cost_usd: None,
            eleven_credits: None,
        };
        record_run(&path, &rec).unwrap();
        let disk = Project::load(&path).unwrap();
        assert_eq!(external_change(&base, &disk), ExternalChange::RunsOnly);
        let mut mine = base.clone();
        mine.spec.push_str("edited");
        assert!(mine.merge_runs_from(&disk));
        assert_eq!(mine.runs.len(), 1);
        let mut done = rec.clone();
        done.status = RunStatus::Succeeded;
        done.finished = Some("2026-11-14T10:05:00Z".into());
        record_run(&path, &done).unwrap();
        let disk = Project::load(&path).unwrap();
        assert_eq!(disk.runs.len(), 1);
        assert!(mine.merge_runs_from(&disk));
        assert_eq!(mine.runs[0].status, RunStatus::Succeeded);
        let mut other = disk.clone();
        other.name = "Renamed elsewhere".into();
        assert_eq!(external_change(&base, &other), ExternalChange::Other);
    }

    #[test]
    fn template_drops_media_and_history() {
        let p = sample();
        let t = as_template(&p, "Template");
        assert!(t.media.is_empty() && t.runs.is_empty());
        assert_eq!(t.spec, p.spec);
        assert_eq!(t.run_configs, p.run_configs);
        assert_ne!(t.id, p.id);
    }

    #[test]
    fn schema_is_generated() {
        let s = json_schema();
        assert!(s["properties"]["spec"].is_object());
        assert!(s["properties"]["run_configs"].is_object());
    }
}
