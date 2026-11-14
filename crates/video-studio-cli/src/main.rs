//! `video-studio`: Syncfusion Video Studio without the UI (ui/spec.md §13), and the helper the
//! app gives Claude during runs (§14).
//!
//! The same binary plays three small extra roles so nothing else has to be installed:
//! - `video-studio eleven …` — the ElevenLabs helper, usable only inside a run;
//! - `video-studio hook` — the pre-tool permission hook Claude Code calls;
//! - invoked under the name `cutline` (a link in the run folder's `.bin`), it runs the real
//!   cutline and keeps the per-invocation logs.

mod eleven_cmd;
mod progress;

use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use video_studio_core::claude::broker::{hook_response, Policy};
use video_studio_core::eleven::HelperContext;
use video_studio_core::media::{self, MediaStatus};
use video_studio_core::platform::{NoFileRefs, OsSecrets, Secrets, ELEVEN_KEY};
use video_studio_core::presets::resolve_run_outputs;
use video_studio_core::project::{IntentKind, Project, RunKind, RunStatus};
use video_studio_core::run::events::EventSink;
use video_studio_core::run::{exit_code, start_run, RunEnv, RunHandle, RunRequest};
use video_studio_core::validation::{self, Report, Severity, SystemEnv, ValidateInput};
use video_studio_core::{logs, paths, project, Error};

#[derive(Parser)]
#[command(
    name = "video-studio",
    version,
    about = "Syncfusion Video Studio: prepare and produce videos with Claude Code and cutline, without the UI"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check a project before a run: static checks, and optionally a cheap dry run
    Validate {
        project: Utf8PathBuf,
        /// Run config (name or ID) to validate; without it only the project is checked
        #[arg(long)]
        config: Option<String>,
        /// Also do the dry run: Claude writes project.json5 and runs `cutline validate` and a review
        #[arg(long)]
        dry_run: bool,
        /// Print the validation report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Run one or more run configs: validate, dry run (unless skipped), then produce the outputs
    Run {
        project: Utf8PathBuf,
        /// Run config (name or ID); repeat to run several one after another
        #[arg(long, required = true)]
        config: Vec<String>,
        /// Production model for this run: a CLI alias (opus) or a full model ID
        #[arg(long)]
        model: Option<String>,
        /// Parent folder for the run folder, instead of the run config's
        #[arg(long)]
        output: Option<Utf8PathBuf>,
        /// Skip the dry run even if the run config asks for one
        #[arg(long)]
        no_dry_run: bool,
        /// Don't ask anything: accept warnings, and let Claude proceed on its stated assumption
        #[arg(long)]
        yes: bool,
        /// Write NDJSON events to stdout instead of progress lines
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = "info", value_parser = ["info", "debug"])]
        log_level: String,
    },
    /// Show run configs, outputs and media status
    List {
        project: Utf8PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Find media that moved: search a folder, or rewrite a path prefix
    Relink {
        project: Utf8PathBuf,
        /// Folder to scan for the missing files (matched by name, then fingerprint)
        #[arg(long)]
        search: Option<Utf8PathBuf>,
        /// Rewrite a path prefix: `<old>=<new>`
        #[arg(long)]
        prefix: Option<String>,
        /// Save the project; without this the changes are only shown
        #[arg(long)]
        write: bool,
        #[arg(long)]
        json: bool,
    },
    /// Run history with folders and status
    Runs {
        project: Utf8PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// List a run's logs, export them as a zip, or print the issue report
    Logs {
        run_folder: Utf8PathBuf,
        /// Write the log folder and project.json5 to this zip file
        #[arg(long)]
        zip: Option<Utf8PathBuf>,
        /// With --zip: replace media paths and file names with placeholders
        #[arg(long)]
        redact: bool,
        /// Print a Markdown report of cutline errors, warnings and failed QA checks
        #[arg(long)]
        issue_report: bool,
    },
    /// Create a project file from a template
    New {
        project: Utf8PathBuf,
        #[arg(long)]
        name: Option<String>,
        /// marketing, product_demo, tutorial, explainer, social_clip, testimonial, announcement, event_recap, internal, app_store_preview, other
        #[arg(long, default_value = "marketing")]
        intent: String,
        /// Media files or folders to reference
        #[arg(long)]
        media: Vec<Utf8PathBuf>,
    },
    /// ElevenLabs helper used by Claude during a run (voice, music, effects, transcripts)
    Eleven {
        #[command(subcommand)]
        command: eleven_cmd::ElevenCommand,
    },
    /// Store, check or remove the ElevenLabs API key in the system keychain
    Secret {
        /// set (key on stdin), status, test or delete
        #[arg(value_parser = ["set", "status", "test", "delete"])]
        action: String,
        #[arg(long)]
        json: bool,
    },
    /// Pre-tool permission hook for Claude Code (reads the tool call on stdin)
    #[command(hide = true)]
    Hook {
        #[arg(long)]
        policy: Utf8PathBuf,
    },
}

fn library() -> Utf8PathBuf {
    paths::library_dir()
}

fn helper_path() -> Utf8PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("video-studio"))
}

fn secrets() -> Box<dyn Secrets> {
    // Tests can't use the real keychain: a file named by this variable stands in for it.
    match std::env::var("VIDEO_STUDIO_TEST_SECRETS") {
        Ok(path) if !path.is_empty() => Box::new(FileSecrets(Utf8PathBuf::from(path))),
        _ => Box::new(OsSecrets),
    }
}

/// A JSON file used as the secret store in tests.
struct FileSecrets(Utf8PathBuf);

impl FileSecrets {
    fn read(&self) -> BTreeMap<String, String> {
        std::fs::read_to_string(&self.0).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
}

impl Secrets for FileSecrets {
    fn get(&self, name: &str) -> video_studio_core::Result<Option<String>> {
        Ok(self.read().get(name).cloned())
    }
    fn set(&self, name: &str, value: &str) -> video_studio_core::Result<()> {
        let mut m = self.read();
        m.insert(name.into(), value.into());
        paths::write_json_atomic(&self.0, &m)
    }
    fn delete(&self, name: &str) -> video_studio_core::Result<()> {
        let mut m = self.read();
        m.remove(name);
        paths::write_json_atomic(&self.0, &m)
    }
}

fn fail(e: &Error) -> i32 {
    eprintln!("error: {e}");
    match e {
        Error::Environment(_) => exit_code::ENVIRONMENT,
        Error::InvalidProject(_) | Error::NotFound(_) => exit_code::VALIDATION_BLOCKERS,
        _ => exit_code::RUN_FAILED,
    }
}

fn main() {
    // Called as `cutline` from a run folder's .bin: act as the logging proxy.
    let argv0 = std::env::args().next().unwrap_or_default();
    let called_as = Utf8Path::new(&argv0).file_stem().unwrap_or("").to_ascii_lowercase();
    if called_as == "cutline" {
        let code = match HelperContext::from_env() {
            Ok(ctx) => video_studio_core::run::proxy::run(&ctx, &std::env::args().skip(1).collect::<Vec<_>>()),
            Err(e) => {
                eprintln!("{e}");
                126
            }
        };
        std::process::exit(code);
    }
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Validate { project, config, dry_run, json } => {
            cmd_validate(&project, config.as_deref(), dry_run, json)
        }
        Command::Run { project, config, model, output, no_dry_run, yes, json, log_level } => {
            cmd_run(&project, &config, model, output, no_dry_run, yes, json, log_level == "debug")
        }
        Command::List { project, json } => cmd_list(&project, json),
        Command::Relink { project, search, prefix, write, json } => {
            cmd_relink(&project, search.as_deref(), prefix.as_deref(), write, json)
        }
        Command::Runs { project, json } => cmd_runs(&project, json),
        Command::Logs { run_folder, zip, redact, issue_report } => {
            cmd_logs(&run_folder, zip.as_deref(), redact, issue_report)
        }
        Command::New { project, name, intent, media } => cmd_new(&project, name.as_deref(), &intent, &media),
        Command::Eleven { command } => eleven_cmd::run(command, secrets().as_ref()),
        Command::Secret { action, json } => cmd_secret(&action, json),
        Command::Hook { policy } => cmd_hook(&policy),
    };
    std::process::exit(code);
}

// MARK: validate

struct Loaded {
    project: Project,
    path: Utf8PathBuf,
}

fn load(path: &Utf8Path) -> Result<Loaded, Error> {
    let abs = paths::absolutize(
        path,
        &std::env::current_dir()
            .ok()
            .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
            .unwrap_or_else(|| Utf8PathBuf::from(".")),
    );
    Ok(Loaded { project: Project::load(&abs)?, path: abs })
}

/// Static validation of a project (and one run config) on this machine. Probes media that hasn't
/// been probed when cutline is available.
fn static_validation(
    l: &mut Loaded,
    config: Option<&str>,
    kind: RunKind,
    output: Option<&Utf8Path>,
) -> Result<Report, Error> {
    let lib = library();
    let settings = video_studio_core::settings::Settings::load(&lib);
    let platforms = video_studio_core::presets::Platforms::load(&lib);
    let models = video_studio_core::models::ModelList::load(&lib);
    let rc = match config {
        Some(c) => Some(l.project.run_config(c).cloned().ok_or_else(|| {
            let names: Vec<&str> = l.project.run_configs.iter().map(|r| r.name.as_str()).collect();
            Error::NotFound(format!(
                "no run config \"{c}\" (the project has: {})",
                if names.is_empty() { "none".into() } else { names.join(", ") }
            ))
        })?),
        None => None,
    };
    let resolutions = media::resolve_all(&l.project, Some(&l.path), &settings.media_roots, &NoFileRefs);
    if let Some(cutline) =
        video_studio_core::claude::locate::find_cutline(&settings.cutline_path, &settings.cutline_repo)
    {
        // Probe data is a cache: fill it in memory for the checks without touching the file.
        media::probe_all(&mut l.project, &resolutions, Utf8Path::new(&cutline.path));
    }
    let secrets = secrets();
    let env = SystemEnv {
        settings: &settings,
        secrets: secrets.as_ref(),
        eleven_base_url: std::env::var("VIDEO_STUDIO_ELEVEN_BASE_URL").ok().filter(|s| !s.is_empty()),
    };
    Ok(validation::validate(&ValidateInput {
        project: &l.project,
        project_path: Some(&l.path),
        run_config: rc.as_ref(),
        kind,
        settings: &settings,
        platforms: &platforms,
        models: &models,
        library: &lib,
        resolutions: &resolutions,
        env: &env,
        output_override: output,
    }))
}

fn print_report(report: &Report, to_stderr: bool) {
    let mut out: Box<dyn Write> = if to_stderr { Box::new(std::io::stderr()) } else { Box::new(std::io::stdout()) };
    for i in &report.issues {
        let tag = match i.severity {
            Severity::Blocker => "BLOCKER",
            Severity::Warning => "warning",
            Severity::Info => "info   ",
        };
        let at = i.fix.as_ref().map(|f| match (&f.target, f.line) {
            (_, Some(line)) => format!(" (spec line {line})"),
            (Some(t), _) => format!(" ({}: {t})", f.kind),
            _ => format!(" ({})", f.kind),
        });
        let _ = writeln!(out, "{tag}  {}{}", i.title, at.unwrap_or_default());
        if let Some(d) = &i.detail {
            let _ = writeln!(out, "         {d}");
        }
    }
    let e = &report.estimates;
    let _ = writeln!(out, "{} blocker(s), {} warning(s), {} note(s)", report.blockers, report.warnings, report.infos);
    if e.claude_usd_high > 0.0 {
        let _ =
            writeln!(out, "Estimated Claude cost: ${:.2}–${:.2} (approximate)", e.claude_usd_low, e.claude_usd_high);
    }
    if e.eleven_credits > 0 {
        let _ = writeln!(out, "Estimated ElevenLabs credits: {} of a {} cap", e.eleven_credits, e.eleven_cap);
    }
}

fn environment_missing(report: &Report) -> bool {
    report.issues.iter().any(|i| {
        i.severity == Severity::Blocker
            && matches!(i.rule.as_str(), "claude_missing" | "claude_unsupported" | "cutline_missing")
    })
}

fn report_exit(report: &Report) -> i32 {
    if environment_missing(report) {
        exit_code::ENVIRONMENT
    } else if report.has_blockers() {
        exit_code::VALIDATION_BLOCKERS
    } else if report.warnings > 0 {
        exit_code::SUCCESS_WITH_WARNINGS
    } else {
        exit_code::SUCCESS
    }
}

fn cmd_validate(project: &Utf8Path, config: Option<&str>, dry_run: bool, json: bool) -> i32 {
    let mut l = match load(project) {
        Ok(l) => l,
        Err(e) => return fail(&e),
    };
    if dry_run && config.is_none() {
        eprintln!("error: --dry-run needs --config <run-config>");
        return exit_code::VALIDATION_BLOCKERS;
    }
    let mut report =
        match static_validation(&mut l, config, if dry_run { RunKind::DryRun } else { RunKind::Production }, None) {
            Ok(r) => r,
            Err(e) => return fail(&e),
        };
    if dry_run && !report.has_blockers() {
        let request = RunRequest {
            project_path: l.path.clone(),
            run_config: config.unwrap_or_default().to_string(),
            kind: RunKind::DryRun,
            model: None,
            output: None,
            revise: None,
            assume_answers: true,
            question_timeout_s: None,
            validation: Some(report.clone()),
        };
        match execute_run(&request, json, false, true) {
            Err(e) => return fail(&e),
            Ok(outcome) => {
                if let Some(r) = &outcome.readiness {
                    report.add_dry_run(r);
                    let _ =
                        paths::write_json_atomic(&Utf8Path::new(&outcome.folder).join("logs/validation.json"), &report);
                }
                if outcome.status == RunStatus::Failed || outcome.status == RunStatus::Cancelled {
                    if json {
                        println!("{}", serde_json::to_string(&report).unwrap_or_default());
                    } else {
                        print_report(&report, false);
                    }
                    eprintln!(
                        "The dry run {}: {}",
                        if outcome.status == RunStatus::Cancelled { "was cancelled" } else { "failed" },
                        outcome.message.unwrap_or_default()
                    );
                    return outcome.exit_code;
                }
            }
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    } else {
        print_report(&report, false);
    }
    report_exit(&report)
}

// MARK: run

static INTERRUPTS: AtomicU32 = AtomicU32::new(0);

fn confirm(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Starts a run and follows it to the end: progress or NDJSON, questions on a terminal, Ctrl-C.
fn execute_run(
    request: &RunRequest,
    json: bool,
    debug: bool,
    quiet_header: bool,
) -> Result<video_studio_core::run::RunOutcome, Error> {
    let lib = library();
    let mut env = RunEnv::detect(&lib, &helper_path())?;
    let secrets = secrets();
    env.eleven_key_present = matches!(secrets.get(ELEVEN_KEY), Ok(Some(k)) if !k.is_empty());
    env.eleven_test_key = std::env::var("VIDEO_STUDIO_TEST_ELEVEN_KEY").ok().filter(|k| !k.is_empty());
    let interactive =
        !json && std::io::stdin().is_terminal() && std::io::stderr().is_terminal() && !request.assume_answers;
    let mut request = request.clone();
    if !interactive {
        request.assume_answers = true;
    }
    if request.question_timeout_s.is_none() && env.settings.headless_question_timeout_s > 0 {
        request.question_timeout_s = Some(env.settings.headless_question_timeout_s);
    }
    let handle_slot: Arc<Mutex<Option<Arc<RunHandle>>>> = Arc::new(Mutex::new(None));
    let (question_tx, question_rx) = std::sync::mpsc::channel::<progress::PendingQuestion>();
    let sink: Arc<dyn EventSink> = if json {
        Arc::new(progress::JsonSink)
    } else {
        Arc::new(progress::TerminalSink::new(debug, quiet_header, interactive.then_some(question_tx)))
    };
    let handle = Arc::new(start_run(&request, &env, sink)?);
    *handle_slot.lock().unwrap() = Some(handle.clone());

    // Ctrl-C: a graceful stop first, a kill on the second (Ctrl-Break does the same on Windows).
    let for_signal = handle.clone();
    let _ = ctrlc::set_handler(move || {
        if INTERRUPTS.fetch_add(1, Ordering::SeqCst) == 0 {
            eprintln!("\nStopping… (press Ctrl-C again to kill immediately)");
            for_signal.cancel();
        } else {
            for_signal.kill();
        }
    });
    if interactive {
        let for_questions = handle.clone();
        std::thread::spawn(move || {
            for q in question_rx {
                let answers = progress::ask_on_terminal(&q);
                for_questions.answer(&q.id, answers);
            }
        });
    }
    Ok(handle.wait())
}

#[allow(clippy::too_many_arguments)]
fn cmd_run(
    project: &Utf8Path,
    configs: &[String],
    model: Option<String>,
    output: Option<Utf8PathBuf>,
    no_dry_run: bool,
    yes: bool,
    json: bool,
    debug: bool,
) -> i32 {
    let mut worst = exit_code::SUCCESS;
    let interactive = !json && !yes && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    for config in configs {
        let mut l = match load(project) {
            Ok(l) => l,
            Err(e) => return fail(&e),
        };
        // Validation always runs first; a run can't start with blockers.
        let mut report = match static_validation(&mut l, Some(config), RunKind::Production, output.as_deref()) {
            Ok(r) => r,
            Err(e) => return fail(&e),
        };
        if report.has_blockers() {
            if json {
                println!("{}", json!({"event": "validation", "report": report}));
            }
            print_report(&report, true);
            eprintln!("\"{config}\" can't run until the blockers are fixed.");
            return report_exit(&report);
        }
        if report.warnings > 0 {
            print_report(&report, true);
            if interactive && !confirm(&format!("Run \"{config}\" with {} warning(s)?", report.warnings)) {
                return exit_code::VALIDATION_BLOCKERS;
            }
        }
        let rc = l.project.run_config(config).cloned().expect("validated above");
        if rc.dry_run_first && !no_dry_run {
            if !json {
                eprintln!("Dry run for \"{}\" ({})…", rc.name, l.project.models.dry_run);
            }
            let dry = RunRequest {
                project_path: l.path.clone(),
                run_config: rc.id.clone(),
                kind: RunKind::DryRun,
                model: None,
                output: None,
                revise: None,
                assume_answers: yes,
                question_timeout_s: None,
                validation: Some(report.clone()),
            };
            match execute_run(&dry, json, debug, true) {
                Err(e) => return fail(&e),
                Ok(outcome) => {
                    if outcome.status == RunStatus::Cancelled {
                        return exit_code::CANCELLED;
                    }
                    if outcome.status == RunStatus::Failed {
                        eprintln!(
                            "The dry run failed: {}\nLogs: {}/logs",
                            outcome.message.unwrap_or_default(),
                            outcome.folder
                        );
                        return exit_code::RUN_FAILED;
                    }
                    if let Some(r) = &outcome.readiness {
                        report.add_dry_run(r);
                        let findings: Vec<_> = report.issues.iter().filter(|i| i.category == "dry_run").collect();
                        if !findings.is_empty() {
                            eprintln!("The dry run found {} point(s):", findings.len());
                            for f in &findings {
                                eprintln!(
                                    "  - {}{}",
                                    f.title,
                                    f.fix
                                        .as_ref()
                                        .and_then(|x| x.line)
                                        .map(|l| format!(" (spec line {l})"))
                                        .unwrap_or_default()
                                );
                            }
                            let proceed = if interactive { confirm("Continue to the production run?") } else { yes };
                            if !proceed {
                                eprintln!("Stopped before the production run. Fix the spec, or pass --yes to proceed anyway (or --no-dry-run to skip the dry run).");
                                return exit_code::VALIDATION_BLOCKERS;
                            }
                        }
                    }
                }
            }
        }
        let request = RunRequest {
            project_path: l.path.clone(),
            run_config: rc.id.clone(),
            kind: RunKind::Production,
            model: model.clone(),
            output: output.clone(),
            revise: None,
            assume_answers: yes,
            question_timeout_s: None,
            validation: Some(report),
        };
        match execute_run(&request, json, debug, false) {
            Err(e) => return fail(&e),
            Ok(outcome) => {
                if !json {
                    eprintln!();
                    eprintln!("{}: {}", rc.name, outcome.status.label());
                    for o in &outcome.outputs {
                        if o.exists {
                            eprintln!(
                                "  {}  ({}, QA {}){}",
                                o.path,
                                paths::format_bytes(o.bytes),
                                o.qa_verdict,
                                if o.incomplete { "  INCOMPLETE" } else { "" }
                            );
                        } else {
                            eprintln!("  {}  MISSING", o.path);
                        }
                    }
                    if let Some(m) = &outcome.message {
                        eprintln!("  {m}");
                    }
                    eprintln!("  Logs: {}/logs", outcome.folder);
                }
                worst = worst.max(outcome.exit_code);
                if outcome.status == RunStatus::Cancelled {
                    return exit_code::CANCELLED;
                }
            }
        }
    }
    worst
}

// MARK: list, relink, runs, logs, new

fn status_word(s: MediaStatus) -> &'static str {
    match s {
        MediaStatus::Ok => "ok",
        MediaStatus::Moved => "moved",
        MediaStatus::Changed => "changed",
        MediaStatus::Missing => "MISSING",
    }
}

fn cmd_list(project: &Utf8Path, as_json: bool) -> i32 {
    let l = match load(project) {
        Ok(l) => l,
        Err(e) => return fail(&e),
    };
    let lib = library();
    let settings = video_studio_core::settings::Settings::load(&lib);
    let platforms = video_studio_core::presets::Platforms::load(&lib);
    let res = media::resolve_all(&l.project, Some(&l.path), &settings.media_roots, &NoFileRefs);
    let p = &l.project;
    if as_json {
        let configs: Vec<Value> = p
            .run_configs
            .iter()
            .map(|rc| {
                let outs = resolve_run_outputs(p, rc, &platforms).unwrap_or_default();
                json!({"id": rc.id, "name": rc.name, "production_model": rc.production_model, "quality": rc.quality, "dry_run_first": rc.dry_run_first, "output_folder": rc.output_root(&p.name, Some(&settings.default_output_folder)), "outputs": outs})
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json!({"project": {"id": p.id, "name": p.name, "intent": p.intent, "path": l.path}, "run_configs": configs, "media": res})).unwrap_or_default());
        return 0;
    }
    println!("{}  ({})", p.name, l.path);
    println!("Intent: {}", p.intent.describe());
    if let Some(b) = &p.brand {
        println!("Brand: {}", b.snapshot.name);
    }
    println!("\nRun configs:");
    if p.run_configs.is_empty() {
        println!("  (none)");
    }
    for rc in &p.run_configs {
        println!(
            "  {}  [{}]  model {}, {}{}",
            rc.name,
            rc.id,
            rc.production_model,
            if rc.quality == project::Quality::Draft { "draft" } else { "final" },
            if rc.dry_run_first { ", dry run first" } else { "" }
        );
        println!("    → {}", rc.output_root(&p.name, Some(&settings.default_output_folder)));
        match resolve_run_outputs(p, rc, &platforms) {
            Ok(outs) => {
                for o in outs {
                    println!("    - {}  {}×{} {} ({})", o.name, o.width, o.height, o.aspect_label(), o.target);
                }
            }
            Err(e) => println!("    ! {e}"),
        }
    }
    println!("\nMedia:");
    if p.media.is_empty() {
        println!("  (none)");
    }
    for m in &p.media {
        let r = res.iter().find(|r| r.id == m.id);
        let status = r.map(|r| status_word(r.status)).unwrap_or("MISSING");
        let dur = m
            .probe
            .as_ref()
            .and_then(|x| x.duration_s)
            .map(|d| format!("  {}", media::format_duration(d)))
            .unwrap_or_default();
        println!(
            "  {:<8} {:<24} {:<10}{dur}  {}",
            status,
            m.id,
            m.kind.key(),
            r.and_then(|r| r.path.clone().or(r.stored_path.clone())).unwrap_or_default()
        );
    }
    let missing = res.iter().filter(|r| r.status == MediaStatus::Missing).count();
    if missing > 0 {
        println!("\n{missing} media item(s) missing. Try: video-studio relink {} --search <folder>", l.path);
    }
    0
}

fn cmd_relink(project: &Utf8Path, search: Option<&Utf8Path>, prefix: Option<&str>, write: bool, as_json: bool) -> i32 {
    let mut l = match load(project) {
        Ok(l) => l,
        Err(e) => return fail(&e),
    };
    if search.is_none() && prefix.is_none() {
        eprintln!("error: give --search <dir> and/or --prefix <old>=<new>");
        return exit_code::VALIDATION_BLOCKERS;
    }
    let settings = video_studio_core::settings::Settings::load(&library());
    let before = media::resolve_all(&l.project, Some(&l.path), &settings.media_roots, &NoFileRefs);
    // Items found by relative path or a search root are updated first, as the app does on open.
    let mut changed = media::apply_resolutions(&mut l.project, Some(&l.path), &before, &NoFileRefs);
    let mut relinked: Vec<String> = Vec::new();
    if let Some(p) = prefix {
        let Some((old, new)) = p.split_once('=') else {
            eprintln!("error: --prefix takes <old>=<new>");
            return exit_code::VALIDATION_BLOCKERS;
        };
        let fix = media::PrefixFix {
            old_prefix: old.trim_end_matches(['/', '\\']).to_string(),
            new_prefix: new.trim_end_matches(['/', '\\']).to_string(),
        };
        relinked.extend(media::apply_prefix_fix(&mut l.project, Some(&l.path), &fix, &NoFileRefs));
    }
    if let Some(dir) = search {
        match media::search_folder(&mut l.project, Some(&l.path), dir, &NoFileRefs) {
            Ok(ids) => relinked.extend(ids),
            Err(e) => return fail(&e),
        }
    }
    changed |= !relinked.is_empty();
    let after = media::resolve_all(&l.project, Some(&l.path), &settings.media_roots, &NoFileRefs);
    let still: Vec<&media::Resolution> = after.iter().filter(|r| r.status == MediaStatus::Missing).collect();
    if as_json {
        println!("{}", serde_json::to_string_pretty(&json!({"relinked": relinked, "still_missing": still.iter().map(|r| &r.id).collect::<Vec<_>>(), "written": write && changed, "media": after})).unwrap_or_default());
    } else {
        for id in &relinked {
            let r = after.iter().find(|r| &r.id == id);
            println!(
                "relinked  {id}  → {}{}",
                r.and_then(|r| r.path.clone()).unwrap_or_default(),
                if r.map(|r| r.status) == Some(MediaStatus::Changed) {
                    "  (content differs: shown as Changed)"
                } else {
                    ""
                }
            );
        }
        for r in &still {
            println!("missing   {}  (was {})", r.id, r.stored_path.clone().unwrap_or_default());
        }
        println!("{} relinked, {} still missing.", relinked.len(), still.len());
    }
    if write && changed {
        if let Err(e) = l.project.save(&l.path) {
            return fail(&e);
        }
        if !as_json {
            println!("Saved {}", l.path);
        }
    } else if changed && !as_json {
        println!("Nothing was saved. Add --write to save these changes.");
    }
    if still.is_empty() {
        0
    } else {
        exit_code::SUCCESS_WITH_WARNINGS
    }
}

fn cmd_runs(project: &Utf8Path, as_json: bool) -> i32 {
    let l = match load(project) {
        Ok(l) => l,
        Err(e) => return fail(&e),
    };
    if as_json {
        println!("{}", serde_json::to_string_pretty(&l.project.runs).unwrap_or_default());
        return 0;
    }
    if l.project.runs.is_empty() {
        println!("No runs yet.");
    }
    for r in &l.project.runs {
        let kind = if r.kind == RunKind::DryRun { "dry run   " } else { "production" };
        let cost = r.cost_usd.map(|c| format!("  ${c:.2}")).unwrap_or_default();
        let exists = if Utf8Path::new(&r.folder).is_dir() { "" } else { "  (folder not found)" };
        println!(
            "{}  {kind}  {:<24} {:<24}{cost}",
            r.started,
            r.status.label(),
            r.run_config_name.clone().unwrap_or_else(|| r.run_config.clone())
        );
        println!("    {}{exists}", r.folder);
    }
    0
}

fn cmd_logs(run_folder: &Utf8Path, zip: Option<&Utf8Path>, redact: bool, issue_report: bool) -> i32 {
    if !run_folder.join("logs").is_dir() {
        eprintln!("error: {run_folder} isn't a run folder (it has no logs/)");
        return exit_code::VALIDATION_BLOCKERS;
    }
    if issue_report {
        print!("{}", logs::issue_report(run_folder));
        return 0;
    }
    if let Some(dest) = zip {
        return match logs::export_logs(run_folder, dest, redact) {
            Ok(n) => {
                println!("Wrote {n} files to {dest}{}", if redact { " (media paths redacted)" } else { "" });
                0
            }
            Err(e) => fail(&e),
        };
    }
    for f in video_studio_core::run::collect::list_logs(run_folder) {
        println!("{:>10}  {:<10} {}", paths::format_bytes(f.bytes), f.source, f.path);
    }
    0
}

fn parse_intent(s: &str) -> Option<IntentKind> {
    IntentKind::ALL.into_iter().find(|k| k.key() == s)
}

fn cmd_new(path: &Utf8Path, name: Option<&str>, intent: &str, media_paths: &[Utf8PathBuf]) -> i32 {
    if path.exists() {
        eprintln!("error: {path} already exists");
        return exit_code::VALIDATION_BLOCKERS;
    }
    let Some(kind) = parse_intent(intent) else {
        eprintln!("error: unknown intent \"{intent}\"");
        return exit_code::VALIDATION_BLOCKERS;
    };
    let lib = library();
    let settings = video_studio_core::settings::Settings::load(&lib);
    let platforms = video_studio_core::presets::Platforms::load(&lib);
    let name = name.map(str::to_string).unwrap_or_else(|| path.file_stem().unwrap_or("Untitled").to_string());
    let mut p = Project::new(&name);
    p.intent.kind = kind;
    let defaults = platforms.intent_defaults(kind);
    p.spec = video_studio_core::spec::template_for(&defaults.template, &name);
    p.models.assistant = settings.default_assistant_model.clone();
    p.models.dry_run = settings.default_dry_run_model.clone();
    let mut taken = std::collections::HashSet::new();
    for preset in defaults.outputs.iter().filter_map(|id| platforms.preset(id)) {
        let cfg = video_studio_core::presets::OutputConfig::from_preset(preset, &taken);
        taken.insert(cfg.id.clone());
        p.output_configs.push(cfg);
    }
    let mut rc = project::RunConfig::new("Default");
    rc.production_model = settings.default_production_model.clone();
    rc.eleven_credit_cap = settings.eleven_default_credit_cap;
    rc.outputs = p.output_configs.iter().map(|o| o.id.clone()).collect();
    p.run_configs.push(rc);
    let abs = paths::absolutize(
        path,
        &std::env::current_dir()
            .ok()
            .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
            .unwrap_or_else(|| Utf8PathBuf::from(".")),
    );
    for m in media_paths {
        let r = if m.is_dir() {
            media::add_folder(&mut p, Some(&abs), m, None, &NoFileRefs).map(|_| ())
        } else {
            media::add_media(&mut p, Some(&abs), m, &NoFileRefs).map(|_| ())
        };
        if let Err(e) = r {
            return fail(&e);
        }
    }
    match p.save(&abs) {
        Ok(()) => {
            println!(
                "Created {abs} ({} template, {} output(s), {} media item(s))",
                defaults.template,
                p.output_configs.len(),
                p.media.len()
            );
            0
        }
        Err(e) => fail(&e),
    }
}

// MARK: secret, hook

fn cmd_secret(action: &str, as_json: bool) -> i32 {
    let secrets = secrets();
    let base = std::env::var("VIDEO_STUDIO_ELEVEN_BASE_URL").ok().filter(|s| !s.is_empty());
    let result: Result<Value, Error> = match action {
        "set" => {
            let mut key = String::new();
            let _ = std::io::stdin().read_to_string(&mut key);
            let key = key.trim();
            if key.is_empty() {
                Err(Error::other("no key was given on stdin"))
            } else {
                secrets.set(ELEVEN_KEY, key).map(|_| json!({"present": true}))
            }
        }
        "delete" => secrets.delete(ELEVEN_KEY).map(|_| json!({"present": false})),
        "status" => secrets.get(ELEVEN_KEY).map(|k| {
            // Shown masked: the last four characters only.
            let masked = k.as_deref().filter(|k| !k.is_empty()).map(|k| format!("••••{}", k.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>()));
            json!({"present": masked.is_some(), "masked": masked})
        }),
        "test" => match secrets.get(ELEVEN_KEY) {
            Ok(Some(k)) if !k.is_empty() => video_studio_core::eleven::test_key(&k, base.as_deref()).map(|s| json!({"present": true, "ok": true, "tier": s.tier, "used": s.character_count, "limit": s.character_limit, "remaining": s.remaining()})),
            Ok(_) => Err(Error::Environment("no ElevenLabs key is stored".into())),
            Err(e) => Err(e),
        },
        _ => unreachable!("clap limits the actions"),
    };
    match result {
        Ok(v) => {
            if as_json {
                println!("{v}");
            } else {
                match action {
                    "status" => println!(
                        "{}",
                        v["masked"]
                            .as_str()
                            .map(|m| format!("ElevenLabs key: {m}"))
                            .unwrap_or_else(|| "No ElevenLabs key is stored.".into())
                    ),
                    "test" => println!(
                        "The key works. {} of {} credits remain ({}).",
                        v["remaining"],
                        v["limit"],
                        v["tier"].as_str().unwrap_or("")
                    ),
                    "set" => println!("Stored."),
                    _ => println!("Removed."),
                }
            }
            0
        }
        Err(e) => {
            if as_json {
                println!("{}", json!({"ok": false, "error": e.to_string()}));
                1
            } else {
                fail(&e)
            }
        }
    }
}

fn cmd_hook(policy_file: &Utf8Path) -> i32 {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let call: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let out = match paths::read_json::<Policy>(policy_file) {
        Ok(policy) => hook_response(&policy, &call),
        // No policy means the run is over or was tampered with: refuse everything.
        Err(_) => {
            json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": "This session's permission policy is missing, so tool calls are blocked by Syncfusion Video Studio."}})
        }
    };
    println!("{out}");
    0
}
