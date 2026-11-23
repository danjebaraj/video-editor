//! Preparing a run (spec §12.2 step 1): the run folder, the files Claude reads, the permission
//! policy, the helper context and the Claude Code options.

use std::collections::HashSet;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::claude::broker::{hook_settings, Mode, Policy};
use crate::claude::locate::{self, ClaudeInfo, CutlineInfo};
use crate::claude::process::{containing_dir, ClaudeOptions};
use crate::eleven::{self, HelperContext, VoiceProvider, CONTEXT_ENV};
use crate::media::{self, MediaStatus, Resolution};
use crate::models::ModelList;
use crate::platform::{FileRefs, NoFileRefs};
use crate::presets::{cutline_exports, resolve_run_outputs, Platforms, ResolvedOutput};
use crate::project::{Project, RunConfig, RunKind, RunRecord, RunStatus};
use crate::settings::Settings;
use crate::validation::Report;
use crate::{ids, paths, timeutil, Error, Result, CORE_VERSION};

/// The machine and library a run uses.
pub struct RunEnv {
    pub library: Utf8PathBuf,
    pub settings: Settings,
    pub platforms: Platforms,
    pub models: ModelList,
    pub claude: ClaudeInfo,
    pub cutline: CutlineInfo,
    /// The `video-studio` binary: the ElevenLabs helper, the hook and the cutline proxy.
    pub helper: Utf8PathBuf,
    /// cutline's home (tools, models, cache), readable during runs.
    pub cutline_home: Option<Utf8PathBuf>,
    pub cutline_doctor: Option<Value>,
    pub eleven_base_url: Option<String>,
    pub eleven_test_key: Option<String>,
    /// Whether an ElevenLabs key is stored (the caller checks through the helper binary).
    pub eleven_key_present: bool,
    /// The same for Cartesia, which speaks the voice-over when Settings › Voice says so.
    pub cartesia_base_url: Option<String>,
    pub cartesia_test_key: Option<String>,
    pub cartesia_key_present: bool,
    /// Extra environment for the Claude Code process (proxies, test doubles).
    pub extra_env: std::collections::HashMap<String, String>,
    pub file_refs: std::sync::Arc<dyn FileRefs>,
}

impl RunEnv {
    /// Finds Claude Code and cutline. Fails with [`Error::Environment`] when either is missing
    /// or Claude Code is too old (CLI exit code 5).
    pub fn detect(library: &Utf8Path, helper: &Utf8Path) -> Result<RunEnv> {
        let settings = Settings::load(library);
        // With Codex as the provider Claude Code isn't needed (FEAT-spry-alpaca-5RQZ).
        let codex = settings.provider == crate::codex::Provider::Codex;
        if codex && crate::codex::find_codex(&settings.codex_path).is_none() {
            return Err(Error::Environment(CODEX_MISSING.into()));
        }
        let claude = match locate::find_claude(&settings.claude_path) {
            Some(c) => c,
            None if codex => ClaudeInfo {
                path: String::new(),
                version: String::new(),
                version_text: String::new(),
                effort_levels: Vec::new(),
                login_path: None,
                supported: false,
                git_bash: None,
            },
            None => return Err(Error::Environment("Claude Code isn't installed. Install the Claude Code CLI (https://claude.com/claude-code), then try again.".into())),
        };
        if !claude.supported && !codex {
            return Err(Error::Environment(format!(
                "Claude Code {} is too old; this app needs {} or later. Update the CLI and try again.",
                claude.version,
                locate::min_version_text()
            )));
        }
        let cutline = locate::find_cutline(&settings.cutline_path, &settings.cutline_repo).ok_or_else(|| {
            Error::Environment(
                "cutline wasn't found. Set its path in Settings › cutline, or put `cutline` on your PATH.".into(),
            )
        })?;
        let doctor = locate::cutline_doctor(Utf8Path::new(&cutline.path)).ok();
        let cutline_home = doctor
            .as_ref()
            .and_then(|d| d.raw["home"].as_str())
            .map(Utf8PathBuf::from)
            .or_else(|| std::env::var("CUTLINE_HOME").ok().map(Utf8PathBuf::from));
        Ok(RunEnv {
            platforms: Platforms::load(library),
            models: ModelList::load(library),
            library: library.to_owned(),
            settings,
            claude,
            cutline,
            helper: helper.to_owned(),
            cutline_home,
            cutline_doctor: doctor.map(|d| d.raw),
            eleven_base_url: std::env::var("VIDEO_STUDIO_ELEVEN_BASE_URL").ok().filter(|s| !s.is_empty()),
            eleven_test_key: None,
            eleven_key_present: false,
            cartesia_base_url: std::env::var("VIDEO_STUDIO_CARTESIA_BASE_URL").ok().filter(|s| !s.is_empty()),
            cartesia_test_key: None,
            cartesia_key_present: false,
            extra_env: Default::default(),
            file_refs: std::sync::Arc::new(NoFileRefs),
        })
    }
}

/// Feedback on an earlier run (spec §12.5 Revise).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Revise {
    /// ID of the run to revise.
    pub previous_run: String,
    pub feedback: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunRequest {
    pub project_path: Utf8PathBuf,
    /// Run config ID or name.
    pub run_config: String,
    pub kind: RunKind,
    /// Overrides the config's model for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Overrides the config's output folder for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Utf8PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revise: Option<Revise>,
    /// Audio Review: the output (ID or name) whose timing and audio settings to audition;
    /// default the run config's first output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_output: Option<String>,
    /// Headless with `--yes` or no terminal: questions aren't asked, Claude proceeds on its
    /// stated assumption and the question is logged.
    #[serde(default)]
    pub assume_answers: bool,
    /// Seconds a question waits before Claude proceeds on its assumption (0 or absent: forever).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_timeout_s: Option<u32>,
    /// The validation report this run was started with; saved as `logs/validation.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<Report>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Resolved absolute path; absent when the file is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub status: MediaStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<media::Probe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// A JPEG copy Claude can open, for a photo it can't (HEIC/HEIF; see `viewable`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewable_path: Option<String>,
    /// Why no viewable copy could be made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewable_error: Option<String>,
}

pub fn manifest(project: &Project, resolutions: &[Resolution]) -> Vec<ManifestEntry> {
    project
        .media
        .iter()
        .map(|m| {
            let r = resolutions.iter().find(|r| r.id == m.id);
            let mut probe = m.probe.clone();
            if let Some(p) = probe.as_mut() {
                p.for_hash = None;
            }
            ManifestEntry {
                id: m.id.clone(),
                kind: m.kind.key().to_string(),
                role: m.role.and_then(|r| serde_json::to_value(r).ok()).and_then(|v| v.as_str().map(str::to_string)),
                path: r.and_then(|r| r.path.clone()),
                status: r.map(|r| r.status).unwrap_or(MediaStatus::Missing),
                probe,
                notes: m.notes.clone(),
                tags: m.tags.clone(),
                viewable_path: None,
                viewable_error: None,
            }
        })
        .collect()
}

/// Everything the orchestrator needs to launch and follow the run.
pub struct Prepared {
    pub run_id: String,
    pub folder: Utf8PathBuf,
    pub logs: Utf8PathBuf,
    pub context_dir: Utf8PathBuf,
    pub mode: Mode,
    pub project: Project,
    pub run_config: RunConfig,
    pub outputs: Vec<ResolvedOutput>,
    pub manifest: Vec<ManifestEntry>,
    pub policy: Policy,
    pub options: ClaudeOptions,
    pub prompt: String,
    pub record: RunRecord,
    pub helper_context: HelperContext,
    pub request: RunRequest,
    /// Set when Codex drives the run instead of Claude Code (`options` is then unused).
    pub codex: Option<crate::codex::agent::CodexOptions>,
}

fn unique_folder(parent: &Utf8Path, name: &str) -> Utf8PathBuf {
    let first = parent.join(name);
    if !first.exists() {
        return first;
    }
    let mut n = 2;
    loop {
        let c = parent.join(format!("{name}-{n}"));
        if !c.exists() {
            return c;
        }
        n += 1;
    }
}

fn link_or_copy(target: &Utf8Path, link: &Utf8Path) -> Result<()> {
    let _ = std::fs::remove_file(link);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)?;
    }
    #[cfg(not(unix))]
    {
        if std::fs::hard_link(target, link).is_err() {
            std::fs::copy(target, link)?;
        }
    }
    Ok(())
}

fn copy_tree(from: &Utf8Path, to: &Utf8Path) {
    if from.is_file() {
        if let Some(d) = to.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::copy(from, to);
    } else if from.is_dir() {
        for e in walkdir::WalkDir::new(from).into_iter().flatten().filter(|e| e.file_type().is_file()) {
            if let Ok(p) = paths::utf8(e.path().to_path_buf()) {
                if let Ok(rel) = p.strip_prefix(from) {
                    let dest = to.join(rel);
                    if let Some(d) = dest.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::copy(&p, &dest);
                }
            }
        }
    }
}

/// What a run says when Codex is the provider and can't be found.
pub const CODEX_MISSING: &str = "Codex is selected in Settings › Assistant, but it isn't installed. Install the Codex CLI (it provides `codex app-server`) or set its path in Settings › Assistant, then try again.";

/// What Codex is told on top of the production instructions (FEAT-spry-alpaca-5RQZ): its tools
/// aren't Claude's, and the app checks every command and write.
pub const CODEX_RUN_NOTE: &str = "\n\n## Working in Codex\n\n- You run as Codex inside Syncfusion Video Studio. Your working folder is this run's folder; write only there (with apply_patch). Media, brand and reference folders are read-only. `logs/`, `.bin/`, `run.json` and `media-manifest.json` are written by the app.\n- Run `cutline …` and `video-studio eleven …` / `video-studio capture …` as plain commands by those bare names, one per command, with no pipes, redirections or substitutions. Every command is checked by the app and anything else is refused; don't try other tools to get around a refusal.\n- When you need a decision from the user, ask with the request_user_input tool.";

/// Builds the run folder and everything in it. Nothing is launched yet.
pub fn prepare(request: &RunRequest, env: &RunEnv) -> Result<Prepared> {
    // The provider is captured at run start: changing Settings later affects only later runs.
    let codex_exe = match env.settings.provider {
        crate::codex::Provider::Codex => Some(
            crate::codex::find_codex(&env.settings.codex_path)
                .ok_or_else(|| Error::Environment(CODEX_MISSING.into()))?,
        ),
        crate::codex::Provider::Claude => None,
    };
    let mut project = Project::load(&request.project_path)?;
    let rc = project.run_config(&request.run_config).cloned().ok_or_else(|| {
        let names: Vec<&str> = project.run_configs.iter().map(|r| r.name.as_str()).collect();
        Error::NotFound(format!(
            "no run config \"{}\" in {} (it has: {})",
            request.run_config,
            request.project_path,
            if names.is_empty() { "none".to_string() } else { names.join(", ") }
        ))
    })?;
    let outputs = resolve_run_outputs(&project, &rc, &env.platforms)?;
    if outputs.is_empty() {
        return Err(Error::InvalidProject(format!("the run config \"{}\" has no outputs selected", rc.name)));
    }
    let mode = match request.kind {
        RunKind::DryRun => Mode::DryRun,
        // generated audio is made as in production, within the same credit cap
        RunKind::Production | RunKind::AudioReview => Mode::Production,
    };
    let outputs = match request.kind {
        RunKind::AudioReview => vec![audio_review_output(&outputs, request.audio_output.as_deref())?],
        _ => outputs,
    };
    let run_id = ids::new_id("run");
    let stamp = timeutil::run_stamp();
    let slug = ids::slug(&rc.name);
    let folder_name = format!("{stamp}-{}", if slug.is_empty() { "run" } else { &slug });
    // A dry run works in a scratch folder in the library; a production run in the output folder.
    let parent = match (request.kind, &request.output) {
        (RunKind::DryRun, _) => env.library.join("dry-runs").join(&project.id),
        (RunKind::AudioReview, _) => env.library.join("audio-reviews").join(&project.id),
        (RunKind::Production, Some(o)) => o.clone(),
        (RunKind::Production, None) => rc.output_root(&project.name, Some(&env.settings.default_output_folder)),
    };
    std::fs::create_dir_all(&parent).map_err(|e| {
        Error::Io(std::io::Error::new(e.kind(), format!("couldn't create the output folder {parent}: {e}")))
    })?;
    let folder = paths::canonical_or_normal(&unique_folder(&parent, &folder_name));
    let logs = folder.join("logs");
    for d in [&folder, &logs, &folder.join("outputs"), &folder.join("audio"), &folder.join(".bin")] {
        std::fs::create_dir_all(d)?;
    }

    // Media: resolve in place, then describe for Claude.
    let resolutions =
        media::resolve_all(&project, Some(&request.project_path), &env.settings.media_roots, env.file_refs.as_ref());
    // Probe data is a cache in the project; fill what is missing so the manifest is complete.
    media::probe_all(&mut project, &resolutions, Utf8Path::new(&env.cutline.path));
    let mut manifest_entries = manifest(&project, &resolutions);
    // HEIC/HEIF photos get JPEG copies Claude can open; a failed copy never stops the run
    crate::viewable::ensure_viewable_copies(&mut manifest_entries, &folder, Utf8Path::new(&env.cutline.path));
    paths::write_json_atomic(&folder.join("media-manifest.json"), &manifest_entries)?;
    paths::write_atomic(&folder.join("spec.md"), project.spec.replace("\r\n", "\n").as_bytes())?;
    let brand = project.brand.as_ref().map(|b| &b.snapshot);
    if let Some(r) = &project.brand {
        let b = &r.snapshot;
        // The preset as the app keeps it, for Claude to read (with only the instruction sets
        // this project chose), and the same brand in cutline's own format where cutline looks
        // for it without being told (`brand/default.json`).
        paths::write_json_atomic(&folder.join("brand-preset.json"), &r.for_run())?;
        std::fs::create_dir_all(folder.join("brand"))?;
        paths::write_json_atomic(&folder.join("brand").join("default.json"), &b.for_cutline().file)?;
    }
    let exports = cutline_exports(&outputs);
    paths::write_json_atomic(&folder.join("outputs.json"), &json!({"outputs": outputs, "cutline": exports}))?;

    // Revise: start from the previous run's edit and audio.
    let mut previous: Option<RunRecord> = None;
    if let Some(rev) = &request.revise {
        let prev = project
            .runs
            .iter()
            .find(|r| r.id == rev.previous_run)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("no run \"{}\" to revise", rev.previous_run)))?;
        let prev_folder = Utf8PathBuf::from(&prev.folder);
        if !prev_folder.is_dir() {
            return Err(Error::NotFound(format!(
                "the run to revise is no longer at {prev_folder}; use Locate in the run history first"
            )));
        }
        for name in ["project.json5", "audio", "captures", "work", "summary.md"] {
            let dest = if name == "summary.md" { folder.join("previous-summary.md") } else { folder.join(name) };
            copy_tree(&prev_folder.join(name), &dest);
        }
        previous = Some(prev);
    }

    // Names Claude types resolve to the helper: `cutline` is a logging proxy, `video-studio` the
    // ElevenLabs helper.
    let bin = folder.join(".bin");
    let exe = if cfg!(windows) { ".exe" } else { "" };
    link_or_copy(&env.helper, &bin.join(format!("cutline{exe}")))?;
    link_or_copy(&env.helper, &bin.join(format!("video-studio{exe}")))?;

    // Folders Claude may read besides the run folder.
    let mut read_paths: Vec<Utf8PathBuf> = Vec::new();
    let mut add_read = |p: Utf8PathBuf| {
        if !read_paths.contains(&p) {
            read_paths.push(p);
        }
    };
    for m in &manifest_entries {
        if let Some(p) = &m.path {
            add_read(containing_dir(Utf8Path::new(p)));
        }
    }
    if let Some(b) = brand {
        for (_, a) in b.assets() {
            if a.exists() {
                add_read(a.resolved());
            }
        }
    }
    if let Some(h) = &env.cutline_home {
        add_read(h.clone());
    }
    // Claude Code saves large tool results (the cutline schema, long logs) in its own folder
    // for this working directory and reads them back with the file tools.
    if let Some(d) = locate::claude_project_dir(&folder) {
        add_read(d);
    }
    // …and the output of a command it moved to the background (a render that outlasts the
    // command timeout) in its temporary folder for this working directory.
    for d in locate::claude_task_dirs(&folder) {
        add_read(d);
    }

    // The helper's context and the hook's policy live outside anything Claude can read.
    let context_dir = env.library.join("run-contexts").join(&run_id);
    std::fs::create_dir_all(&context_dir)?;
    let provider = env.settings.voice_provider;
    let helper_context = HelperContext {
        run_id: run_id.clone(),
        run_folder: folder.clone(),
        mode,
        logs_dir: logs.clone(),
        cutline: Some(Utf8PathBuf::from(&env.cutline.path)),
        credit_cap: rc.eleven_credit_cap,
        cache_dir: eleven::cache_dir(&env.library),
        cache_limit_bytes: env.settings.eleven_cache_limit_bytes,
        default_voice: match provider {
            VoiceProvider::Elevenlabs => brand
                .map(|b| b.voice.voice_id.clone())
                .filter(|v| !v.is_empty())
                .or_else(|| Some(env.settings.eleven_default_voice.clone()).filter(|v| !v.is_empty())),
            VoiceProvider::Cartesia => brand
                .map(|b| b.voice.cartesia_voice_id.clone())
                .filter(|v| !v.is_empty())
                .or_else(|| Some(env.settings.cartesia_default_voice.clone()).filter(|v| !v.is_empty())),
        },
        // A brand's voice settings and model are ElevenLabs': Cartesia takes its speed, no more.
        default_voice_settings: brand.and_then(|b| b.voice.settings.clone()),
        default_model: match provider {
            VoiceProvider::Elevenlabs => brand.and_then(|b| b.voice.model.clone()),
            VoiceProvider::Cartesia => None,
        },
        voice_preference: env.settings.eleven_voice_preference,
        voice_gender: env.settings.eleven_voice_gender,
        voice_provider: provider,
        cartesia_base_url: env.cartesia_base_url.clone(),
        cartesia_test_key: env.cartesia_test_key.clone(),
        base_url: env.eleven_base_url.clone(),
        test_api_key: env.eleven_test_key.clone(),
        capture: project
            .app_capture
            .enabled
            .then(|| crate::capture::CaptureSettings { apps: project.app_capture.apps.clone() }),
    };
    let context_file = context_dir.join("context.json");
    paths::write_json_atomic(&context_file, &helper_context)?;
    let policy = Policy {
        mode,
        work_dir: folder.clone(),
        read_paths: read_paths.clone(),
        write_files: None,
        cutline: Some(Utf8PathBuf::from(&env.cutline.path)),
        helper: Some(env.helper.clone()),
        capture: project.app_capture.enabled && request.kind != RunKind::AudioReview,
        audio_review: request.kind == RunKind::AudioReview,
        path_dirs: vec![bin.clone()],
        web_access: rc.web_access,
        log: Some(logs.join("permissions.ndjson")),
    };
    let policy_file = context_dir.join("policy.json");
    paths::write_json_atomic(&policy_file, &policy)?;

    // An empty model in the request means "not chosen", as it does in the project.
    let requested_model = request.model.clone().filter(|m| !m.trim().is_empty());
    let model = match (&codex_exe, request.kind) {
        // Codex: the model chosen for this run, else Settings' Codex model, else Codex's default
        (Some(_), _) => requested_model
            .clone()
            .or_else(|| crate::codex::model_for(&env.settings.codex_model))
            .unwrap_or_else(|| CODEX_DEFAULT_MODEL.to_string()),
        (None, RunKind::DryRun) => requested_model.unwrap_or_else(|| project.models.dry_run_or(&env.settings)),
        (None, RunKind::Production | RunKind::AudioReview) => {
            requested_model.unwrap_or_else(|| rc.production_model.clone())
        }
    };
    let mut child_env = locate::child_env(env.claude.login_path.as_deref());
    let base_path = child_env.get("PATH").cloned().unwrap_or_default();
    let sep = if cfg!(windows) { ";" } else { ":" };
    child_env.insert("PATH".into(), format!("{bin}{sep}{base_path}"));
    child_env.insert(CONTEXT_ENV.into(), context_file.to_string());
    child_env.extend(env.extra_env.clone());

    let mut tools: Vec<String> = ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "AskUserQuestion", "TodoWrite"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if rc.web_access {
        tools.extend(["WebSearch".to_string(), "WebFetch".to_string()]);
    }
    let guide = locate::cutline_guide(Utf8Path::new(&env.cutline.path)).unwrap_or_default();
    let has_key = env.eleven_test_key.is_some() || env.eleven_key_present;
    let finish = project.finish_or(&env.settings);
    let has_cartesia_key = env.cartesia_test_key.is_some() || env.cartesia_key_present;
    let voice_choosing =
        eleven::voices::choosing_from(provider, env.settings.eleven_voice_preference, env.settings.eleven_voice_gender);
    let system = super::prompt::system_prompt(&super::prompt::PromptInput {
        project: &project,
        run_config: &rc,
        kind: request.kind,
        folder: &folder,
        outputs: &outputs,
        exports: &exports,
        manifest: &manifest_entries,
        cutline_guide: &guide,
        cutline_still: env.cutline.still,
        eleven_available: has_key,
        voice_provider: provider,
        cartesia_available: has_cartesia_key,
        default_voice: helper_context.default_voice.as_deref(),
        voice_choosing: Some(&voice_choosing),
        platforms: &env.platforms,
        finish: &finish,
    });
    let prompt = super::prompt::user_prompt(request.kind, request.revise.as_ref(), previous.as_ref());
    // What Claude was told, kept with the logs: the instructions are half of any run that went
    // wrong, and they change between versions of the app.
    paths::write_atomic(&logs.join("prompt-system.md"), system.as_bytes())?;
    paths::write_atomic(&logs.join("prompt.md"), prompt.as_bytes())?;
    let options = ClaudeOptions {
        executable: Utf8PathBuf::from(&env.claude.path),
        working_dir: folder.clone(),
        env: child_env,
        model: model.clone(),
        // The run config's effort, else the project's (Project › Models), else Settings.
        effort: rc
            .effort
            .clone()
            .filter(|e| !e.is_empty())
            .or_else(|| project.models.effort.clone().filter(|e| !e.is_empty()))
            .or_else(|| Some(env.settings.effort.clone()).filter(|e| !e.is_empty()))
            .filter(|e| env.claude.effort_levels.contains(e)),
        tools,
        resume_session: previous.as_ref().and_then(|p| p.claude_session.clone()),
        system_prompt_append: system,
        settings: Some(hook_settings(&env.helper, &policy_file)),
        add_dirs: read_paths.iter().map(|p| containing_dir(p)).collect::<HashSet<_>>().into_iter().collect(),
        max_budget_usd: if request.kind != RunKind::DryRun { rc.max_spend_usd } else { None },
        log: Some(logs.join("claude.jsonl")),
    };

    let codex = codex_exe.map(|exe| {
        let mut shell_env = std::collections::BTreeMap::new();
        for k in ["PATH", CONTEXT_ENV] {
            if let Some(v) = options.env.get(k) {
                shell_env.insert(k.to_string(), v.clone());
            }
        }
        crate::codex::agent::CodexOptions {
            executable: exe,
            working_dir: folder.clone(),
            env: options.env.clone(),
            shell_env,
            model: (model != CODEX_DEFAULT_MODEL).then(|| model.clone()),
            effort: crate::codex::agent::codex_effort(
                rc.effort
                    .as_deref()
                    .filter(|e| !e.is_empty())
                    .or(project.models.effort.as_deref().filter(|e| !e.is_empty()))
                    .or(Some(env.settings.effort.as_str()).filter(|e| !e.is_empty())),
            ),
            developer_instructions: format!("{}{CODEX_RUN_NOTE}", options.system_prompt_append),
            // only a Codex thread from the revised run is resumed
            resume_thread: previous
                .as_ref()
                .and_then(|p| p.claude_session.as_deref())
                .and_then(crate::codex::agent::thread_of)
                .map(str::to_string),
            writable_roots: vec![folder.clone()],
            web_access: rc.web_access,
            log: Some(logs.join("codex.jsonl")),
        }
    });

    let record = RunRecord {
        id: run_id.clone(),
        run_config: rc.id.clone(),
        run_config_name: Some(rc.name.clone()),
        kind: request.kind,
        folder: folder.to_string(),
        status: RunStatus::Running,
        started: timeutil::now_rfc3339(),
        finished: None,
        claude_session: None,
        model: Some(model.clone()),
        previous_run: previous.as_ref().map(|p| p.id.clone()),
        cost_usd: None,
        eleven_credits: None,
    };

    // run.json: the resolved config (inputs). logs/run.json grows the versions and the outcome.
    let resolved = json!({
        "run_id": run_id,
        "kind": request.kind,
        "project": {"id": project.id, "name": project.name, "path": request.project_path},
        "run_config": rc,
        "model": model,
        "quality": rc.quality,
        "locales": if rc.locales.is_empty() { project.intent.locales.clone() } else { rc.locales.clone() },
        "intent": project.intent,
        "finish": {"from": if project.finish.is_some() { "project" } else { "settings" }, "summary": finish.summary(), "choices": finish},
        "outputs": outputs,
        "cutline": exports,
        "previous_run": previous.as_ref().map(|p| json!({"id": p.id, "folder": p.folder})),
        "revise_feedback": request.revise.as_ref().map(|r| r.feedback.clone()),
    });
    paths::write_json_atomic(&folder.join("run.json"), &resolved)?;
    let mut log_run = resolved.clone();
    if let Some(o) = log_run.as_object_mut() {
        o.insert("app".into(), json!({"name": crate::APP_NAME, "core_version": CORE_VERSION}));
        match &codex {
            Some(c) => {
                o.insert("provider".into(), json!("codex"));
                o.insert(
                    "codex".into(),
                    json!({"path": c.executable, "model": c.model, "sandbox": c.sandbox_policy()}),
                );
            }
            None => {
                o.insert("provider".into(), json!("claude"));
                o.insert("claude".into(), json!({"path": env.claude.path, "version": env.claude.version_text}));
            }
        }
        o.insert("cutline".into(), json!({"path": env.cutline.path, "version": env.cutline.version, "doctor": env.cutline_doctor.as_ref().map(doctor_brief)}));
        o.insert(
            "os".into(),
            json!({"family": std::env::consts::OS, "arch": std::env::consts::ARCH, "version": crate::platform::os_version()}),
        );
        o.insert("tools".into(), json!(options.tools));
        o.insert("effort".into(), json!(options.effort));
        o.insert("folder".into(), json!(folder));
        o.insert("started".into(), json!(record.started));
        o.insert("status".into(), json!("running"));
    }
    paths::write_json_atomic(&logs.join("run.json"), &log_run)?;
    if let Some(v) = &request.validation {
        paths::write_json_atomic(&logs.join("validation.json"), v)?;
    }

    Ok(Prepared {
        run_id,
        folder,
        logs,
        context_dir,
        mode,
        project,
        run_config: rc,
        outputs,
        manifest: manifest_entries,
        policy,
        options,
        prompt,
        record,
        helper_context,
        request: request.clone(),
        codex,
    })
}

/// The model a Codex run records when none is chosen: Codex picks its own default.
pub const CODEX_DEFAULT_MODEL: &str = "codex-default";

/// The parts of `cutline doctor --json` worth keeping in every run log.
fn doctor_brief(d: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for k in [
        "ok",
        "errors",
        "warnings",
        "home",
        "home_source",
        "platform",
        "chrome",
        "whisper_cli",
        "whisper_models",
        "face_model",
        "fixes",
    ] {
        if let Some(v) = d.get(k) {
            out.insert(k.into(), v.clone());
        }
    }
    if let Some(ff) = d.get("ffmpeg") {
        out.insert("ffmpeg".into(), ff.clone());
    }
    let missing: Vec<Value> = d["capabilities"]
        .as_array()
        .map(|a| a.iter().filter(|c| c["present"] == false && c["required"] == true).cloned().collect())
        .unwrap_or_default();
    out.insert("missing_required".into(), Value::Array(missing));
    Value::Object(out)
}

/// The output an Audio Review auditions: the one named (by ID or name), else the first.
pub fn audio_review_output(outputs: &[ResolvedOutput], want: Option<&str>) -> Result<ResolvedOutput> {
    match want.map(str::trim).filter(|w| !w.is_empty()) {
        None => Ok(outputs[0].clone()),
        Some(w) => outputs.iter().find(|o| o.id == w || o.name == w).cloned().ok_or_else(|| {
            let names: Vec<&str> = outputs.iter().map(|o| o.name.as_str()).collect();
            Error::InvalidProject(format!("no output \"{w}\" to audition (outputs: {})", names.join(", ")))
        }),
    }
}
