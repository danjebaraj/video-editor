//! What Claude is told in a run (spec §12.2 step 2): the cutline workflow, the manifest, brand
//! rules, intent, outputs, locales and budget. The same production prompt is used for the dry
//! run, with a different finish.

use camino::Utf8Path;
use serde_json::Value;

use super::prepare::{ManifestEntry, Revise};
use crate::media::{format_duration, MediaStatus};
use crate::presets::{Platforms, ResolvedOutput};
use crate::project::{Project, Quality, RunConfig, RunKind, RunRecord};

pub struct PromptInput<'a> {
    pub project: &'a Project,
    pub run_config: &'a RunConfig,
    pub kind: RunKind,
    pub folder: &'a Utf8Path,
    pub outputs: &'a [ResolvedOutput],
    pub exports: &'a Value,
    pub manifest: &'a [ManifestEntry],
    pub cutline_guide: &'a str,
    /// Whether the cutline has `cutline still` (BUG-0002); older builds can't convert HEIC/HEIF.
    pub cutline_still: bool,
    pub eleven_available: bool,
    /// Who speaks the voice-over (Settings › Voice), and whether Cartesia's key is stored.
    pub voice_provider: crate::eleven::VoiceProvider,
    pub cartesia_available: bool,
    pub default_voice: Option<&'a str>,
    /// How to choose a voice when none is fixed (Settings › ElevenLabs), in words.
    pub voice_choosing: Option<&'a str>,
    pub platforms: &'a Platforms,
    /// How the video is finished: the project's own choices or the ones in Settings.
    pub finish: &'a crate::finish::Finish,
}

/// One line per media item: ID, kind, role, duration, size, status, notes.
pub fn manifest_summary(manifest: &[ManifestEntry]) -> String {
    if manifest.is_empty() {
        return "(no media attached)".into();
    }
    let mut s = String::new();
    for m in manifest {
        let mut bits: Vec<String> = vec![m.kind.clone()];
        if let Some(r) = &m.role {
            bits.push(r.replace('_', " "));
        }
        if let Some(p) = &m.probe {
            if let Some(d) = p.duration_s {
                bits.push(format_duration(d));
            }
            if let (Some(w), Some(h)) = (p.width, p.height) {
                bits.push(format!("{w}×{h}"));
            }
            if let Some(f) = &p.fps {
                bits.push(format!("{f} fps"));
            }
        }
        match m.status {
            MediaStatus::Missing => bits.push("MISSING".into()),
            MediaStatus::Changed => bits.push("changed since added".into()),
            _ => {}
        }
        let name = m.path.as_deref().and_then(|p| p.rsplit(['/', '\\']).next()).unwrap_or("");
        s.push_str(&format!(
            "- `{}` — {}{}",
            m.id,
            bits.join(", "),
            if name.is_empty() { String::new() } else { format!(" — {name}") }
        ));
        if let Some(n) = m.notes.as_deref().filter(|n| !n.trim().is_empty()) {
            s.push_str(&format!(" — note: {}", n.trim().replace('\n', " ")));
        }
        if !m.tags.is_empty() {
            s.push_str(&format!(" — tags: {}", m.tags.join(", ")));
        }
        if m.viewable_path.is_some() {
            s.push_str(&format!(" — view: {}/{}.jpg", crate::viewable::PREVIEWS_DIR, m.id));
        } else if let Some(e) = &m.viewable_error {
            s.push_str(&format!(" — can't be viewed: {}", e.trim().replace('\n', " ")));
        }
        s.push('\n');
    }
    s
}

/// A short description of the project for the spec assistant and for runs (spec §4.3).
pub fn project_summary(project: &Project, outputs: &[ResolvedOutput], manifest: &[ManifestEntry]) -> String {
    let i = &project.intent;
    let mut s = format!("Project: {}\nIntent: {}\n", project.name, i.describe());
    if let Some(a) = i.audience.as_deref().filter(|a| !a.trim().is_empty()) {
        s.push_str(&format!("Audience: {a}\n"));
    }
    if let Some(t) = i.tone.as_deref().filter(|a| !a.trim().is_empty()) {
        s.push_str(&format!("Tone: {t}\n"));
    }
    if let Some(l) = i.target_length_s {
        s.push_str(&format!("Target length: {}\n", format_duration(l)));
    }
    if let Some(c) = i.cta.as_deref().filter(|a| !a.trim().is_empty()) {
        s.push_str(&format!("Primary call to action: {c}\n"));
    }
    if !i.locales.is_empty() {
        s.push_str(&format!("Languages: {}\n", i.locales.join(", ")));
    }
    if let Some(b) = &project.brand {
        s.push_str(&format!("Brand preset: {}\n", b.snapshot.name));
        s.push_str(&instructions_text(b));
    }
    if !outputs.is_empty() {
        s.push_str("Outputs:\n");
        for o in outputs {
            let cap = o.limits.max_duration_s.map(|d| format!(", at most {}", format_duration(d))).unwrap_or_default();
            let captions = match o.captions.as_str() {
                "burn" => "captions burned in",
                "sidecar" => "captions as a separate file, not burned in",
                "none" => "no captions",
                other => other,
            };
            s.push_str(&format!(
                "- {} ({}, {}×{}, {}, {captions}{cap})\n",
                o.name,
                o.platform,
                o.width,
                o.height,
                o.aspect_label()
            ));
        }
    }
    let cap = &project.app_capture;
    s.push_str(&if !cap.enabled {
        "App recording: off. Production runs can't drive or record apps; every shot must come from the attached media.\n".to_string()
    } else if cap.apps.is_empty() {
        "App recording: on. During production, Claude Code can open desktop apps and web apps on this computer, operate them and record the screen.\n".to_string()
    } else {
        format!(
            "App recording: on, for these apps only: {}. During production, Claude Code can open them, operate them and record the screen.\n",
            cap.apps.join(", ")
        )
    });
    s.push_str("Media (refer to items by ID, as `media:<id>`):\n");
    s.push_str(&manifest_summary(manifest));
    s
}

/// The brand's instruction sets this project chose, verbatim, under their labels.
fn instructions_text(b: &crate::brand::BrandRef) -> String {
    let chosen: Vec<_> = b.chosen_instructions().into_iter().filter(|i| !i.text.trim().is_empty()).collect();
    if chosen.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "Brand instructions chosen for this video, verbatim from the brand owner. They give the product context and the voice every video of it shares: follow them in what the video says and how it says it.\n",
    );
    for set in chosen {
        s.push_str(&format!("**{}**\n", set.label.trim()));
        for line in set.text.trim().lines() {
            s.push_str(&format!("> {line}\n"));
        }
    }
    s
}

/// What the spec assistant needs to know about the voice-over: when the voice performs delivery
/// tags, the `vo` blocks carry them.
pub fn assistant_voice_notes(provider: crate::eleven::VoiceProvider, brand_model: Option<&str>) -> String {
    let model = brand_model.map(str::trim).filter(|m| !m.is_empty()).unwrap_or(crate::eleven::tags::DEFAULT_MODEL);
    if provider != crate::eleven::VoiceProvider::Elevenlabs || !crate::eleven::tags::eleven_performs_tags(model) {
        return String::new();
    }
    format!(
        "Voice-over: ElevenLabs, model `{model}`, which performs delivery tags. Write them into the `vo` blocks so the read sounds right for a business audience, and keep them when you edit a block. They don't count as words for length, and captions leave them out.\n{}",
        crate::eleven::tags::ELEVEN_TAG_GUIDE
    )
}

fn brand_section(project: &Project) -> String {
    let Some(b) = &project.brand else {
        return "No brand preset is selected. Choose restrained, neutral colors and one clean sans-serif typeface.\n"
            .into();
    };
    let b = &b.snapshot;
    let cutline = b.for_cutline();
    let mut s = format!(
        "Brand preset: {}. The app has already written it as a cutline brand file, brand/default.json, which cutline loads on its own: leave `brand` out of project.json5 and keep that file's fonts, colors, logo and watermark as they are. brand-preset.json is the app's own copy with everything the brand owner entered; it is not a cutline file, so never point `brand` at it.\n",
        b.name
    );
    let c = &b.colors;
    s.push_str(&format!(
        "- Colors: primary {}, secondary {}, accent {}, background {}, text {}.\n",
        c.primary, c.secondary, c.accent, c.background, c.text
    ));
    let font = |f: &crate::brand::FontRef| f.family.clone().or_else(|| f.file.as_ref().map(|a| a.path.clone()));
    if let Some(h) = font(&b.fonts.heading) {
        s.push_str(&format!("- Heading font: {h}\n"));
    }
    if let Some(h) = font(&b.fonts.body) {
        s.push_str(&format!("- Body font: {h}\n"));
    }
    for (label, a) in b.assets() {
        if !label.ends_with("font") {
            s.push_str(&format!("- {label}: {}\n", a.path));
        }
    }
    if cutline.watermark {
        s.push_str(&format!(
            "- Watermark: the logo, {}, opacity {}. brand/default.json already says so; cutline draws it over the edit.\n",
            b.watermark.position.replace('_', " "),
            b.watermark.opacity
        ));
    } else {
        s.push_str("- No watermark: set `watermark: false` in project.json5.\n");
    }
    for note in &cutline.notes {
        s.push_str(&format!("- {note} Say so once under assumptions (dry run) or Assumptions (summary.md).\n"));
    }
    for (label, v) in [
        ("Lower-third style", &b.lower_third_style),
        ("Caption style", &b.caption_style),
        ("Music mood", &b.music_mood),
    ] {
        if !v.trim().is_empty() {
            s.push_str(&format!("- {label}: {}\n", v.trim()));
        }
    }
    if !b.lower_third_style.trim().is_empty() || !b.caption_style.trim().is_empty() {
        s.push_str("  The caption and lower-third styles are the brand owner's own words, not cutline names: pick the closest cutline caption style, position and template. That is expected, so don't report the choice as a risk.\n");
    }
    s.push_str(&rules_text(b));
    s
}

/// The brand's rules, verbatim. Every project that uses the brand gets them.
fn rules_text(b: &crate::brand::BrandPreset) -> String {
    if b.rules.trim().is_empty() {
        return String::new();
    }
    let mut s = String::from("Brand rules, verbatim from the brand owner. Follow them exactly:\n");
    for line in b.rules.trim().lines() {
        s.push_str(&format!("> {line}\n"));
    }
    s
}

/// What the spec assistant is told about the brand beyond the project summary: its rules, so
/// the script is written to them from the start. (A run gets them in its Brand section.)
pub fn assistant_brand_notes(project: &Project) -> String {
    project.brand.as_ref().map(|b| rules_text(&b.snapshot)).unwrap_or_default()
}

fn audio_section(input: &PromptInput) -> String {
    let rc = input.run_config;
    if input.kind == RunKind::DryRun {
        let voice = match input.default_voice {
            Some(v) => format!("The voice for this project is `{v}`; plan with it unless the spec names another."),
            None => {
                "No voice is fixed for this project: the production run will choose one itself, following the voice preferences in Settings › ElevenLabs. Report that once under ambiguities, and say where a voice is fixed: in the brand preset (App Library › Brands) or as the default voice in Settings › ElevenLabs. Don't pick one.".to_string()
            }
        };
        return format!("Audio: this is a dry run, so no audio is generated. Where the edit needs voice-over, music or sound effects, plan each file under audio/ and estimate credits with `video-studio eleven quote …`. Hold each file's place with a stand-in:\n\
         - Voice-over: write the text to audio/<name>.txt (the `vo` block word for word, with any delivery tags in square brackets it has), then `video-studio standin audio --out audio/<name>.mp3 --text-file audio/<name>.txt`. It writes a quiet sound as long as the words take to speak and a stand-in transcript, audio/<name>.json. Give the asset `role: \"voice\"`, `script: \"audio/<name>.txt\"` and `transcript: \"audio/<name>.json\"`, exactly as production will, and time the edit with cues where you would in production. Captions and cue timing can then be checked; real narration will differ by a second or so per passage, which is expected and not a risk.\n\
         - Music and sound effects: `video-studio standin audio --out audio/<name>.mp3 --seconds <n>`.\n\
         {voice}\n");
    }
    let cartesia = input.voice_provider == crate::eleven::VoiceProvider::Cartesia;
    if !input.eleven_available && !(cartesia && input.cartesia_available) {
        return "Audio: no ElevenLabs key is configured, so nothing can be generated. Use only audio files from the media manifest; if the spec needs narration or music that isn't attached, say so in summary.md under Known issues.\n".into();
    }
    let voice = match input.default_voice {
        Some(v) => format!("The brand's default voice is `{v}`: use it unless the spec names another."),
        None => match input.voice_choosing {
            Some(how) => format!("No voice is fixed for this project, so you choose one. {how}"),
            None => "No default voice is set: pick one with `video-studio eleven voices --search <words>` that fits the tone, and say which in summary.md.".to_string(),
        },
    };
    let tagged = !cartesia && crate::eleven::tags::eleven_performs_tags(&eleven_model(input));
    let neighbours = if cartesia || tagged {
        "- Every passage is voiced on its own: `--before-file` and `--after-file` have no effect with this voice model, so give every passage complete sentences and keep its delivery in the same register as the others.\n"
    } else {
        "- Give each passage its neighbours: `--before-file` is the text file of the passage spoken just before it, `--after-file` the one just after (the first passage has only an after, the last only a before). The voice then carries its pace and tone across them. They cost nothing extra.\n"
    };
    format!(
        "Audio: generate voice-over, music and sound effects with the app's helper. It is the only way to reach ElevenLabs.\n\
         - `video-studio eleven tts --voice <id> --text-file <file> --out <name>.mp3 [--before-file <file>] [--after-file <file>] [--take <n>] [--model <id>] [--settings '<json>']`\n\
         - `video-studio eleven music --prompt \"<brief>\" --length <seconds> --out <name>.mp3`\n\
         - `video-studio eleven sfx --prompt \"<brief>\" --length <seconds> --out <name>.mp3`\n\
         - `video-studio eleven scribe --in <audio> --out <name>.json` (word timings; import with `cutline transcribe`)\n\
         - `video-studio eleven voices [--search <words>] [--gender female|male|any] [--prefer tried|latest]` (the account's voices, best first for the preferences in Settings) and `video-studio eleven quote <tts|music|sfx> …` (estimate, costs nothing)\n\
         Outputs land in audio/. Write voice-over text to a file under audio/ or work/ first, one file per scene or passage, and voice the words of the spec's `vo` blocks exactly as written. Delivery tags (`[warmly]`, `<break time=\"1s\"/>`) are not words: cutline leaves them out of captions and the script check, so the file with tags is also the asset's `script`.\n\
         The passages must sound like one reading, not several:\n\
         - Use the same voice, model and `--settings` for every passage of a video.\n\
         {neighbours}\
         - When a take is wrong (a long pause, a rushed or dragging read, a mispronounced word), ask for another with `--take 2`, then `--take 3`: same text, same settings, a new reading. Do not change the settings for one passage; if the settings are wrong, change them for all and voice every passage again.\n\
         - After transcribing, compare the passages: `cutline validate` reports one spoken much faster or slower than the rest (`W_VOICE_PACE`). The render brings every voice file to the same loudness, so do not correct their levels with `gain_db`.\n\
         {voice}\n\
         {delivery}\
         {provider}\
         Budget: this run may spend at most {} ElevenLabs credits. Every call is quoted first; a call that would pass the cap fails with a message telling you how much is left. Identical requests are served from a cache at no cost, so never change text just to regenerate.\n",
        rc.eleven_credit_cap,
        delivery = if cartesia { String::new() } else { eleven_delivery(input) },
        provider = cartesia_note(input)
    )
}

/// The ElevenLabs model the run's voice-over uses: the brand's, or the helper's default.
fn eleven_model(input: &PromptInput) -> String {
    input
        .project
        .brand
        .as_ref()
        .and_then(|b| b.snapshot.voice.model.clone())
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| crate::eleven::tags::DEFAULT_MODEL.to_string())
}

/// How ElevenLabs voice-over text carries delivery tags, for the model in force.
fn eleven_delivery(input: &PromptInput) -> String {
    let model = eleven_model(input);
    let from_brand = model != crate::eleven::tags::DEFAULT_MODEL;
    let whose = if from_brand { "the brand's voice model" } else { "the default voice model" };
    if !crate::eleven::tags::eleven_performs_tags(&model) {
        return format!(
            "The voice model is `{model}` ({whose}). It reads delivery tags in square brackets aloud, so the helper removes them before the text goes out: leave the spec's tags in the text files, but add none. The brand preset can name `{}` or `eleven_v3` instead to have them performed.\n",
            crate::eleven::tags::DEFAULT_MODEL
        );
    }
    format!(
        "The voice model is `{model}` ({whose}); pass no other `--model` unless the spec names one. It performs delivery tags, and every passage should use them.\n{}\
         The spec's `vo` blocks may already carry tags: keep them as they are, and add tags following this guide where a passage has none or needs another. Never add, drop or change words to fit a tag.\n",
        crate::eleven::tags::ELEVEN_TAG_GUIDE
    )
}

/// What changes when Cartesia speaks the voice-over instead of ElevenLabs.
fn cartesia_note(input: &PromptInput) -> String {
    use crate::finish::Energy;
    if input.voice_provider != crate::eleven::VoiceProvider::Cartesia {
        return String::new();
    }
    if !input.cartesia_available {
        return "The voice-over of this run is to be spoken by Cartesia (Settings › ElevenLabs › Voice-over by), but no Cartesia key is stored, so `video-studio eleven tts` will fail. Do not generate the voice another way: use voice-over files from the media manifest if there are any, and otherwise say in summary.md under Known issues that the Cartesia key is missing.\n".into();
    }
    let settings = match input.finish.energy {
        Energy::Upbeat => {
            "For this video's upbeat read pass `--settings '{\"speed\": 1.03, \"emotion\": \"excited\"}'`"
        }
        Energy::Balanced => "For this video's even read pass no `--settings`",
        Energy::Calm => "For this video's calm read pass `--settings '{\"speed\": 0.97, \"emotion\": \"calm\"}'`",
    };
    let mut s = format!(
        "The voice-over of this run is spoken by Cartesia, not ElevenLabs (Settings › ElevenLabs › Voice-over by). The commands stay the same: in this run `video-studio eleven tts` and `video-studio eleven voices` reach Cartesia, and voice IDs are Cartesia's. What differs:\n\
         - `--settings` takes Cartesia's own: `{{\"speed\": 0.6 to 1.5, \"emotion\": \"<one word, e.g. calm, content, excited>\", \"volume\": 0.5 to 2}}`. {settings}, in place of any ElevenLabs settings (stability, similarity_boost, style) the Finish section gives; the brand's are ElevenLabs' too and only their speed is used.\n\
         - `--model` is a Cartesia model (default `{model}`).\n\
         {tags}\
         - `--before-file` and `--after-file` have no effect, so give every passage complete sentences. `--take <n>` still gives another reading.\n\
         - A character costs one credit, counted against the same cap.\n\
         - Music, sound effects and transcripts (`eleven music`, `sfx`, `scribe`) are still ElevenLabs.\n",
        model = crate::eleven::cartesia::DEFAULT_MODEL,
        tags = crate::eleven::tags::CARTESIA_TAG_GUIDE
    );
    if !input.eleven_available {
        s.push_str("No ElevenLabs key is configured, so `eleven music`, `eleven sfx` and `eleven scribe` will fail: take word timings from `cutline transcribe` instead, use music and sound effects only from the media manifest, and say in summary.md what the video lacks for it.\n");
    }
    s
}

/// How a production run makes footage of an app that nobody recorded beforehand.
fn capture_section(input: &PromptInput) -> String {
    let cap = &input.project.app_capture;
    if !cap.enabled {
        return "App recording is turned off for this project: you cannot drive or record apps. If the spec asks for footage of an app that isn't in the media manifest, do not fake it: say exactly which shots are missing (in summary.md under Known issues, or under missing_media in a dry run) and that the user can turn on Project › App recording or attach recordings.\n".into();
    }
    let apps = if cap.apps.is_empty() {
        "Any app may be driven except terminals, system settings, password managers and Finder.".to_string()
    } else {
        format!("Only these apps may be driven: {}.", cap.apps.join(", "))
    };
    if input.kind == RunKind::DryRun {
        return format!(
            "You (Claude Code) can drive desktop apps and web apps and record the screen during the production run; cutline does not do this, it only edits the recordings afterwards. Nothing is driven or recorded in a dry run. Where the spec asks for footage of an app that isn't attached, plan it: which app or web address, which steps, and which file under captures/ each recording will be. You choose each recording's length in production (recordings have a fixed length), so plan it as the scene's length plus about 3 seconds; the length is then known, not a risk. Hold each recording's place with `video-studio standin video --out captures/<name>.mov --seconds <n> [--size 1280x800]` (a flat-colored clip) and reference those paths in project.json5. Zooms and crops onto parts of the app can only be measured from the real recording: leave them out of the dry-run edit and say so once under assumptions. `video-studio capture check` shows whether the computer allows recording and control; report a missing permission as a risk. {apps}\n"
        );
    }
    format!(
        "When the spec asks for footage of an app that isn't in the media manifest — a `- **Record:**` line, or a scene that describes steps in a desktop app or a web app — you make that footage yourself. You (Claude Code) open the app, operate it and record the screen with the app's helper. cutline does not do this; it only edits the recordings afterwards. Do this before writing project.json5.\n\
         - `video-studio capture check` — whether screen recording and control of apps are allowed. Run it first.\n\
         - `video-studio capture open \"<app name>\"` or `video-studio capture open <https://address> [--with \"<browser>\"]` — opens it and makes it the app being driven. Web apps are driven in the browser.\n\
         - `video-studio capture window --size 1280x800 --at 80,60` — set the window's size and position before recording; 16:9 or 16:10 sizes edit best.\n\
         - `video-studio capture screenshot --out <name>.png` — the app's front window. Open it with Read to see the app. One pixel is one point, so its pixel coordinates are what click, move, drag and scroll take.\n\
         - `video-studio capture click <x> <y> [--double] [--right]`, `move <x> <y>`, `drag <x> <y> <to_x> <to_y>`, `scroll <x> <y> <lines>` (negative scrolls down).\n\
         - `video-studio capture type \"<text>\"`, `video-studio capture key <combo>` (return, tab, escape, down, cmd+n, cmd+shift+z; `--times <n>`), `video-studio capture menu \"File\" \"New Project…\"` (exact names, including …).\n\
         - `video-studio capture wait <seconds>` — let a page load or an animation finish (30 at most per call).\n\
         - `video-studio capture record start --out <name>.mov --seconds <n> [--clicks]` — records the app's front window for exactly `n` seconds (600 at most) and returns at once. Perform the steps, then `video-studio capture record wait` until it reports the file. A recording can't be stopped early, only dropped with `record cancel`, so choose `n` to cover every step plus a few seconds and trim the clip in the edit.\n\
         - Add `--app \"<name>\"` to act on another app, and `--screen` to use the whole display instead of the window.\n\
         How to work:\n\
         1. Rehearse without recording: open the app, size the window, take a screenshot, do a step, take another screenshot to see what happened. Never click from memory or from an old screenshot. Learn where things are and how long each step takes.\n\
         2. Put the app back in its starting state, then record one scene (or one continuous action) per file: start the recording, perform the steps with short waits between them so a viewer can follow, and wait for the file.\n\
         3. Check each recording: `cutline probe captures/<name>.mov`, and look at frames with `cutline review` once it is in the project. Record again if a step went wrong.\n\
         4. Use the recordings in project.json5 by their paths under captures/, trimmed to the action. Recordings have no sound.\n\
         {apps} Use realistic sample content, never real personal data, and leave the app as you found it where you can (close the documents you created). The user may be at the computer: the pointer and keyboard are shared while you drive, so keep each session short.\n\
         If a capture command says a permission is missing, you cannot fix it: ask the user with AskUserQuestion to grant it (the command's message says where) and to answer when done, then try again. If it still can't be done, finish what you can and list the missing shots in summary.md under Known issues.\n"
    )
}

/// The text appended to Claude Code's system prompt for a production or dry run.
pub fn system_prompt(input: &PromptInput) -> String {
    let p = input.project;
    let rc = input.run_config;
    let dry = input.kind == RunKind::DryRun;
    let defaults = input.platforms.intent_defaults(p.intent.kind);
    let locales = if rc.locales.is_empty() { p.intent.locales.clone() } else { rc.locales.clone() };
    let exports_text = serde_json::to_string_pretty(input.exports).unwrap_or_default();
    let mut s = String::new();
    s.push_str(&format!(
        "You are producing a video for Syncfusion Video Studio with cutline, a declarative command-line video editor. You work alone inside one run folder and the user is not watching: the app shows them your progress, your questions and the results.\n\n\
         Run folder (your working directory): {}\n\
         - spec.md — the video spec the user wrote. It is the source of truth for content.\n\
         - media-manifest.json — every media file: ID, absolute path, kind, role, probe data, notes. In spec.md, `media:<id>` refers to these IDs. `kind` and `role` there are the user's hints about each file, not cutline asset roles. {HEIC_NOTE} {}\n\
{}         - outputs.json — the deliverables for this run and the matching cutline `target` and `exports`.\n\
         - run.json — the resolved run settings.\n\n",
        input.folder,
        if input.cutline_still {
            crate::viewable::on_demand_note("cutline", crate::viewable::CLAUDE_COPIES_DIR)
        } else {
            "Other HEIC/HEIF photos can't be opened in this run: this cutline is an older build without `cutline still`. Don't try other ways to convert them; if the spec needs one, say so in your report.".to_string()
        },
        if p.brand.is_some() { "         - brand/default.json — the brand as a cutline brand file; brand-preset.json — the brand preset as the user entered it.\n".trim_start() } else { "" }
    ));
    s.push_str("## What you must do\n");
    if dry {
        s.push_str(
            "This is a DRY RUN: a cheap rehearsal that finds problems before an expensive production run.\n\
             1. Read spec.md, media-manifest.json, outputs.json and, when there is a brand, brand-preset.json.\n\
             2. Write project.json5 for the whole edit, exactly as you would for production. For every file the production run will make itself — app recordings under captures/ (see Recording apps) and generated audio under audio/ (see Audio) — write a stand-in of the planned length with `video-studio standin …` and reference that path, so cutline can check the rest of the edit.\n\
             3. Run `cutline validate --json project.json5` and fix what it reports. Then `cutline expand project.json5`, and `cutline cues project.json5` when the project uses cues.\n\
             4. Run `cutline review project.json5 --out work/review` and look at what it produced (contact sheets, cut pairs, caption frames, QA; no final render). The pack shows the first export only. For each other export with a different shape (a 4:5 or 9:16 cut next to a 16:9 one), write check-<output folder name>.json5 next to project.json5: the same project with only that export left in `exports`, and review it with `--out work/review-<output folder name>`. Those copies are scratch files for this check; project.json5 stays the edit. With the stand-ins in place this covers timing, text layout, captions against the watermark and other text, and each output's framing. It cannot cover what the real footage and audio look and sound like; don't report that as a risk, it is what a dry run is.\n\
             5. Write readiness.json (schema below) and finish with one short paragraph.\n\
             You cannot render finals (`cutline render`), generate audio, or drive and record apps in a dry run; those commands are refused. Keep to the run folder itself: apart from the check-*.json5 files, don't make copies of the project to check it.\n\n",
        );
        s.push_str(
            "readiness.json:\n```json\n{\n  \"ready\": true,\n  \"ambiguities\": [{\"text\": \"what is unclear\", \"spec_line\": 12, \"detail\": \"what you would need to know\"}],\n  \"assumptions\": [{\"text\": \"what you assumed in order to continue\", \"spec_line\": 20}],\n  \"missing_media\": [{\"text\": \"what is needed and not attached\", \"spec_line\": 31}],\n  \"risks\": [{\"text\": \"a choice that may disappoint, or a check that failed\", \"detail\": \"why\"}],\n  \"planned\": [{\"text\": \"a file the production run will record or generate, with its length\", \"spec_line\": 43}],\n  \"estimated_durations\": {\"<output folder name>\": 45.0},\n  \"notes\": \"anything else the user should know before paying for a full run\"\n}\n```\n\
             `spec_line` is the 1-based line of spec.md the point is about; leave it out when there is none. `missing_media` is only for media the user must supply: footage or audio the spec needs that isn't attached and that the production run can't record or generate. Files the run makes itself (app recordings, voice-over, music, sound effects) go under `planned`, one entry each, never under `missing_media` or `risks`. Use empty lists when there is nothing to report, and set `ready` to false if a production run would fail or clearly disappoint as things stand. Be specific and brief: each entry is shown to the user as one line.\n\n",
        );
    } else {
        s.push_str(&format!(
            "1. Read spec.md, media-manifest.json, outputs.json and, when there is a brand, brand-preset.json.\n\
             2. Record the app footage the spec asks for and that isn't attached (see Recording apps), and generate the audio the spec needs (see Audio).\n\
             3. Write project.json5: one cutline project that produces every output through `exports`.\n\
             4. `cutline validate --json project.json5` until it is clean. Use `cutline expand` and `cutline cues` to check timing.\n\
             5. `cutline review project.json5`, then look at the review pack (contact sheet, cut pairs, captions, reframe sheet, QA summary) and fix what is wrong. Repeat until you would sign it off.\n\
             6. `cutline render project.json5{}` — renders every export and runs QA on each. Fix QA errors and render again; do not pass `--allow-qa-errors` unless a finding is a false alarm, and say so in summary.md.\n\
             7. Check each rendered file's `.qa.json`, then write summary.md (sections below).\n\n",
            if rc.quality == Quality::Draft { " --draft" } else { "" }
        ));
        if rc.quality == Quality::Draft {
            s.push_str("This run is DRAFT quality: render with `--draft` (fast, lower resolution) so the user can judge the edit. Everything else is the same.\n\n");
        }
        s.push_str(
            "summary.md must have these sections: **What I made** (one line per output file), **Assumptions** (every choice the spec left open, and every question you could not get answered), **QA results** (per output: pass, or the findings), **Known issues**, and **Before publishing** — cutline's publishing checklist, including the synthetic-content disclosure for platforms that require it when any voice or music was generated.\n\n",
        );
    }
    s.push_str("## The video\n");
    s.push_str(&project_summary(p, input.outputs, input.manifest));
    s.push_str(&format!("Editing defaults for this intent: {} pacing, {} captions, music level {}. cutline style: `{}`. The spec overrides these.\n\n", defaults.pacing, defaults.captions, defaults.music_level, p.intent.kind.cutline_style()));
    s.push_str("## Outputs\n");
    s.push_str(&format!(
        "Put this `target` and these `exports` in project.json5 unchanged (names and `out` paths matter: the app collects files from them):\n```json5\n{exports_text}\n```\n"
    ));
    for o in input.outputs {
        let mut limits: Vec<String> = Vec::new();
        if let Some(d) = o.limits.min_duration_s {
            limits.push(format!("at least {}", format_duration(d)));
        }
        if let Some(d) = o.limits.max_duration_s {
            limits.push(format!("at most {}", format_duration(d)));
        }
        if let Some(b) = o.limits.max_bytes {
            limits.push(format!("under {}", crate::paths::format_bytes(b)));
        }
        let limits = if limits.is_empty() { String::new() } else { format!(" Limits: {}.", limits.join(", ")) };
        s.push_str(&format!(
            "- **{}** → outputs/{}/ — {}×{} ({}), captions {}{}.{limits} {}\n",
            o.name,
            o.slug,
            o.width,
            o.height,
            o.aspect_label(),
            o.captions,
            if o.audio { "" } else { ", no audio" },
            o.notes
        ));
    }
    s.push_str("The same edit serves every export; cutline re-fits framing and text per target. If an output needs a shorter cut (a platform limit), use `variants` rather than a second project.\n");
    if !locales.is_empty() {
        s.push_str(&format!("Languages: {}. The first is the base language; add the others as cutline `locales` with translated strings and voice-over.\n", locales.join(", ")));
    }
    s.push('\n');
    s.push_str("## Finish\n");
    s.push_str(&input.finish.directions());
    s.push('\n');
    s.push_str("## Brand\n");
    s.push_str(&brand_section(p));
    s.push('\n');
    s.push_str("## Audio\n");
    s.push_str(&audio_section(input));
    s.push('\n');
    s.push_str("## Recording apps\n");
    s.push_str(&capture_section(input));
    s.push('\n');
    s.push_str(
        "## Rules of this environment\n\
         - Shell: only `cutline …`, `video-studio eleven …`, `video-studio capture …` and, in a dry run, `video-studio standin …` run. One plain command per call: no pipes, redirections, `cd`, variables or other programs (`ls`, `cat`, `ffmpeg`, `python`, `open`, `osascript`, `screencapture` are refused). Use the Read, Glob and Grep tools to look at files; they also show images, so open the review pack's PNGs with Read.\n\
         - Files: you may write only inside the run folder, and may read the run folder, the media listed in the manifest, the brand assets and cutline's own folder. Reference media in project.json5 by the absolute paths in media-manifest.json; never copy media.\n\
         - Run every command from the run folder and give paths relative to it (`--out work/review`, not a path that starts inside work/), so files land where you expect.\n\
         - Keep scratch files under work/. Screenshots and recordings you make go to captures/. logs/ belongs to the app.\n",
    );
    s.push_str(if rc.web_access {
        "- Web search and fetch are available for this run.\n"
    } else {
        "- There is no web access in this run.\n"
    });
    s.push_str(
        "- Questions: when something only the user can decide would change the video, and guessing wrong would waste the run, ask with the AskUserQuestion tool. Give two to four concrete options and put the one you recommend first. The user may be away: if the answer says to proceed on your assumption, take your first option and record the question and the assumption in summary.md. Do not ask about things the spec, the brand or common sense already settle.\n\
         - Don't stop early. Finish every output, or explain in summary.md exactly what blocked it.\n\n",
    );
    if !input.cutline_guide.trim().is_empty() {
        s.push_str("## cutline workflow guide (from `cutline guide`)\n\n");
        s.push_str(input.cutline_guide.trim());
        s.push('\n');
    }
    s
}

/// The first user message of the run.
pub fn user_prompt(kind: RunKind, revise: Option<&Revise>, previous: Option<&RunRecord>) -> String {
    match (kind, revise) {
        (_, Some(r)) => format!(
            "Revise the video from the previous run.\n\nThe previous run's project.json5, audio/ and work/ have been copied into this new run folder, and its summary is in previous-summary.md{}. Outputs go to this folder's outputs/ as before.\n\nFeedback from the user:\n\n{}\n\nMake the changes, keep everything the feedback doesn't touch, reuse the existing audio where the text hasn't changed, then validate, review, render and write summary.md as usual. In summary.md, start with what changed in this revision.",
            previous.map(|p| format!(" (previous run folder: {})", p.folder)).unwrap_or_default(),
            r.feedback.trim()
        ),
        (RunKind::DryRun, None) => "Do the dry run for the video described in spec.md: write project.json5, validate and review it, then write readiness.json.".into(),
        (RunKind::Production, None) => "Produce the video described in spec.md: generate the audio it needs, write project.json5, validate, review, render every output, check QA, then write summary.md.".into(),
    }
}

/// How Claude sees HEIC/HEIF photos (FEAT-0004): through the JPEG copies the app makes.
pub const HEIC_NOTE: &str = "HEIC and HEIF photos can't be opened with Read; open the JPEG at viewable_path (listed as \"view:\") to see the picture, and keep referring to the original file (its path in project.json5, media:<id> in spec.md).";

/// The photos in `manifest` that have (or failed to get) a viewable copy, one line each, for a
/// media list written before the copies were made (the spec assistant's).
pub fn viewable_list(manifest: &[ManifestEntry]) -> String {
    let mut s = String::new();
    for m in manifest {
        if m.viewable_path.is_some() {
            s.push_str(&format!("- `{}` — view: {}/{}.jpg\n", m.id, crate::viewable::PREVIEWS_DIR, m.id));
        } else if let Some(e) = &m.viewable_error {
            s.push_str(&format!("- `{}` — can't be viewed: {}\n", m.id, e.trim().replace('\n', " ")));
        }
    }
    if s.is_empty() {
        s
    } else {
        format!("\nViewable copies of HEIC/HEIF photos:\n{s}")
    }
}

/// What the spec assistant can use to open a HEIC/HEIF photo outside the media list.
#[derive(Debug, Clone, Copy)]
pub enum HeicTool<'a> {
    /// A cutline with `cutline still`: the only shell command the assistant may run.
    Still(&'a Utf8Path),
    /// A cutline from before `still` existed (BUG-0002): no shell; Claude asks for an update.
    Outdated(&'a Utf8Path),
    None,
}

/// The spec assistant's system prompt (spec §4.3).
pub fn assistant_system_prompt(
    folder: &Utf8Path,
    project_summary: &str,
    read_paths: &[String],
    reference_paths: &[String],
    web_access: bool,
    heic: HeicTool<'_>,
) -> String {
    let mut extra = if read_paths.is_empty() {
        String::new()
    } else {
        format!("\n- You may also read (never edit) the media folders: {}.", read_paths.join(", "))
    };
    if !reference_paths.is_empty() {
        extra.push_str(&format!(
            "\n- The user shared these folders as reference material (briefs, docs, source files). Read them (never edit) with Read, Glob and Grep when they help: {}.",
            reference_paths.join(", ")
        ));
    }
    const PREVIEWS: &str = crate::viewable::PREVIEWS_DIR;
    let no_shell = "You can't run shell commands or edit other files; such attempts are denied automatically.";
    let (shell, heic_more) = match heic {
        HeicTool::Still(c) => (
            "You can't edit other files, and the only shell command you may run is `cutline still` as described next; anything else is denied automatically.".to_string(),
            format!(
                " {}",
                crate::viewable::on_demand_note(
                    &crate::claude::broker::shell_quote(c.as_str()),
                    folder.join(crate::viewable::CLAUDE_COPIES_DIR).as_ref()
                )
            ),
        ),
        HeicTool::Outdated(c) => (
            no_shell.to_string(),
            format!(
                " Other HEIC/HEIF files can't be opened in this session: the installed cutline ({c}) is an older build without the `still` command that converts them. Don't try other ways to convert or open them. If the user wants such photos, tell them once to update cutline (Settings › cutline shows which one is used) and start a new message, or to add the photos as media, or to describe them in words."
            ),
        ),
        HeicTool::None => (
            no_shell.to_string(),
            " Other HEIC/HEIF files can't be opened in this session: ask the user to add them as media, or describe them in words.".to_string(),
        ),
    };
    extra.push_str("\n- Any other folder is outside this session. If the user points you at one, tell them to add it with the Folders button under the message box, then try again.");
    format!(
        "You are helping the user write the spec for a video in Syncfusion Video Studio. The spec is the Markdown file spec.md in your working folder {folder}. When it is complete, another Claude session turns it into a finished video with cutline (a declarative video editor) and ElevenLabs (voice and music), so the spec must say clearly what the video shows and says.\n\
         - Edit only spec.md. The user reviews every change you make as a diff and accepts or rejects each hunk, so keep edits focused and leave the rest of the file exactly as it is.{extra}\n\
         - {shell}{}\n\
         - {HEIC_NOTE} The copies are in {PREVIEWS}/ in your working folder.{heic_more}\n\
         - Media is referenced as `media:<id>`: write `![label](media:<id>)` to place an item. Use only IDs from the list below; never invent one. If the video needs footage that isn't attached and can't be recorded from an app (see the next point), say so in the spec in words.\n\
         - Footage of an app does not have to exist yet. When \"App recording\" is on in the project summary below, the production run records it: Claude Code itself opens the desktop app or the web app (in a browser) on this computer, clicks, types and uses menus, and records the screen. cutline is not involved in that; it only edits the recordings. So never tell the user such footage can't be captured or must be recorded by hand. Write the scene so the run can perform it: a `- **Record:**` line naming the app (as shown in Applications) or the web address, followed by numbered steps that say exactly what to click, type or choose and what should be on screen at the end, plus any sample content to use. Keep each recorded scene to one continuous action. Recordings have no sound. If App recording is off, or limited to apps that don't include the one needed, tell the user they can change it under Project › App recording, or attach a recording instead. The first time a run drives apps, the computer asks the user to allow Screen Recording and Accessibility for Syncfusion Video Studio.\n\
         - Structured blocks, all plain Markdown: a scene is a heading that starts with \"Scene\" followed by `- **Duration:**`, `- **Media:**` and `- **On-screen text:**` lines; text to be voiced goes in a fenced block tagged `vo`; a music brief goes in a fenced block tagged `music` with `mood:`, `tempo:` and `source:` lines; the call to action goes under a \"Call to action\" heading.\n\
         - Voice-over is spoken at about 150 words per minute. Keep the script within the target length.\n\
         - How the video is finished is not written in the spec unless this video should differ: transitions between scenes, cuts on the music's beat, a short lead-in before the first word, and how the music fades in and out come from Settings › Finish (or Project › Finish) and every run applies them. In a `music` block describe the mood and tempo; the run asks for a steady beat it can cut to.\n\
         - When you need the user to choose or to tell you something only they know (audience, length, tone, call to action, must-include shots), use the AskUserQuestion tool rather than guessing.\n\
         - Replace `{{{{…}}}}` template placeholders with real content or remove them.\n\n\
         About this project:\n{project_summary}",
        if web_access { "\n- Web search and fetch are available when facts need checking." } else { "" }
    )
}

/// The prompt behind the "Gather missing info" action (spec §4.3).
pub const GATHER_INFO_PROMPT: &str = "Read spec.md and find what a video editor would still need to know to produce this video well: audience, length, tone, the call to action, must-include shots, what each scene shows, missing narration, placeholders left from the template, and anything ambiguous. Interview me about the gaps with the AskUserQuestion tool, a few questions at a time, most important first. Then fold my answers into spec.md.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::{BrandPreset, BrandRef};
    use crate::media::Probe;
    use crate::presets::{cutline_exports, resolve_run_outputs, OutputConfig};
    use crate::project::IntentKind;

    fn fixture() -> (Project, RunConfig, Vec<ResolvedOutput>, Value, Vec<ManifestEntry>, Platforms) {
        let platforms = Platforms::bundled();
        let mut p = Project::new("Spring launch");
        p.intent.kind = IntentKind::Marketing;
        p.intent.audience = Some("Engineering managers".into());
        p.intent.target_length_s = Some(45.0);
        p.intent.locales = vec!["en".into(), "de".into()];
        let mut b = BrandPreset::new("Acme");
        b.rules = "Do: lead with the product.\nDon't: use stock music.".into();
        b.voice.voice_id = "voice-abc".into();
        p.brand = Some(BrandRef::of(&b));
        p.output_configs = vec![
            OutputConfig {
                id: "o1".into(),
                preset: "youtube_shorts".into(),
                name: None,
                overrides: Default::default(),
            },
            OutputConfig { id: "o2".into(), preset: "linkedin".into(), name: None, overrides: Default::default() },
        ];
        let mut rc = RunConfig::new("Social pack");
        rc.outputs = vec!["o1".into(), "o2".into()];
        rc.eleven_credit_cap = 7000;
        let outs = resolve_run_outputs(&p, &rc, &platforms).unwrap();
        let ex = cutline_exports(&outs);
        let manifest = vec![
            ManifestEntry {
                id: "m-hero".into(),
                kind: "video".into(),
                role: Some("footage".into()),
                path: Some("/footage/hero.mov".into()),
                status: MediaStatus::Ok,
                probe: Some(Probe {
                    duration_s: Some(32.1),
                    width: Some(3840),
                    height: Some(2160),
                    fps: Some("30/1".into()),
                    ..Probe::default()
                }),
                notes: Some("Use the second half".into()),
                tags: vec![],
                viewable_path: None,
                viewable_error: None,
            },
            ManifestEntry {
                id: "m-gone".into(),
                kind: "audio".into(),
                role: None,
                path: None,
                status: MediaStatus::Missing,
                probe: None,
                notes: None,
                tags: vec![],
                viewable_path: None,
                viewable_error: None,
            },
        ];
        (p, rc, outs, ex, manifest, platforms)
    }

    fn production(p: &Project, provider: crate::eleven::VoiceProvider) -> String {
        let (_, rc, outs, ex, manifest, platforms) = fixture();
        system_prompt(&PromptInput {
            project: p,
            run_config: &rc,
            kind: RunKind::Production,
            folder: "/runs/r1".into(),
            outputs: &outs,
            exports: &ex,
            manifest: &manifest,
            cutline_guide: "",
            cutline_still: true,
            eleven_available: true,
            default_voice: Some("voice-abc"),
            voice_provider: provider,
            cartesia_available: true,
            voice_choosing: None,
            platforms: &platforms,
            finish: &crate::finish::Finish::default(),
        })
    }

    #[test]
    fn delivery_tags_follow_the_voice_model() {
        use crate::eleven::VoiceProvider;
        let (mut p, ..) = fixture();
        let s = production(&p, VoiceProvider::Elevenlabs);
        for needle in [
            "model is `eleven_v4` (the default voice model)",
            "`[warmly]`",
            "`[confident]`",
            "`[reassuring]`",
            "open every passage with one tag",
            "have no effect with this voice model",
        ] {
            assert!(s.contains(needle), "missing {needle:?}");
        }
        assert!(!s.contains("Give each passage its neighbours") && !s.contains("<spell>"));

        p.brand.as_mut().unwrap().snapshot.voice.model = Some("eleven_multilingual_v2".into());
        let old = production(&p, VoiceProvider::Elevenlabs);
        assert!(
            old.contains("reads delivery tags in square brackets aloud")
                && old.contains("Give each passage its neighbours")
        );
        assert!(!old.contains("`[reassuring]`"));

        let cartesia = production(&p, VoiceProvider::Cartesia);
        assert!(cartesia.contains("<spell>WPF</spell>") && cartesia.contains("the helper removes them"));
        assert!(!cartesia.contains("`[reassuring]`") && !cartesia.contains("Give each passage its neighbours"));

        assert!(assistant_voice_notes(VoiceProvider::Elevenlabs, None).contains("`[confident]`"));
        assert!(assistant_voice_notes(VoiceProvider::Elevenlabs, Some("eleven_flash_v2_5")).is_empty());
        assert!(assistant_voice_notes(VoiceProvider::Cartesia, None).is_empty());
        assert_eq!(
            assistant_brand_notes(&p),
            "Brand rules, verbatim from the brand owner. Follow them exactly:\n> Do: lead with the product.\n> Don't: use stock music.\n"
        );
        p.brand = None;
        assert!(assistant_brand_notes(&p).is_empty());
    }

    #[test]
    fn only_the_chosen_instruction_sets_reach_claude() {
        use crate::brand::InstructionSet;
        let (mut p, rc, outs, _, manifest, _) = fixture();
        let _ = rc;
        let b = p.brand.as_mut().unwrap();
        let mut reports = InstructionSet::new("Bold Reports");
        reports.text =
            "Audience: report authors and BI developers.\nSay \"Bold Reports\", never \"BoldReports\".".into();
        let mut wpf = InstructionSet::new("Syncfusion WPF");
        wpf.text = "Desktop developers on .NET.".into();
        let empty = InstructionSet::new("Empty");
        b.instructions = vec![reports.id.clone(), empty.id.clone(), "ins_gone".into()];
        b.snapshot.instructions = vec![wpf, reports, empty];
        let summary = project_summary(&p, &outs, &manifest);
        assert!(summary.contains("Brand instructions chosen for this video"));
        assert!(
            summary.contains("**Bold Reports**\n> Audience: report authors and BI developers.\n> Say \"Bold Reports\"")
        );
        assert!(!summary.contains("Desktop developers") && !summary.contains("**Empty**"));
        let run = p.brand.as_ref().unwrap().for_run();
        assert_eq!(run.instructions.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["Bold Reports", "Empty"]);
        assert!(production(&p, crate::eleven::VoiceProvider::Elevenlabs).contains("**Bold Reports**"));

        p.brand.as_mut().unwrap().instructions.clear();
        assert!(!project_summary(&p, &outs, &manifest).contains("Brand instructions"));
    }

    /// BUG-0002: a run offers `cutline still` only when this cutline has it.
    #[test]
    fn run_prompt_heic_depends_on_cutline_still() {
        let (p, rc, outs, ex, manifest, platforms) = fixture();
        let make = |still: bool| {
            system_prompt(&PromptInput {
                project: &p,
                run_config: &rc,
                kind: RunKind::Production,
                folder: "/runs/r1".into(),
                outputs: &outs,
                exports: &ex,
                manifest: &manifest,
                cutline_guide: "",
                cutline_still: still,
                eleven_available: true,
                default_voice: None,
                voice_provider: Default::default(),
                cartesia_available: false,
                voice_choosing: None,
                platforms: &platforms,
                finish: &crate::finish::Finish::default(),
            })
        };
        assert!(make(true).contains("`cutline still '<photo>' --out 'photo-copies/<name>.jpg'"));
        let old = make(false);
        assert!(!old.contains("cutline still '<photo>'") && old.contains("older build without `cutline still`"));
    }

    #[test]
    fn production_prompt_carries_everything_the_spec_lists() {
        let (p, rc, outs, ex, manifest, platforms) = fixture();
        let s = system_prompt(&PromptInput {
            project: &p,
            run_config: &rc,
            kind: RunKind::Production,
            folder: "/runs/r1".into(),
            outputs: &outs,
            exports: &ex,
            manifest: &manifest,
            cutline_guide: "GUIDE TEXT",
            cutline_still: true,
            eleven_available: true,
            default_voice: Some("voice-abc"),
            voice_provider: Default::default(),
            cartesia_available: false,
            voice_choosing: None,
            platforms: &platforms,
            finish: &crate::finish::Finish::default(),
        });
        assert!(
            s.contains("## Finish\n") && s.contains("snap_to_beats") && s.contains("smoothleft"),
            "the finish is part of every run"
        );
        for needle in [
            "/runs/r1",
            "media-manifest.json",
            "`m-hero` — video, footage, 32.1 s, 3840×2160, 30/1 fps — hero.mov — note: Use the second half",
            "`m-gone` — audio, MISSING",
            "Intent: Marketing",
            "Audience: Engineering managers",
            "Target length: 45.0 s",
            "\"name\": \"youtube-shorts\"",
            "outputs/linkedin/linkedin.mp4",
            "at most 3:00",
            "> Don't: use stock music.",
            "default voice is `voice-abc`",
            "at most 7000 ElevenLabs credits",
            "Languages: en, de",
            "no web access",
            "AskUserQuestion",
            "summary.md",
            "Before publishing",
            "GUIDE TEXT",
            "cutline render project.json5",
            "## Recording apps",
            "You (Claude Code) open the app, operate it and record the screen",
            "video-studio capture record start --out <name>.mov --seconds <n>",
            "Any app may be driven except terminals",
        ] {
            assert!(s.contains(needle), "missing {needle:?} in:\n{s}");
        }
        assert!(!s.contains("readiness.json") && !s.contains("--draft"));
        let mut draft = rc.clone();
        draft.quality = Quality::Draft;
        draft.web_access = true;
        let d = system_prompt(&PromptInput {
            project: &p,
            run_config: &draft,
            kind: RunKind::Production,
            folder: "/r".into(),
            outputs: &outs,
            exports: &ex,
            manifest: &manifest,
            cutline_guide: "",
            cutline_still: true,
            eleven_available: false,
            default_voice: None,
            voice_provider: Default::default(),
            cartesia_available: false,
            voice_choosing: None,
            platforms: &platforms,
            finish: &crate::finish::Finish::default(),
        });
        assert!(
            d.contains("cutline render project.json5 --draft")
                && d.contains("Web search and fetch are available")
                && d.contains("no ElevenLabs key is configured")
        );
        // no voice fixed: the run is told how Settings wants one chosen
        let how =
            crate::eleven::voices::choosing(crate::eleven::VoicePreference::Latest, crate::eleven::VoiceGender::Male);
        let v = system_prompt(&PromptInput {
            project: &p,
            run_config: &rc,
            kind: RunKind::Production,
            folder: "/r".into(),
            outputs: &outs,
            exports: &ex,
            manifest: &manifest,
            cutline_guide: "",
            cutline_still: true,
            eleven_available: true,
            default_voice: None,
            voice_provider: Default::default(),
            cartesia_available: false,
            voice_choosing: Some(&how),
            platforms: &platforms,
            finish: &crate::finish::Finish::default(),
        });
        assert!(
            v.contains("so you choose one") && v.contains("a male voice") && v.contains("most recently added"),
            "the voice preferences reach the run"
        );
    }

    #[test]
    fn dry_run_prompt_forbids_rendering_and_asks_for_the_report() {
        let (p, rc, outs, ex, manifest, platforms) = fixture();
        let s = system_prompt(&PromptInput {
            project: &p,
            run_config: &rc,
            kind: RunKind::DryRun,
            folder: "/r".into(),
            outputs: &outs,
            exports: &ex,
            manifest: &manifest,
            cutline_guide: "",
            cutline_still: true,
            eleven_available: true,
            default_voice: None,
            voice_provider: Default::default(),
            cartesia_available: false,
            voice_choosing: None,
            platforms: &platforms,
            finish: &crate::finish::Finish::default(),
        });
        assert!(
            s.contains("DRY RUN")
                && s.contains("readiness.json")
                && s.contains("cannot render finals")
                && s.contains("no audio is generated")
        );
        assert!(s.contains("\"exports\""), "the same outputs as production");
        assert!(s.contains("Nothing is driven or recorded in a dry run") && !s.contains("capture record start"));
        // run-made files get stand-ins and their own list, so they aren't reported as missing
        assert!(
            s.contains("video-studio standin video --out captures/")
                && s.contains("video-studio standin audio --out audio/")
        );
        assert!(s.contains("\"planned\"") && s.contains("never under `missing_media`"));
        assert!(
            s.contains("No voice is fixed for this project") && s.contains("the production run will choose one itself")
        );
        assert!(s.contains("--text-file audio/<name>.txt") && s.contains("transcript: \"audio/<name>.json\""));
        assert!(s.contains("check-<output folder name>.json5") && s.contains("Settings › ElevenLabs"));
        // a project that turns app recording off, or limits it, says so in both kinds of run
        let mut off = p.clone();
        off.app_capture.enabled = false;
        let mut only = p.clone();
        only.app_capture.apps = vec!["Safari".into()];
        for kind in [RunKind::DryRun, RunKind::Production] {
            let prompt = |project: &Project| {
                system_prompt(&PromptInput {
                    project,
                    run_config: &rc,
                    kind,
                    folder: "/r".into(),
                    outputs: &outs,
                    exports: &ex,
                    manifest: &manifest,
                    cutline_guide: "",
                    cutline_still: true,
                    eleven_available: true,
                    default_voice: None,
                    voice_provider: Default::default(),
                    cartesia_available: false,
                    voice_choosing: None,
                    platforms: &platforms,
                    finish: &crate::finish::Finish::default(),
                })
            };
            let s = prompt(&off);
            assert!(
                s.contains("App recording is turned off")
                    && s.contains("App recording: off")
                    && !s.contains("capture open")
            );
            assert!(prompt(&only).contains("Only these apps may be driven: Safari."));
        }
        assert!(user_prompt(RunKind::DryRun, None, None).contains("readiness.json"));
        assert!(user_prompt(RunKind::Production, None, None).contains("summary.md"));
        let rev = user_prompt(
            RunKind::Production,
            Some(&Revise { previous_run: "run_1".into(), feedback: " make the hook faster ".into() }),
            None,
        );
        assert!(rev.contains("make the hook faster") && rev.contains("previous-summary.md"));
    }

    #[test]
    fn assistant_prompt() {
        let (p, _, outs, _, manifest, _) = fixture();
        let summary = project_summary(&p, &outs, &manifest);
        let s = assistant_system_prompt(
            "/scratch/s1".into(),
            &summary,
            &["/footage".to_string()],
            &["/briefs".to_string()],
            false,
            HeicTool::None,
        );
        assert!(
            s.contains("Edit only spec.md")
                && s.contains("/footage")
                && s.contains("reference material")
                && s.contains("/briefs")
                && s.contains("`m-hero`")
                && s.contains("{{…}}")
                && s.contains("Brand preset: Acme")
        );
        assert!(!s.contains("Web search"));
        assert!(
            s.contains("App recording: on.") && s.contains("- **Record:**") && s.contains("cutline is not involved")
        );
        assert!(s.contains("can't run shell commands") && !s.contains("cutline still"));
        assert!(assistant_system_prompt("/s".into(), "", &[], &[], true, HeicTool::None).contains("Web search"));
    }

    /// BUG-0001: HEIC photos outside the media list (a media or reference folder) can be opened
    /// through a copy Claude makes with `cutline still`.
    #[test]
    fn assistant_prompt_heic_on_demand() {
        let s = assistant_system_prompt(
            "/s".into(),
            "",
            &["/Users/me/Downloads".to_string()],
            &[],
            false,
            HeicTool::Still(Utf8Path::new("/Applications/Video Studio.app/cutline")),
        );
        assert!(s.contains("only shell command you may run is `cutline still`"), "{s}");
        // BUG-0002: the exact command with the absolute copies folder, and no shell tricks
        assert!(
            s.contains("`'/Applications/Video Studio.app/cutline' still '<photo>' --out '/s/photo-copies/<name>.jpg' --max-side 1568`"),
            "{s}"
        );
        assert!(s.contains("don't try Read on it") && s.contains("no variables") && s.contains("temporary folders"));
    }

    /// BUG-0002: with a cutline that has no `still`, there is no shell and Claude is told why,
    /// so it asks for an update instead of trying commands that can't work.
    #[test]
    fn assistant_prompt_outdated_cutline() {
        let s = assistant_system_prompt(
            "/s".into(),
            "",
            &["/Users/me/Downloads".to_string()],
            &[],
            false,
            HeicTool::Outdated(Utf8Path::new("/usr/local/bin/cutline")),
        );
        assert!(s.contains("can't run shell commands"), "{s}");
        assert!(!s.contains("only shell command you may run"), "{s}");
        assert!(s.contains("(/usr/local/bin/cutline) is an older build without the `still` command"), "{s}");
        assert!(s.contains("update cutline"));
    }
}
