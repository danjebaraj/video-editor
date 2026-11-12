//! `cutline`: a declarative command-line video editor driven by Claude.

mod deps_cmd;
mod doctor;
mod edit;
mod out;

use clap::{Parser, Subcommand};
use out::*;
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "cutline", version, about = "Declarative CLI video editor for Shorts, short and long-form video")]
struct Cli {
    /// Machine-readable JSON on stdout (logs go to stderr).
    #[arg(long, global = true)]
    json: bool,
    /// Log every ffmpeg command and more.
    #[arg(long, short, global = true)]
    verbose: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check tools, capabilities, fonts, models, Chrome and disk space.
    Doctor,
    /// Manage external tools (ffmpeg, models, chrome).
    Deps {
        #[command(subcommand)]
        cmd: DepsCmd,
    },
    /// Normalized media info as JSON.
    Probe {
        files: Vec<PathBuf>,
        /// Also measure loudness (decodes the audio).
        #[arg(long)]
        loudness: bool,
    },
    /// Schema and semantic validation.
    Validate { project: PathBuf },
    /// Print the project JSON Schema.
    Schema,
    /// Scaffold a project, brand file and folders.
    Init {
        /// Directory to create the project in.
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "shorts")]
        target: String,
        /// Add every media file in this directory as an asset.
        #[arg(long)]
        assets: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Explicit timeline after auto directives + EDL table.
    Expand { project: PathBuf },
    /// Final render, then QA.
    Render {
        project: PathBuf,
        #[arg(long)]
        export: Option<String>,
        #[arg(long)]
        draft: bool,
        /// Print the exact ffmpeg commands and filter scripts without running them.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_cache: bool,
        #[arg(long)]
        allow_qa_errors: bool,
        /// Use a hardware encoder for the final render.
        #[arg(long)]
        hw: bool,
        /// Concurrent segment jobs (default: cores/2).
        #[arg(long, default_value_t = 0)]
        jobs: usize,
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
    /// Still PNGs (`--at`) or a 540p proxy clip (`--range a-b`).
    Preview {
        project: PathBuf,
        #[arg(long, num_args = 1..)]
        at: Vec<String>,
        #[arg(long)]
        range: Option<String>,
        #[arg(long)]
        safe_zones: bool,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Run QA on any video file.
    Qa {
        video: PathBuf,
        #[arg(long, default_value = "shorts")]
        target: String,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Toolchain + pipeline self-test on synthetic media.
    Selftest {
        #[arg(long, conflicts_with = "full")]
        quick: bool,
        #[arg(long)]
        full: bool,
        /// Keep the working directory with all artifacts.
        #[arg(long)]
        keep: bool,
        /// Working directory (default: a temp dir).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum DepsCmd {
    /// Download tools into the tools dir.
    Install {
        /// ffmpeg | whisper-model | face-model | chrome
        what: Option<String>,
        /// Model name for whisper-model (e.g. base.en, large-v3-turbo-q5_0).
        name: Option<String>,
        #[arg(long)]
        all: bool,
        /// Ignore the lockfile pin and install the latest upstream version.
        #[arg(long)]
        latest: bool,
    },
    /// Fetch latest versions, verify with selftest, then promote.
    Update {
        #[arg(long)]
        check: bool,
    },
    /// Switch back to the previous ffmpeg version.
    Rollback,
    /// Installed versions vs the lockfile.
    Status,
}

fn init_tracing(verbose: bool) {
    let filter = std::env::var("CUTLINE_LOG").unwrap_or_else(|_| if verbose { "debug".into() } else { "warn".into() });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let code = if e.use_stderr() { EXIT_USAGE } else { EXIT_OK };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    init_tracing(cli.verbose);
    let json_mode = cli.json;
    let r = std::panic::catch_unwind(|| dispatch(cli))
        .unwrap_or_else(|_| Err(CliError::internal(anyhow::anyhow!("internal panic"))));
    std::process::exit(emit(json_mode, r));
}

fn dispatch(cli: Cli) -> CmdResult {
    match cli.cmd {
        Cmd::Doctor => doctor::run(),
        Cmd::Deps { cmd } => match cmd {
            DepsCmd::Install { what, name, all, latest } => {
                let what = if all { "all".to_string() } else { what.unwrap_or_else(|| "ffmpeg".into()) };
                match what.as_str() {
                    "ffmpeg" => deps_cmd::install_ffmpeg(latest),
                    "whisper-model" => {
                        let n = name.unwrap_or_else(|| "large-v3-turbo-q5_0".into());
                        let s = deps_cmd::whisper_model_spec(&n);
                        deps_cmd::install_model("whisper", &format!("whisper-{n}"), &s.url, s.file)
                    }
                    "face-model" => deps_cmd::install_face_model(),
                    "all" => {
                        let mut res = vec![];
                        let mut code = EXIT_OK;
                        let s = deps_cmd::whisper_model_spec("base.en");
                        let steps: Vec<(&str, Box<dyn Fn() -> CmdResult>)> = vec![
                            ("ffmpeg", Box::new(move || deps_cmd::install_ffmpeg(latest))),
                            (
                                "whisper-model",
                                Box::new(move || deps_cmd::install_model("whisper", "whisper-base.en", &s.url, s.file)),
                            ),
                            ("face-model", Box::new(deps_cmd::install_face_model)),
                        ];
                        let mut human = vec![];
                        for (n, f) in steps {
                            match f() {
                                Ok(o) => {
                                    human.push(o.human);
                                    res.push(json!({"step": n, "ok": true, "result": o.json}));
                                }
                                Err(e) => {
                                    code = EXIT_TOOL;
                                    human.push(format!("{n}: FAILED: {:#}", e.err));
                                    res.push(json!({"step": n, "ok": false, "error": format!("{:#}", e.err)}));
                                }
                            }
                        }
                        Ok(Output::ok(json!({"ok": code == 0, "steps": res}), human.join("\n")).with_code(code))
                    }
                    other => Err(CliError::usage(anyhow::anyhow!("unknown dependency `{other}`"))),
                }
            }
            DepsCmd::Update { check } => deps_cmd::update(check),
            DepsCmd::Rollback => deps_cmd::rollback(),
            DepsCmd::Status => deps_cmd::status(),
        },
        Cmd::Validate { project } => edit::validate(&project),
        Cmd::Schema => edit::schema(),
        Cmd::Init { dir, target, assets, name } => edit::init(&dir, &target, assets.as_deref(), name.as_deref()),
        Cmd::Expand { project } => edit::expand(&project),
        Cmd::Render { project, export, draft, dry_run, no_cache, allow_qa_errors, hw, jobs, out } => edit::render(
            &project,
            edit::RenderArgs { export, draft, dry_run, no_cache, allow_qa_errors, hw, jobs, out },
        ),
        Cmd::Preview { project, at, range, safe_zones, out } => {
            if at.is_empty() && range.is_none() {
                return Err(CliError::usage(anyhow::anyhow!("preview needs --at <t>... or --range a-b")));
            }
            edit::preview(&project, &at, range.as_deref(), safe_zones, out)
        }
        Cmd::Qa { video, target, project } => edit::qa(&video, &target, project.as_deref()),
        Cmd::Selftest { quick: _, full, keep, out } => selftest(full, keep, out),
        Cmd::Probe { files, loudness } => {
            if files.is_empty() {
                return Err(CliError::usage(anyhow::anyhow!("probe needs at least one file")));
            }
            let tc = cutline_deps::toolchain().tool_err()?;
            let mut v = vec![];
            for f in &files {
                let mut i = cutline_media::probe(tc, f, None).map_err(CliError::validation)?;
                if loudness && i.audio.is_some() {
                    i.loudness = cutline_media::probe::measure_loudness(tc, f).ok();
                }
                v.push(i);
            }
            let j = if v.len() == 1 { json!(v[0]) } else { json!(v) };
            let h = serde_json::to_string_pretty(&j).unwrap_or_default();
            Ok(Output::ok(j, h))
        }
    }
}

fn selftest(full: bool, keep: bool, out: Option<PathBuf>) -> CmdResult {
    use cutline_qa::suite::{run_selftest, Mode};
    cutline_deps::toolchain().tool_err()?;
    let (dir, tmp) = match out {
        Some(d) => (d, None),
        None => {
            let t = std::env::temp_dir().join(format!("cutline-selftest-{}", std::process::id()));
            (t.clone(), Some(t))
        }
    };
    std::fs::create_dir_all(&dir).map_err(|e| CliError::internal(anyhow::Error::from(e)))?;
    let rep = run_selftest(if full { Mode::Full } else { Mode::Quick }, dir.clone());
    let report_path = dir.join("selftest_report.json");
    let _ = std::fs::write(&report_path, serde_json::to_vec_pretty(&rep).unwrap_or_default());
    // Always keep a copy of the report in the cwd-independent cache dir for `deps update`.
    let human = cutline_qa::selftest::summary(&rep);
    let mut j = json!(rep);
    if let (Some(t), false) = (tmp, keep) {
        let keep_report = std::env::temp_dir().join("cutline-selftest-last.json");
        let _ = std::fs::copy(&report_path, &keep_report);
        let _ = std::fs::remove_dir_all(t);
        j["report"] = json!(keep_report);
    } else {
        j["report"] = json!(report_path);
    }
    Ok(Output::ok(j, human).with_code(if rep.ok { EXIT_OK } else { EXIT_VALIDATION }))
}
