//! `video-studio eleven …` (spec §14): the commands Claude runs for voice, music, effects and
//! transcripts. Output is short plain text Claude can act on; nothing here prints the API key.

use camino::Utf8PathBuf;
use clap::{Args, Subcommand};
use serde_json::Value;

use video_studio_core::eleven::{self, HelperContext, Pricing, Request};
use video_studio_core::platform::Secrets;
use video_studio_core::Error;

#[derive(Args, Clone)]
pub struct TtsArgs {
    /// Voice ID (default: the brand preset's voice)
    #[arg(long)]
    voice: Option<String>,
    /// File holding the text to voice
    #[arg(long, conflicts_with = "text")]
    text_file: Option<Utf8PathBuf>,
    /// The text to voice (prefer --text-file for anything longer than a line)
    #[arg(long)]
    text: Option<String>,
    /// ElevenLabs model ID
    #[arg(long)]
    model: Option<String>,
    /// Voice settings as JSON, e.g. '{"stability":0.4,"similarity_boost":0.8}'
    #[arg(long)]
    settings: Option<String>,
    /// File holding the passage spoken just before this one (its script), so the voice
    /// carries on from it instead of starting afresh
    #[arg(long)]
    before_file: Option<Utf8PathBuf>,
    /// File holding the passage spoken just after this one
    #[arg(long)]
    after_file: Option<Utf8PathBuf>,
    /// Another take of the same text, voice and settings: 2, 3, … (each number is its own
    /// reading, and is cached like any other request)
    #[arg(long)]
    take: Option<u32>,
}

#[derive(Args, Clone)]
pub struct PromptArgs {
    /// What to generate
    #[arg(long)]
    prompt: String,
    /// Length in seconds
    #[arg(long)]
    length: f64,
}

#[derive(Subcommand)]
pub enum QuoteCommand {
    Tts(TtsArgs),
    Music(PromptArgs),
    Sfx(PromptArgs),
}

#[derive(Subcommand)]
pub enum ElevenCommand {
    /// Text to speech
    Tts {
        #[command(flatten)]
        args: TtsArgs,
        /// Output file, relative to the run's audio/ folder
        #[arg(long)]
        out: String,
    },
    /// Generate music from a brief
    Music {
        #[command(flatten)]
        args: PromptArgs,
        #[arg(long)]
        out: String,
    },
    /// Generate a sound effect
    Sfx {
        #[command(flatten)]
        args: PromptArgs,
        #[arg(long)]
        out: String,
    },
    /// Transcribe audio with word timings (Scribe JSON, which cutline imports)
    Scribe {
        #[arg(long = "in")]
        input: Utf8PathBuf,
        #[arg(long)]
        out: String,
    },
    /// List voices
    Voices {
        #[arg(long)]
        search: Option<String>,
        /// female, male or any (default: the choice in Settings › ElevenLabs)
        #[arg(long)]
        gender: Option<String>,
        /// tried (voices used in earlier videos first) or latest (newest first); default: Settings
        #[arg(long)]
        prefer: Option<String>,
    },
    /// Estimate the credits a call would cost, without generating anything
    Quote {
        #[command(subcommand)]
        command: QuoteCommand,
    },
}

fn tts_request(ctx: &HelperContext, a: &TtsArgs) -> Result<Request, Error> {
    let read = |f: &Utf8PathBuf| -> Result<String, Error> {
        let p = if f.is_absolute() {
            f.clone()
        } else {
            std::env::current_dir().ok().and_then(|d| Utf8PathBuf::from_path_buf(d).ok()).unwrap_or_default().join(f)
        };
        std::fs::read_to_string(&p).map_err(|e| Error::NotFound(format!("couldn't read {f}: {e}")))
    };
    let text = match (&a.text_file, &a.text) {
        (Some(f), _) => read(f)?,
        (None, Some(t)) => t.clone(),
        (None, None) => return Err(Error::other("give --text-file <file> or --text \"…\"")),
    };
    let neighbour = |f: &Option<Utf8PathBuf>| -> Result<Option<String>, Error> {
        Ok(f.as_ref().map(read).transpose()?.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
    };
    let settings = match &a.settings {
        Some(s) => Some(
            serde_json::from_str::<Value>(s).map_err(|e| Error::other(format!("--settings isn't valid JSON: {e}")))?,
        ),
        None => ctx.default_voice_settings.clone(),
    };
    Ok(Request::Tts {
        voice: a.voice.clone().or_else(|| ctx.default_voice.clone()).unwrap_or_default(),
        text: text.trim().to_string(),
        model: a.model.clone().or_else(|| ctx.default_model.clone()),
        settings,
        before: neighbour(&a.before_file)?,
        after: neighbour(&a.after_file)?,
        take: a.take,
    })
}

fn check_length(length: f64, max: f64, what: &str) -> Result<(), Error> {
    if !(length > 0.0 && length <= max) {
        return Err(Error::other(format!("--length for {what} must be between 0 and {max} seconds")));
    }
    Ok(())
}

fn generate(ctx: &HelperContext, req: Request, out: &str, secrets: &dyn Secrets) -> Result<(), Error> {
    let r = eleven::execute(ctx, &req, out, secrets)?;
    // Relative to the run folder, with forward slashes, as Claude writes paths in its commands.
    let shown = video_studio_core::paths::strip_path_prefix(
        &video_studio_core::paths::normalize(&r.out),
        &video_studio_core::paths::normalize(&ctx.run_folder),
    )
    .map(|p| p.as_str().replace('\\', "/"))
    .unwrap_or_else(|| r.out.to_string());
    if r.cached {
        println!("{shown} (from cache, 0 credits; {} of {} used in this run)", r.spent, r.cap);
    } else {
        println!("{shown} ({} credits; {} of {} used in this run)", r.credits, r.spent, r.cap);
    }
    Ok(())
}

pub fn run(command: ElevenCommand, secrets: &dyn Secrets) -> i32 {
    let ctx = match HelperContext::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 126;
        }
    };
    let result: Result<(), Error> = (|| match command {
        ElevenCommand::Tts { args, out } => generate(&ctx, tts_request(&ctx, &args)?, &out, secrets),
        ElevenCommand::Music { args, out } => {
            check_length(args.length, 600.0, "music")?;
            generate(&ctx, Request::Music { prompt: args.prompt, length_s: args.length }, &out, secrets)
        }
        ElevenCommand::Sfx { args, out } => {
            check_length(args.length, 30.0, "a sound effect")?;
            generate(&ctx, Request::Sfx { prompt: args.prompt, length_s: args.length }, &out, secrets)
        }
        ElevenCommand::Scribe { input, out } => {
            let p = if input.is_absolute() {
                input
            } else {
                std::env::current_dir()
                    .ok()
                    .and_then(|d| Utf8PathBuf::from_path_buf(d).ok())
                    .unwrap_or_default()
                    .join(input)
            };
            generate(&ctx, Request::Scribe { input: p }, &out, secrets)
        }
        ElevenCommand::Voices { search, gender, prefer } => {
            let gender = match gender.as_deref() {
                Some(g) => eleven::VoiceGender::parse(g)
                    .ok_or_else(|| Error::other(format!("--gender is female, male or any; got `{g}`")))?,
                None => ctx.voice_gender,
            };
            let prefer = match prefer.as_deref() {
                Some(p) => eleven::VoicePreference::parse(p)
                    .ok_or_else(|| Error::other(format!("--prefer is tried or latest; got `{p}`")))?,
                None => ctx.voice_preference,
            };
            let key = match &ctx.test_api_key {
                Some(k) => k.clone(),
                None => secrets
                    .get(video_studio_core::platform::ELEVEN_KEY)?
                    .filter(|k| !k.is_empty())
                    .ok_or_else(|| Error::Environment("No ElevenLabs API key is set.".into()))?,
            };
            let all = eleven::Client::new(&key, ctx.base_url.as_deref()).voices(search.as_deref())?;
            let used = eleven::voices::used(&ctx.cache_dir);
            let found = all.len();
            let voices = eleven::voices::rank(all, prefer, gender, &used);
            if voices.is_empty() {
                match gender.word() {
                    Some(g) if found > 0 => {
                        println!("No {g} voices match ({found} of another gender do: try --gender any).")
                    }
                    _ => println!("No voices match."),
                }
            } else {
                println!(
                    "# {}; {}. Best first.",
                    gender.word().map(|g| format!("{g} voices")).unwrap_or_else(|| "voices of either gender".into()),
                    prefer.describe()
                );
            }
            for v in voices {
                let gender = eleven::voices::gender_of(&v);
                println!(
                    "{}  {}{}{}{}{}",
                    v.voice_id,
                    v.name,
                    gender.map(|g| format!("  ({g})")).unwrap_or_else(|| "  (gender not given)".into()),
                    v.category.map(|c| format!("  [{c}]")).unwrap_or_default(),
                    used.get(&v.voice_id).map(|u| format!("  used {}×", u.count)).unwrap_or_default(),
                    v.description.map(|d| format!("  — {}", d.replace('\n', " "))).unwrap_or_default()
                );
            }
            Ok(())
        }
        ElevenCommand::Quote { command } => {
            let req = match command {
                QuoteCommand::Tts(a) => tts_request(&ctx, &a)?,
                QuoteCommand::Music(a) => Request::Music { prompt: a.prompt, length_s: a.length },
                QuoteCommand::Sfx(a) => Request::Sfx { prompt: a.prompt, length_s: a.length },
            };
            let credits = eleven::quote(&req, &Pricing::bundled(), None);
            let spent = eleven::spent(&ctx.log_path());
            println!(
                "about {credits} credits ({} of {} used in this run, {} left)",
                spent,
                ctx.credit_cap,
                ctx.credit_cap.saturating_sub(spent)
            );
            Ok(())
        }
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            match e {
                Error::Budget(_) => 3,
                Error::Denied(_) => 126,
                _ => 1,
            }
        }
    }
}
