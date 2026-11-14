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
    /// Work on this variant of the project (default: the first by name; `render` does all).
    #[arg(long, global = true)]
    variant: Option<String>,
    /// Work on this locale of the project (default: the base language).
    #[arg(long, global = true)]
    locale: Option<String>,
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
    /// Word-level transcript JSON (cached). Uses whisper.cpp, or imports an ElevenLabs Scribe JSON.
    Transcribe {
        file: PathBuf,
        /// whisper model name (default large-v3-turbo-q5_0, or base.en when only that is installed).
        #[arg(long)]
        model: Option<String>,
        /// Use the fast base.en model.
        #[arg(long)]
        fast: bool,
        #[arg(long)]
        lang: Option<String>,
        /// Import an ElevenLabs Scribe JSON instead of running whisper.
        #[arg(long)]
        import_scribe: Option<PathBuf>,
        /// Write the transcript JSON here (default: stdout only).
        #[arg(long, short)]
        out: Option<PathBuf>,
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
        /// Default: shorts (long_1080p with --style business).
        #[arg(long)]
        target: Option<String>,
        /// creator (Shorts-style defaults) or business (product demos: screencast profile,
        /// sentence captions, business brand).
        #[arg(long, default_value = "creator")]
        style: String,
        /// Add every media file in this directory as an asset.
        #[arg(long)]
        assets: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Explicit timeline after auto directives + EDL table.
    Expand { project: PathBuf },
    /// Where each cue landed: phrase, matched transcript words and timeline time.
    Cues { project: PathBuf },
    /// Tempo and beat times of an audio asset in the project.
    Beats {
        project: PathBuf,
        /// Asset id (default: the music-role audio layer's asset).
        #[arg(long)]
        asset: Option<String>,
    },
    /// Record the project's VHS `.tape` assets again (fresh clocks, process lists, …).
    Record {
        project: PathBuf,
        /// Only this asset id.
        #[arg(long)]
        asset: Option<String>,
    },
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
    /// Text/graphics/caption templates, transitions, effects and other names.
    Templates {
        #[command(subcommand)]
        cmd: TemplatesCmd,
    },
    /// Review pack: contact sheet, cuts, captions, reframe, audio.png, timeline, QA, summary.
    Review {
        project: PathBuf,
        #[arg(long, default_value = "review")]
        out: PathBuf,
        /// Text recognition for the demo checks: auto, vision (macOS), tesseract or off.
        /// Default: CUTLINE_OCR, else the project's `qa.ocr`, else auto.
        #[arg(long)]
        ocr: Option<String>,
    },
    /// Thumbnail (1280×720, or 1080×1920 for Shorts) with template text.
    Thumbnail {
        project: PathBuf,
        #[arg(long)]
        at: String,
        #[arg(long)]
        text: Option<String>,
        #[arg(long, default_value = "hook_title")]
        template: String,
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
    /// YouTube chapter list (from markers, clip `chapter` fields or clip ids).
    Chapters { project: PathBuf },
    /// Cache statistics / cleanup.
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
    /// Print the Claude workflow guide (CLAUDE_GUIDE.md).
    Guide,
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
enum CacheCmd {
    /// Sizes of the global cache and (optionally) a project's .cutline cache.
    Stats {
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Delete cache files (all, or older than e.g. 30d / 12h).
    Clean {
        #[arg(long)]
        older_than: Option<String>,
        #[arg(long)]
        project: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum TemplatesCmd {
    /// List everything with parameters.
    List,
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
    cutline_render::select::set_current(cutline_render::select::Selection {
        variant: cli.variant.clone(),
        locale: cli.locale.clone(),
    });
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
                    "chrome" => deps_cmd::install_chrome(),
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
                            ("chrome", Box::new(deps_cmd::install_chrome)),
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
        Cmd::Transcribe { file, model, fast, lang, import_scribe, out } => {
            transcribe(&file, model, fast, lang, import_scribe, out)
        }
        Cmd::Validate { project } => edit::validate(&project),
        Cmd::Schema => edit::schema(),
        Cmd::Init { dir, target, style, assets, name } => {
            edit::init(&dir, target.as_deref(), &style, assets.as_deref(), name.as_deref())
        }
        Cmd::Expand { project } => edit::expand(&project),
        Cmd::Cues { project } => edit::cues(&project),
        Cmd::Beats { project, asset } => edit::beats(&project, asset.as_deref()),
        Cmd::Record { project, asset } => edit::record(&project, asset.as_deref()),
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
        Cmd::Templates { cmd: TemplatesCmd::List } => templates_list(),
        Cmd::Review { project, out, ocr } => edit::review(&project, &out, ocr.as_deref()),
        Cmd::Thumbnail { project, at, text, template, out } => {
            edit::thumbnail(&project, &at, text.as_deref(), &template, out)
        }
        Cmd::Chapters { project } => edit::chapters(&project),
        Cmd::Cache { cmd } => match cmd {
            CacheCmd::Stats { project } => cache_cmd(None, project, false),
            CacheCmd::Clean { older_than, project } => cache_cmd(older_than, project, true),
        },
        Cmd::Guide => Ok(Output::ok(json!({"guide": GUIDE}), GUIDE)),
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

fn templates_list() -> CmdResult {
    use cutline_core::catalog as c;
    let tpl = cutline_text::templates::list();
    let effects = [
        ("punch_in", "at, scale=1.15, duration=0.25, ease, hold?, focus?{x,y}"),
        ("zoom", "from=1, to, start=0, end?, ease, focus?"),
        ("ken_burns", "from_scale=1, to_scale=1.15, from?{x,y}, to?{x,y}, ease"),
        ("shake", "at, duration=0.3, intensity=0.02"),
        ("flash", "at, duration=0.15, color?"),
        ("blur_in", "duration=0.4"),
        ("glitch", "at, duration=0.15, amount=12"),
        ("vignette", "strength=0.4"),
        ("motion_blur", "at, duration=0.15"),
        ("freeze", "at, duration"),
    ];
    let eases = [
        "linear",
        "in_quad",
        "out_quad",
        "in_out_quad",
        "in_cubic",
        "out_cubic",
        "in_out_cubic",
        "in_expo",
        "out_expo",
        "in_out_expo",
        "back",
        "spring",
    ];
    let anims = ["fade", "slide_left", "slide_right", "slide_up", "slide_down", "pop", "scale_in"];
    let emoji = cutline_text::templates::emoji_names();
    let fonts = cutline_render::pipeline::bundled_fonts();
    let j = json!({
        "text_templates": tpl,
        "transitions": {"xfade": c::XFADE_TRANSITIONS, "custom": c::CUSTOM_TRANSITIONS},
        "effects": effects.iter().map(|(n, p)| json!({"name": n, "params": p})).collect::<Vec<_>>(),
        "easings": eases,
        "layer_animations": anims,
        "caption_styles": c::CAPTION_STYLES,
        "voice_chains": c::VOICE_CHAINS,
        "color_presets": c::COLOR_PRESETS,
        "luts": c::BUNDLED_LUTS,
        "sfx_aliases": c::SFX_ALIASES,
        "emoji": emoji,
        "fonts": fonts,
        "position_presets": ["center", "top", "bottom", "left", "right", "top_left", "top_right", "bottom_left", "bottom_right", "upper_mid", "lower_mid"],
    });
    let mut h = String::from("Text templates (layer type \"text\"):\n");
    for t in &tpl {
        h.push_str(&format!("  {:<14} {}\n                 params: {}\n", t.name, t.description, t.params.join(", ")));
    }
    h.push_str(&format!(
        "\nTransitions: {}\nCustom transitions: {}\n",
        c::XFADE_TRANSITIONS.join(", "),
        c::CUSTOM_TRANSITIONS.join(", ")
    ));
    h.push_str("\nEffects:\n");
    for (n, p) in effects {
        h.push_str(&format!("  {n:<12} {p}\n"));
    }
    h.push_str(&format!("\nEasings: {}\nLayer enter/exit: {}\nCaption styles: {}\nVoice chains: {}\nColor presets: {}\nLUTs: {}\nEmoji: {}\nFonts: {}\n",
        eases.join(", "), anims.join(", "), c::CAPTION_STYLES.join(", "), c::VOICE_CHAINS.join(", "), c::COLOR_PRESETS.join(", "), c::BUNDLED_LUTS.join(", "), emoji.join(", "), fonts.join(", ")));
    Ok(Output::ok(j, h))
}

fn transcribe(
    file: &std::path::Path,
    model: Option<String>,
    fast: bool,
    lang: Option<String>,
    scribe: Option<PathBuf>,
    out: Option<PathBuf>,
) -> CmdResult {
    use cutline_transcribe::whisper;
    let tc = cutline_deps::toolchain().tool_err()?;
    let fp = cutline_media::fingerprint(file).map_err(CliError::usage)?;
    let cache = cutline_media::cache::Cache::global("transcripts");
    let model =
        model.unwrap_or_else(|| if fast { whisper::FAST_MODEL.into() } else { cutline_render::auto::whisper_model() });
    let asset = file.file_stem().and_then(|s| s.to_str()).unwrap_or("asset").to_string();
    let (t, source) = match &scribe {
        Some(sf) => {
            let text = std::fs::read_to_string(sf).map_err(CliError::usage)?;
            let t = cutline_transcribe::scribe::import(&text, &asset).map_err(CliError::validation)?;
            // cache under the default model key so renders pick it up without the file
            cutline_transcribe::put_cache(&cache, &fp, &model, lang.as_deref(), &t).map_err(CliError::internal)?;
            (t, "scribe".to_string())
        }
        None => {
            let work = std::env::temp_dir().join(format!("cutline-whisper-{}", std::process::id()));
            match cutline_transcribe::get(tc, &asset, file, &fp, None, &model, lang.as_deref(), &cache, true).map_err(CliError::tool)? {
                Some((t, s)) => {
                    let _ = std::fs::remove_dir_all(work);
                    (t, format!("{s:?}"))
                }
                None => {
                    return Err(CliError::tool(anyhow::anyhow!(
                        "no transcriber available: {}. Import a Scribe JSON with --import-scribe, or install whisper.cpp (whisper-cli) and `cutline deps install whisper-model {model}`",
                        whisper::available(&model).err().map(|e| format!("{e:#}")).unwrap_or_default()
                    )))
                }
            }
        }
    };
    if let Some(o) = &out {
        std::fs::write(o, serde_json::to_vec_pretty(&t).unwrap_or_default()).map_err(CliError::internal)?;
    }
    let human = format!(
        "{} words, {} segments ({source})\n{}",
        t.words.len(),
        t.segments.len(),
        t.segments
            .iter()
            .map(|s| format!("[{:>7.2}–{:>7.2}] {}", s.start, s.end, s.text))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let mut j = json!(t);
    j["source"] = json!(source);
    Ok(Output::ok(j, human))
}

const GUIDE: &str = include_str!("../../../docs/CLAUDE_GUIDE.md");

fn cache_cmd(older: Option<String>, project: Option<PathBuf>, clean: bool) -> CmdResult {
    use cutline_media::cache::{clean as do_clean, parse_age, stats};
    let mut roots = vec![cutline_deps::paths::cache_dir()];
    if let Some(p) = project {
        let dir = if p.is_dir() { p } else { p.parent().map(|d| d.to_path_buf()).unwrap_or_default() };
        roots.push(cutline_render::cutline_dir(&dir).join("cache"));
        roots.push(cutline_render::cutline_dir(&dir).join("tmp"));
    }
    let age = match &older {
        Some(a) => Some(
            parse_age(a)
                .ok_or_else(|| CliError::usage(anyhow::anyhow!("bad --older-than `{a}` (use e.g. 30d, 12h, 45m)")))?,
        ),
        None => None,
    };
    let res: Vec<_> = roots.iter().map(|r| if clean { do_clean(r, age) } else { stats(r) }).collect();
    let h = res
        .iter()
        .map(|s| {
            format!(
                "{} {}: {} files, {:.1} MB",
                if clean { "removed from" } else { "cache" },
                s.root,
                s.files,
                s.bytes as f64 / 1e6
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Output::ok(json!({"cleaned": clean, "roots": res}), h))
}
