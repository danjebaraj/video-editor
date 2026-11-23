//! UniFFI bindings for the shells (ui/spec.md §3).
//!
//! The surface is deliberately small. Data crosses as JSON — the same JSON as the `.cutproj`
//! file, the validation report and the NDJSON run events — through one entry point,
//! [`call`], which names a core operation and its arguments. Long-running work (runs and
//! assistant turns) returns an object and reports through a listener, so the UI never blocks on
//! the core.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use camino::{Utf8Path, Utf8PathBuf};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use video_studio_core as core;
use video_studio_core::assistant::{self, AssistantEvent, AssistantHandle, AssistantRequest, AssistantSink};
use video_studio_core::brand::{self, AssetRef, BrandPreset, BrandRef};
use video_studio_core::claude::locate::{self, ClaudeInfo, CutlineInfo, DoctorSummary};
use video_studio_core::media::{self, MediaKind, PrefixFix, Resolution};
use video_studio_core::models::ModelList;
use video_studio_core::platform::FileRefs;
use video_studio_core::presets::{self, OutputConfig, OutputPreset, Platforms};
use video_studio_core::project::{self, DuplicateOptions, IntentKind, Project, RunConfig, RunKind};
use video_studio_core::run::events::{Event, EventSink};
use video_studio_core::run::readiness::ReadinessReport;
use video_studio_core::run::{start_run as core_start_run, RunEnv, RunHandle, RunRequest};
use video_studio_core::settings::Settings;
use video_studio_core::validation::{self, EnvProbe, ValidateInput};
use video_studio_core::{logs, paths, spec};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    /// `code` is one of the core's stable error codes (`not_found`, `environment`, …).
    #[error("{message}")]
    Failed { code: String, message: String },
}

impl From<core::Error> for CoreError {
    fn from(e: core::Error) -> Self {
        CoreError::Failed { code: e.code().to_string(), message: e.to_string() }
    }
}

fn bad(message: impl Into<String>) -> CoreError {
    CoreError::Failed { code: "bad_request".into(), message: message.into() }
}

type R<T> = Result<T, CoreError>;

// MARK: File references (bookmarks) supplied by the shell

/// The shell's implementation of durable file references: security-scoped bookmarks on macOS.
#[uniffi::export(callback_interface)]
pub trait FileRefsProvider: Send + Sync {
    fn create(&self, path: String) -> Option<String>;
    fn resolve(&self, reference: String) -> Option<String>;
}

static FILE_REFS: OnceLock<Mutex<Option<Arc<dyn FileRefsProvider>>>> = OnceLock::new();

/// Registers the shell's file reference provider. Until then media resolves by path only.
#[uniffi::export]
pub fn set_file_refs_provider(provider: Box<dyn FileRefsProvider>) {
    *FILE_REFS.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(Arc::from(provider));
}

struct ShellRefs;

impl FileRefs for ShellRefs {
    fn create(&self, path: &Utf8Path) -> Option<String> {
        let p = FILE_REFS.get()?.lock().unwrap().clone()?;
        p.create(path.to_string())
    }
    fn resolve(&self, reference: &str) -> Option<Utf8PathBuf> {
        let p = FILE_REFS.get()?.lock().unwrap().clone()?;
        p.resolve(reference.to_string()).map(Utf8PathBuf::from)
    }
}

// MARK: Argument helpers

fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> R<T> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null))
        .map_err(|e| bad(format!("argument \"{key}\": {e}")))
}

fn opt<T: DeserializeOwned>(args: &Value, key: &str) -> R<Option<T>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|e| bad(format!("argument \"{key}\": {e}"))),
    }
}

fn project_arg(args: &Value) -> R<Project> {
    let mut p: Project = arg(args, "project")?;
    p.normalize();
    Ok(p)
}

fn path_arg(args: &Value, key: &str) -> R<Option<Utf8PathBuf>> {
    Ok(opt::<String>(args, key)?.filter(|s| !s.is_empty()).map(Utf8PathBuf::from))
}

fn out<T: serde::Serialize>(v: T) -> R<String> {
    serde_json::to_string(&v).map_err(|e| bad(e.to_string()))
}

fn library() -> Utf8PathBuf {
    paths::library_dir()
}

/// Version of the core this binding was built from.
#[uniffi::export]
pub fn core_version() -> String {
    core::CORE_VERSION.to_string()
}

/// Stores a pasted image in the project's sidecar and returns its path (spec §5).
#[uniffi::export]
pub fn store_pasted_image(project_path: String, image: Vec<u8>) -> R<String> {
    Ok(project::store_pasted_image(Utf8Path::new(&project_path), &image)?.to_string())
}

// MARK: Environment

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
struct Environment {
    claude: Option<ClaudeInfo>,
    signed_in: Option<bool>,
    cutline: Option<CutlineInfo>,
    doctor: Option<DoctorSummary>,
    min_claude_version: String,
}

fn detect_environment(settings: &Settings) -> Environment {
    let claude = locate::find_claude(&settings.claude_path);
    let signed_in = claude.as_ref().and_then(locate::claude_signed_in);
    let cutline = locate::find_cutline(&settings.cutline_path, &settings.cutline_repo);
    let doctor = cutline.as_ref().and_then(|c| locate::cutline_doctor(Utf8Path::new(&c.path)).ok());
    Environment { claude, signed_in, cutline, doctor, min_claude_version: locate::min_version_text() }
}

/// The machine as validation sees it: what the shell already detected, plus the ElevenLabs
/// state it read through the helper (the app itself never touches the key).
struct ShellEnv {
    env: Environment,
    eleven_key_present: bool,
    eleven_remaining: Option<u64>,
    cartesia_key_present: bool,
    /// The provider in Settings › Assistant when validation runs (FEAT-spry-alpaca-5RQZ).
    provider: core::codex::Provider,
    /// Codex's readiness, when Codex is the provider.
    codex: Option<core::codex::CodexStatus>,
}

impl EnvProbe for ShellEnv {
    fn provider(&self) -> core::codex::Provider {
        self.provider
    }
    fn codex_status(&self) -> Option<core::codex::CodexStatus> {
        self.codex.clone()
    }
    fn claude(&self) -> Option<ClaudeInfo> {
        self.env.claude.clone()
    }
    fn claude_signed_in(&self, _: &ClaudeInfo) -> Option<bool> {
        self.env.signed_in
    }
    fn cutline(&self) -> Option<CutlineInfo> {
        self.env.cutline.clone()
    }
    fn doctor(&self, _: &CutlineInfo) -> Option<DoctorSummary> {
        self.env.doctor.clone()
    }
    fn free_space(&self, path: &Utf8Path) -> Option<u64> {
        core::platform::free_space(path)
    }
    fn is_writable(&self, path: &Utf8Path) -> bool {
        core::platform::is_writable_dir(path)
    }
    fn eleven_key_present(&self) -> bool {
        self.eleven_key_present
    }
    fn eleven_remaining(&self) -> Option<u64> {
        self.eleven_remaining
    }
    fn cartesia_key_present(&self) -> bool {
        self.cartesia_key_present
    }
    fn capture_permissions(&self) -> Option<core::capture::Permissions> {
        Some(core::capture::permissions(false))
    }
}

fn environment_arg(args: &Value, settings: &Settings) -> R<Environment> {
    Ok(match opt::<Environment>(args, "environment")? {
        Some(mut e) => {
            recheck_still(&mut e);
            e
        }
        None => detect_environment(settings),
    })
}

/// A shell that doesn't know `still` (an older app build) sends the environment without it, and
/// it reads as `false`, so every cutline looked like an old build (BUG-0003). The cutline itself
/// is asked again before anything relies on `still: false`.
fn recheck_still(env: &mut Environment) {
    if let Some(c) = env.cutline.as_mut().filter(|c| !c.still) {
        c.still = locate::has_still(Utf8Path::new(&c.path));
    }
}

// MARK: The dispatcher

/// Runs one core operation. `args` and the result are JSON; the match below is the
/// list of operations, each reading its named arguments with `arg` / `opt`.
#[uniffi::export]
pub fn call(method: String, args: String) -> R<String> {
    let args: Value = if args.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&args).map_err(|e| bad(format!("arguments aren't JSON: {e}")))?
    };
    let lib = library();
    let refs = ShellRefs;
    match method.as_str() {
        // Library and settings
        "library.dir" => out(lib.to_string()),
        "settings.load" => out(Settings::load(&lib)),
        "settings.save" => {
            let s: Settings = arg(&args, "settings")?;
            s.save(&lib)?;
            out(s)
        }
        "models.list" => out(ModelList::load(&lib)),
        "platforms.list" => out(Platforms::load(&lib)),
        "preset.save" => {
            let p: OutputPreset = arg(&args, "preset")?;
            presets::save_user_preset(&lib, &p)?;
            out(Platforms::load(&lib))
        }
        "preset.delete" => {
            presets::delete_user_preset(&lib, &arg::<String>(&args, "id")?)?;
            out(Platforms::load(&lib))
        }
        "preset.duplicate" => {
            let p = presets::duplicate_preset(
                &Platforms::load(&lib),
                &arg::<String>(&args, "id")?,
                &arg::<String>(&args, "name")?,
            )?;
            presets::save_user_preset(&lib, &p)?;
            out(p)
        }
        "environment.detect" => out(detect_environment(&Settings::load(&lib))),
        // Whether the system lets this app record the screen and control other apps; `request`
        // shows the system's prompts.
        "capture.permissions" => out(core::capture::permissions(opt::<bool>(&args, "request")?.unwrap_or(false))),

        // Projects
        "project.new" => {
            let name: String = arg(&args, "name")?;
            let kind: IntentKind = arg(&args, "intent")?;
            let settings = Settings::load(&lib);
            let platforms = Platforms::load(&lib);
            let mut p = Project::new(&name);
            p.intent.kind = kind;
            if let Some(other) = opt::<String>(&args, "intent_other")? {
                p.intent.other = Some(other);
            }
            let defaults = platforms.intent_defaults(kind);
            let template = opt::<String>(&args, "template")?.unwrap_or_else(|| defaults.template.clone());
            p.spec = spec::template_for(&template, &name);
            // Empty: the project follows the defaults in Settings until it chooses its own.
            p.models.assistant = String::new();
            p.models.dry_run = String::new();
            p.models.web_access = settings.web_access_default;
            let mut taken = HashSet::new();
            for preset in defaults.outputs.iter().filter_map(|id| platforms.preset(id)) {
                let cfg = OutputConfig::from_preset(preset, &taken);
                taken.insert(cfg.id.clone());
                p.output_configs.push(cfg);
            }
            let mut rc = RunConfig::new("Default");
            rc.production_model = settings.default_production_model.clone();
            rc.eleven_credit_cap = settings.eleven_default_credit_cap;
            rc.quality =
                if settings.default_quality == "draft" { project::Quality::Draft } else { project::Quality::Final };
            rc.web_access = settings.web_access_default;
            rc.outputs = p.output_configs.iter().map(|o| o.id.clone()).collect();
            p.run_configs.push(rc);
            if let Some(b) = brand::list(&lib).into_iter().next() {
                p.brand = Some(BrandRef::of(&b));
            }
            out(p)
        }
        "project.from_template" => {
            out(project::from_template(Utf8Path::new(&arg::<String>(&args, "path")?), &arg::<String>(&args, "name")?)?)
        }
        "project.load" => out(Project::load(Utf8Path::new(&arg::<String>(&args, "path")?))?),
        "project.save" => {
            let mut p = project_arg(&args)?;
            p.save(Utf8Path::new(&arg::<String>(&args, "path")?))?;
            out(p)
        }
        "project.save_as" => {
            let mut p = project_arg(&args)?;
            project::save_as(
                &mut p,
                path_arg(&args, "old_path")?.as_deref(),
                Utf8Path::new(&arg::<String>(&args, "path")?),
            )?;
            out(p)
        }
        "project.duplicate" => {
            let opts = DuplicateOptions {
                keep_runs: opt(&args, "keep_runs")?.unwrap_or(false),
                keep_sessions: opt(&args, "keep_sessions")?.unwrap_or(false),
            };
            out(project::duplicate(
                Utf8Path::new(&arg::<String>(&args, "source")?),
                Utf8Path::new(&arg::<String>(&args, "destination")?),
                opts,
            )?)
        }
        "project.as_template" => out(project::as_template(&project_arg(&args)?, &arg::<String>(&args, "name")?)),
        "project.export_spec" => out(project::export_spec(&project_arg(&args)?, path_arg(&args, "path")?.as_deref())),
        "project.external_change" => {
            // How the file on disk differs from the copy the app last read or wrote.
            let base: Project = arg(&args, "base")?;
            let disk = Project::load(Utf8Path::new(&arg::<String>(&args, "path")?))?;
            out(json!({"change": project::external_change(&base, &disk), "disk": disk}))
        }
        "project.merge_runs" => {
            let mut p = project_arg(&args)?;
            let disk = Project::load(Utf8Path::new(&arg::<String>(&args, "path")?))?;
            let changed = p.merge_runs_from(&disk);
            out(json!({"project": p, "changed": changed}))
        }
        "project.unused_pasted_images" => {
            out(project::unused_sidecar_images(&project_arg(&args)?, Utf8Path::new(&arg::<String>(&args, "path")?)))
        }
        "project.schema" => out(project::json_schema()),
        "project.summary" => {
            let p = project_arg(&args)?;
            let path = path_arg(&args, "path")?;
            let platforms = Platforms::load(&lib);
            let settings = Settings::load(&lib);
            let resolutions = media::resolve_all(&p, path.as_deref(), &settings.media_roots, &refs);
            let outputs = match opt::<String>(&args, "run_config")?.and_then(|id| p.run_config(&id).cloned()) {
                Some(rc) => presets::resolve_run_outputs(&p, &rc, &platforms).unwrap_or_default(),
                None => {
                    let mut taken = HashSet::new();
                    p.output_configs
                        .iter()
                        .filter_map(|c| presets::resolve_output(c, &platforms, &mut taken).ok())
                        .collect()
                }
            };
            let manifest = core::run::prepare::manifest(&p, &resolutions);
            let read_paths: Vec<String> = resolutions
                .iter()
                .filter_map(|r| r.path.as_ref())
                .filter_map(|p| Utf8Path::new(p).parent().map(|d| d.to_string()))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let mut summary = core::run::prompt::project_summary(&p, &outputs, &manifest);
            summary.push_str(&core::run::prompt::assistant_brand_notes(&p));
            summary.push_str(&core::run::prompt::assistant_voice_notes(
                settings.voice_provider,
                p.brand.as_ref().and_then(|b| b.snapshot.voice.model.as_deref()),
            ));
            out(json!({"summary": summary, "read_paths": read_paths, "media": manifest}))
        }

        // Media
        "media.add" => {
            let mut p = project_arg(&args)?;
            let path = path_arg(&args, "path")?;
            let files: Vec<String> = arg(&args, "files")?;
            let kinds: Option<Vec<MediaKind>> = opt(&args, "kinds")?;
            let mut ids: Vec<String> = Vec::new();
            let mut errors: Vec<String> = Vec::new();
            for f in files {
                let f = Utf8PathBuf::from(f);
                let r = if f.is_dir() {
                    media::add_folder(&mut p, path.as_deref(), &f, kinds.as_deref(), &refs)
                } else {
                    media::add_media(&mut p, path.as_deref(), &f, &refs).map(|id| vec![id])
                };
                match r {
                    Ok(v) => ids.extend(v),
                    Err(e) => errors.push(e.to_string()),
                }
            }
            out(json!({"project": p, "ids": ids, "errors": errors}))
        }
        "media.resolve" => {
            // Resolves every item and applies silent path updates; `dirty` says the project changed.
            let mut p = project_arg(&args)?;
            let path = path_arg(&args, "path")?;
            let settings = Settings::load(&lib);
            let resolutions = media::resolve_all(&p, path.as_deref(), &settings.media_roots, &refs);
            let dirty = media::apply_resolutions(&mut p, path.as_deref(), &resolutions, &refs);
            out(json!({"project": p, "resolutions": resolutions, "dirty": dirty}))
        }
        "media.probe" => {
            let mut p = project_arg(&args)?;
            let resolutions: Vec<Resolution> = arg(&args, "resolutions")?;
            let settings = Settings::load(&lib);
            let changed = match locate::find_cutline(&settings.cutline_path, &settings.cutline_repo) {
                Some(c) => media::probe_all(&mut p, &resolutions, Utf8Path::new(&c.path)),
                None => false,
            };
            out(json!({"project": p, "changed": changed}))
        }
        "media.relink" => {
            let mut p = project_arg(&args)?;
            let path = path_arg(&args, "path")?;
            let fix = media::relink_item(
                &mut p,
                path.as_deref(),
                &arg::<String>(&args, "id")?,
                Utf8Path::new(&arg::<String>(&args, "file")?),
                &refs,
            )?;
            let candidates =
                fix.as_ref().map(|f| media::prefix_fix_candidates(&p, path.as_deref(), f, &refs)).unwrap_or_default();
            out(
                json!({"project": p, "prefix_fix": fix, "candidates": candidates.iter().map(|c| json!({"id": c.0, "path": c.1})).collect::<Vec<_>>()}),
            )
        }
        "media.apply_prefix_fix" => {
            let mut p = project_arg(&args)?;
            let fix: PrefixFix = arg(&args, "fix")?;
            let relinked = media::apply_prefix_fix(&mut p, path_arg(&args, "path")?.as_deref(), &fix, &refs);
            out(json!({"project": p, "relinked": relinked}))
        }
        "media.search_folder" => {
            let mut p = project_arg(&args)?;
            let relinked = media::search_folder(
                &mut p,
                path_arg(&args, "path")?.as_deref(),
                Utf8Path::new(&arg::<String>(&args, "folder")?),
                &refs,
            )?;
            out(json!({"project": p, "relinked": relinked}))
        }
        "media.accept_changed" => {
            let mut p = project_arg(&args)?;
            media::accept_changed(
                &mut p,
                &arg::<String>(&args, "id")?,
                Utf8Path::new(&arg::<String>(&args, "file")?),
                path_arg(&args, "path")?.as_deref(),
                &refs,
            )?;
            out(p)
        }

        // Spec
        "spec.outline" => out(spec::outline(&arg::<String>(&args, "spec")?)),
        "spec.lint" => {
            let p = project_arg(&args)?;
            let ids: HashSet<String> = p.media.iter().map(|m| m.id.clone()).collect();
            let settings = Settings::load(&lib);
            out(spec::lint(
                &p.spec,
                &spec::LintContext {
                    media_ids: &ids,
                    target_length_s: p.intent.target_length_s,
                    words_per_minute: settings.words_per_minute,
                },
            ))
        }
        "spec.templates" => {
            out(spec::TEMPLATES.iter().map(|(id, title)| json!({"id": id, "title": title})).collect::<Vec<_>>())
        }
        "spec.template" => out(spec::template_for(&arg::<String>(&args, "id")?, &arg::<String>(&args, "name")?)),
        "spec.slash_blocks" => out(spec::SLASH_BLOCKS
            .iter()
            .map(|(command, title, snippet)| json!({"command": command, "title": title, "snippet": snippet}))
            .collect::<Vec<_>>()),
        "spec.diff" => out(assistant::diff_hunks(&arg::<String>(&args, "old")?, &arg::<String>(&args, "new")?)),
        "spec.apply_hunks" => out(assistant::apply_hunks(
            &arg::<String>(&args, "old")?,
            &arg::<Vec<assistant::Hunk>>(&args, "hunks")?,
            &arg::<Vec<bool>>(&args, "accepted")?,
        )),

        // Brands
        "brand.list" => out(brand::list(&lib)),
        "brand.save" => {
            let mut b: BrandPreset = arg(&args, "brand")?;
            brand::save(&lib, &mut b)?;
            out(b)
        }
        "brand.new" => out(BrandPreset::new(&arg::<String>(&args, "name")?)),
        "brand.delete" => {
            brand::delete(&lib, &arg::<String>(&args, "id")?)?;
            out(brand::list(&lib))
        }
        "brand.ensure_default" => out(brand::ensure_default(&lib)?),
        "brand.export" => {
            brand::export_cutbrand(
                &arg::<BrandPreset>(&args, "brand")?,
                Utf8Path::new(&arg::<String>(&args, "destination")?),
            )?;
            out(true)
        }
        "brand.import" => out(brand::import_cutbrand(
            &lib,
            Utf8Path::new(&arg::<String>(&args, "file")?),
            opt(&args, "replace")?.unwrap_or(false),
        )?),
        "brand.is_newer" => out(brand::library_is_newer(&lib, &arg::<BrandRef>(&args, "brand")?)),
        "brand.asset" => out(AssetRef::new(Utf8Path::new(&arg::<String>(&args, "path")?))),

        // Outputs
        "outputs.resolve" => {
            let p = project_arg(&args)?;
            let platforms = Platforms::load(&lib);
            let mut taken = HashSet::new();
            let ids: Option<Vec<String>> = opt(&args, "ids")?;
            let list: Vec<Value> = p
                .output_configs
                .iter()
                .filter(|c| ids.as_ref().map(|i| i.contains(&c.id)).unwrap_or(true))
                .map(|c| match presets::resolve_output(c, &platforms, &mut taken) {
                    Ok(o) => serde_json::to_value(o).unwrap_or(Value::Null),
                    Err(e) => json!({"id": c.id, "error": e.to_string()}),
                })
                .collect();
            out(list)
        }
        "outputs.root" => {
            let p = project_arg(&args)?;
            let settings = Settings::load(&lib);
            let rc =
                p.run_config(&arg::<String>(&args, "run_config")?).cloned().ok_or_else(|| bad("no such run config"))?;
            out(rc.output_root(&p.name, Some(&settings.default_output_folder)).to_string())
        }

        // Validation
        "validate" => {
            let mut p = project_arg(&args)?;
            let path = path_arg(&args, "path")?;
            let settings = Settings::load(&lib);
            let platforms = Platforms::load(&lib);
            let models = ModelList::load(&lib);
            let env = environment_arg(&args, &settings)?;
            let kind: RunKind = opt(&args, "kind")?.unwrap_or(RunKind::Production);
            let rc = opt::<String>(&args, "run_config")?.and_then(|id| p.run_config(&id).cloned());
            let resolutions = media::resolve_all(&p, path.as_deref(), &settings.media_roots, &refs);
            if let Some(c) = &env.cutline {
                media::probe_all(&mut p, &resolutions, Utf8Path::new(&c.path));
            }
            let shell_env = ShellEnv {
                env,
                eleven_key_present: opt(&args, "eleven_key_present")?.unwrap_or(false),
                eleven_remaining: opt(&args, "eleven_remaining")?,
                cartesia_key_present: opt(&args, "cartesia_key_present")?.unwrap_or(false),
                provider: settings.provider,
                // The shell's latest `codex.check` when it sends one, else Codex is asked now.
                codex: match settings.provider {
                    core::codex::Provider::Codex => Some(
                        args.get("codex")
                            .and_then(|v| serde_json::from_value::<core::codex::CodexStatus>(v.clone()).ok())
                            .unwrap_or_else(|| core::codex::check(&settings.codex_path)),
                    ),
                    core::codex::Provider::Claude => None,
                },
            };
            let mut report = validation::validate(&ValidateInput {
                project: &p,
                project_path: path.as_deref(),
                run_config: rc.as_ref(),
                kind,
                settings: &settings,
                platforms: &platforms,
                models: &models,
                library: &lib,
                resolutions: &resolutions,
                env: &shell_env,
                output_override: None,
            });
            if let Some(r) = opt::<ReadinessReport>(&args, "readiness")? {
                report.add_dry_run(&r);
            }
            // Probe results are a cache worth keeping: hand the project back with them.
            out(json!({"report": report, "project": p, "resolutions": resolutions}))
        }
        "validation.assistant_context" => {
            let report: video_studio_core::validation::Report = arg(&args, "report")?;
            let only = args.get("only").and_then(|v| v.as_str());
            out(json!({"context": report.assistant_context(only), "request": report.fix_request(only)}))
        }
        "validation.fix_prompt" => out(arg::<ReadinessReport>(&args, "readiness")?.fix_prompt()),

        // Runs and logs
        // Settings › Assistant: is Codex installed, compatible and signed in, and which models?
        "codex.check" => {
            let path = opt::<String>(&args, "path")?.unwrap_or_else(|| Settings::load(&library()).codex_path);
            // the status, plus its finding (code, title, remedy) for Settings to show when not ready
            let status = core::codex::check(&path);
            let mut v = serde_json::to_value(&status).unwrap_or(Value::Null);
            if let Some((code, title, remedy)) = status.finding() {
                v["finding"] = json!({"code": code, "title": title, "remedy": remedy});
            }
            out(v)
        }
        "run.results" => out(core::run::collect::load_results(Utf8Path::new(&arg::<String>(&args, "folder")?))),
        "logs.export" => out(logs::export_logs(
            Utf8Path::new(&arg::<String>(&args, "folder")?),
            Utf8Path::new(&arg::<String>(&args, "destination")?),
            opt(&args, "redact")?.unwrap_or(false),
        )?),
        "logs.issue_report" => out(logs::issue_report(Utf8Path::new(&arg::<String>(&args, "folder")?))),
        "storage.info" => out(logs::storage_info(&lib, &arg::<Vec<String>>(&args, "run_folders")?)),
        "storage.cleanup" => {
            out(logs::cleanup_runs(&lib, &arg::<Vec<String>>(&args, "run_folders")?, arg(&args, "days")?))
        }
        "storage.clear_eleven_cache" => {
            core::eleven::clear_cache(&core::eleven::cache_dir(&lib))?;
            out(true)
        }
        // The model the next operation uses, as the run or turn will use it (BUG-fond-crane-62ZP).
        // `models` and `run_config` are the project's as edited; `provider` defaults to Settings.
        "models.effective" => {
            let settings = Settings::load(&lib);
            let provider: core::codex::Provider = opt(&args, "provider")?.unwrap_or(settings.provider);
            let models: core::project::Models = opt(&args, "models")?.unwrap_or_default();
            let rc: Option<core::project::RunConfig> = opt(&args, "run_config")?;
            let use_: core::project::ModelUse = arg(&args, "use")?;
            let over: Option<String> = opt(&args, "override")?;
            let model =
                core::project::effective_model(&models, rc.as_ref(), &settings, provider, use_, over.as_deref());
            out(json!({"provider": provider, "model": model, "codex_default": model.is_none()}))
        }
        "assistant.gather_info_prompt" => out(core::run::prompt::GATHER_INFO_PROMPT),
        "assistant.sessions_dir" => {
            out(assistant::sessions_dir(&lib, &arg::<String>(&args, "project_id")?).to_string())
        }
        other => Err(bad(format!("unknown operation \"{other}\""))),
    }
}

// MARK: Runs

/// Receives events as JSON, on a background thread: run events (the `events.ndjson` schema) or
/// assistant events.
#[uniffi::export(callback_interface)]
pub trait EventListener: Send + Sync {
    fn on_event(&self, json: String);
}

struct RunSink(Box<dyn EventListener>);

impl EventSink for RunSink {
    fn emit(&self, event: &Event) {
        self.0.on_event(event.to_json());
    }
}

/// A production or dry run in progress.
#[derive(uniffi::Object)]
pub struct Run {
    handle: RunHandle,
}

#[uniffi::export]
impl Run {
    pub fn run_id(&self) -> String {
        self.handle.run_id().to_string()
    }
    pub fn folder(&self) -> String {
        self.handle.folder().to_string()
    }
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
    /// Stop: an interrupt, then after five seconds the whole process tree is killed.
    pub fn cancel(&self) {
        self.handle.cancel();
    }
    pub fn kill(&self) {
        self.handle.kill();
    }
    /// Answers a question event. `answers_json` maps question text to the chosen answer.
    pub fn answer(&self, question_id: String, answers_json: String) -> R<()> {
        let answers: BTreeMap<String, String> = serde_json::from_str(&answers_json).map_err(|e| bad(e.to_string()))?;
        self.handle.answer(&question_id, answers);
        Ok(())
    }
    /// Blocks until the run ends; returns the outcome as JSON. Call it off the main thread.
    pub fn wait(&self) -> String {
        serde_json::to_string(&self.handle.wait()).unwrap_or_else(|_| "{}".into())
    }
}

/// Starts a run. `request_json` is a `RunRequest`; `options_json` carries what only the shell
/// knows: `helper` (path of the bundled `video-studio` binary), `eleven_key_present`, and for
/// tests `extra_env`, `eleven_base_url` and `eleven_test_key`.
#[uniffi::export]
pub fn start_run(request_json: String, options_json: String, listener: Box<dyn EventListener>) -> R<Arc<Run>> {
    let request: RunRequest = serde_json::from_str(&request_json).map_err(|e| bad(format!("request: {e}")))?;
    let options: Value = serde_json::from_str(&options_json).map_err(|e| bad(format!("options: {e}")))?;
    let helper: String = arg(&options, "helper")?;
    let mut env = RunEnv::detect(&library(), Utf8Path::new(&helper))?;
    env.eleven_key_present = opt(&options, "eleven_key_present")?.unwrap_or(false);
    env.eleven_test_key = opt(&options, "eleven_test_key")?;
    env.cartesia_key_present = opt(&options, "cartesia_key_present")?.unwrap_or(false);
    env.cartesia_test_key = opt(&options, "cartesia_test_key")?;
    if let Some(u) = opt::<String>(&options, "cartesia_base_url")? {
        env.cartesia_base_url = Some(u);
    }
    if let Some(u) = opt::<String>(&options, "eleven_base_url")? {
        env.eleven_base_url = Some(u);
    }
    if let Some(extra) = opt::<std::collections::HashMap<String, String>>(&options, "extra_env")? {
        env.extra_env = extra;
    }
    env.file_refs = Arc::new(ShellRefs);
    let handle = core_start_run(&request, &env, Arc::new(RunSink(listener)))?;
    Ok(Arc::new(Run { handle }))
}

// MARK: Assistant

struct ChatSink(Box<dyn EventListener>);

impl AssistantSink for ChatSink {
    fn emit(&self, event: &AssistantEvent) {
        self.0.on_event(serde_json::to_string(event).unwrap_or_else(|_| "{}".into()));
    }
}

/// One spec assistant turn in progress.
#[derive(uniffi::Object)]
pub struct AssistantTurn {
    handle: AssistantHandle,
}

#[uniffi::export]
impl AssistantTurn {
    pub fn stop(&self) {
        self.handle.stop();
    }
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
    pub fn answer(&self, question_id: String, answers_json: String) -> R<()> {
        let answers: BTreeMap<String, String> = serde_json::from_str(&answers_json).map_err(|e| bad(e.to_string()))?;
        self.handle.answer(&question_id, answers);
        Ok(())
    }
    pub fn dismiss(&self, question_id: String) {
        self.handle.dismiss(&question_id);
    }
    /// Blocks until the turn ends. Call it off the main thread.
    pub fn wait(&self) {
        self.handle.wait();
    }
}

/// The Codex model for an assistant turn: the project's Codex assistant model, then
/// Settings › Codex, then Codex's own default (`None`). Claude's `model` is never used.
fn codex_assistant_model(request: &AssistantRequest, settings: &Settings) -> Option<String> {
    let models = video_studio_core::project::Models {
        codex_assistant: request.codex_model.clone().unwrap_or_default(),
        ..Default::default()
    };
    video_studio_core::project::effective_model(
        &models,
        None,
        settings,
        video_studio_core::codex::Provider::Codex,
        video_studio_core::project::ModelUse::Assistant,
        None,
    )
}

/// Starts one assistant turn. `request_json` is an `AssistantRequest`. The provider comes from
/// Settings › Assistant: the Claude CLI or Codex, each found with its path from Settings.
#[uniffi::export]
pub fn start_assistant(request_json: String, listener: Box<dyn EventListener>) -> R<Arc<AssistantTurn>> {
    let mut request: AssistantRequest =
        serde_json::from_str(&request_json).map_err(|e| bad(format!("request: {e}")))?;
    let settings = Settings::load(&library());
    // cutline makes the viewable copies of HEIC/HEIF photos: those in the media list before the
    // turn, and (with `cutline still`) any other one the agent finds in a folder it may read
    if request.cutline.is_none() {
        request.cutline =
            locate::find_cutline(&settings.cutline_path, &settings.cutline_repo).map(|c| Utf8PathBuf::from(c.path));
    }
    // The provider is read when the turn starts (FEAT-spry-alpaca-5RQZ). With Codex, the session
    // token is `codex:<thread>`; a Claude session token starts a separate Codex conversation and
    // the other way round.
    if settings.provider == video_studio_core::codex::Provider::Codex {
        let executable = match video_studio_core::codex::resolve_codex(&settings.codex_path) {
            video_studio_core::codex::CodexLookup::Found(p) => p,
            other => return Err(CoreError::Failed { code: "environment".into(), message: other.problem() }),
        };
        let setup = assistant::CodexSetup { executable, model: codex_assistant_model(&request, &settings) };
        let handle = assistant::start_codex(&request, &setup, Arc::new(ChatSink(listener)))?;
        return Ok(Arc::new(AssistantTurn { handle }));
    }
    let claude = locate::find_claude(&settings.claude_path).ok_or_else(|| CoreError::Failed {
        code: "environment".into(),
        message: "Claude Code isn't installed.".into(),
    })?;
    if !claude.supported {
        return Err(CoreError::Failed {
            code: "environment".into(),
            message: format!(
                "Claude Code {} is too old; this app needs {} or later.",
                claude.version,
                locate::min_version_text()
            ),
        });
    }
    let handle = assistant::start(&request, &claude, Arc::new(ChatSink(listener)))?;
    Ok(Arc::new(AssistantTurn { handle }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(method: &str, args: Value) -> Value {
        serde_json::from_str(&call(method.into(), args.to_string()).unwrap_or_else(|e| panic!("{method}: {e}")))
            .unwrap()
    }

    #[test]
    fn round_trips_a_project_through_the_dispatcher() {
        let d = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(d.path()).unwrap();
        std::env::set_var("VIDEO_STUDIO_HOME", root.join("lib"));
        assert_eq!(core_version(), core::CORE_VERSION);
        let p = c("project.new", json!({"name": "Launch", "intent": "marketing"}));
        assert!(p["spec"].as_str().unwrap().starts_with("# Launch: marketing video"));
        assert_eq!(p["output_configs"].as_array().unwrap().len(), 3);
        assert_eq!(p["run_configs"][0]["outputs"].as_array().unwrap().len(), 3);
        let path = root.join("launch.cutproj");
        let saved = c("project.save", json!({"project": p, "path": path}));
        let loaded = c("project.load", json!({"path": path}));
        assert_eq!(saved, loaded);
        // media by reference, resolution, outline, lint
        std::fs::write(root.join("hero.mov"), b"video").unwrap();
        let added = c("media.add", json!({"project": loaded, "path": path, "files": [root.join("hero.mov")]}));
        assert_eq!(added["ids"], json!(["m-hero"]));
        let resolved = c("media.resolve", json!({"project": added["project"], "path": path}));
        assert_eq!(resolved["resolutions"][0]["status"], "ok");
        let outline = c("spec.outline", json!({"spec": added["project"]["spec"]}));
        assert!(outline.as_array().unwrap().iter().any(|o| o["kind"] == "scene"));
        let lint = c("spec.lint", json!({"project": added["project"]}));
        assert!(lint.as_array().unwrap().iter().any(|i| i["rule"] == "placeholder"));
        let outs = c("outputs.resolve", json!({"project": added["project"]}));
        assert_eq!(outs[0]["width"], 1920);
        // hunks
        let hunks = c("spec.diff", json!({"old": "a\nb\n", "new": "a\nc\n"}));
        assert_eq!(
            c("spec.apply_hunks", json!({"old": "a\nb\n", "hunks": hunks, "accepted": [true]})),
            json!("a\nc\n")
        );
        // brands and presets live in the library
        let b = c("brand.ensure_default", json!({}));
        assert_eq!(c("brand.list", json!({})).as_array().unwrap().len(), 1);
        assert_eq!(c("brand.is_newer", json!({"brand": {"preset_id": b["id"], "snapshot": b}})), json!(false));
        assert!(c("platforms.list", json!({}))["presets"].as_array().unwrap().len() >= 13);
        assert_eq!(c("models.list", json!({}))["defaults"]["dry_run"], "haiku");
        let mut s = c("settings.load", json!({}));
        s["words_per_minute"] = json!(170);
        assert_eq!(c("settings.save", json!({"settings": s}))["words_per_minute"], 170);
        // Settings › Assistant: a Codex that isn't there comes back with its finding and remedy
        // (a configured path that isn't there is named, with how to fix it)
        let codex = c("codex.check", json!({"path": root.join("no-codex-here")}));
        assert_eq!(
            (codex["state"].as_str(), codex["finding"]["code"].as_str()),
            (Some("bad_path"), Some("codex_path_invalid"))
        );
        assert!(codex["finding"]["title"].as_str().unwrap().contains("no-codex-here"));
        assert!(codex["finding"]["remedy"].as_str().unwrap().contains("Settings › Assistant"));
        let missing = core::codex::CodexStatus::Missing.finding().unwrap();
        assert!(missing.0 == "codex_missing" && missing.2.contains("Install the Codex CLI"));
        // validation with a supplied environment (no tools are run)
        let env =
            json!({"claude": null, "signed_in": null, "cutline": null, "doctor": null, "min_claude_version": "2.1.0"});
        let v = c(
            "validate",
            json!({"project": added["project"], "path": path, "run_config": "Default", "environment": env}),
        );
        let rules: Vec<&str> =
            v["report"]["issues"].as_array().unwrap().iter().map(|i| i["rule"].as_str().unwrap()).collect();
        assert!(
            rules.contains(&"claude_missing")
                && rules.contains(&"cutline_missing")
                && rules.contains(&"spec_placeholder"),
            "{rules:?}"
        );
        // With Codex selected, validation checks the Codex status the shell sends, not Claude
        let mut s = c("settings.load", json!({}));
        s["provider"] = json!("codex");
        c("settings.save", json!({"settings": s.clone()}));
        let rules_with = |codex: Value| -> Vec<String> {
            let v = c(
                "validate",
                json!({"project": added["project"], "path": path, "run_config": "Default", "environment": env, "codex": codex}),
            );
            v["report"]["issues"].as_array().unwrap().iter().map(|i| i["rule"].as_str().unwrap().to_string()).collect()
        };
        let ready = rules_with(json!({"state": "ready", "plan": "pro", "models": []}));
        assert!(!ready.iter().any(|r| r == "claude_missing" || r.starts_with("codex_")), "{ready:?}");
        let missing = rules_with(json!({"state": "missing", "finding": {"code": "codex_missing"}}));
        assert!(
            missing.iter().any(|r| r == "codex_missing") && !missing.iter().any(|r| r == "claude_missing"),
            "{missing:?}"
        );
        s["provider"] = json!("claude");
        c("settings.save", json!({"settings": s}));
        // errors carry a code
        match call("project.load".into(), json!({"path": root.join("nope.cutproj")}).to_string()) {
            Err(CoreError::Failed { code, .. }) => assert_eq!(code, "io"),
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(call("no.such".into(), "{}".into()), Err(CoreError::Failed { code, .. }) if code == "bad_request")
        );
    }

    /// BUG-0003: the macOS app sent the environment back without `still`, so validation said a
    /// current cutline was an older build. A missing `still` is asked of the cutline again.
    #[cfg(unix)]
    #[test]
    fn environment_without_still_is_rechecked() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let make = |name: &str, still: bool| {
            let p = t.path().join(name);
            let tail = if still { "exit 0" } else { "echo \"error: unrecognized subcommand '$1'\" >&2; exit 2" };
            std::fs::write(
                &p,
                format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'cutline 0.1.0'; exit 0; fi\n{tail}\n"),
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p.to_str().unwrap().to_string()
        };
        let parse = |path: &str| -> Environment {
            serde_json::from_value(json!({"cutline": {"path": path, "version": "0.1.0"}, "min_claude_version": ""}))
                .unwrap()
        };
        let mut current = parse(&make("new", true));
        assert!(!current.cutline.as_ref().unwrap().still);
        recheck_still(&mut current);
        assert!(current.cutline.unwrap().still);
        let mut old = parse(&make("old", false));
        recheck_still(&mut old);
        assert!(!old.cutline.unwrap().still);
    }
}
