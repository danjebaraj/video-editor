# Syncfusion Video Studio: UI Specification

Source notes: `plans/Video Editor UI Spec.md`. Drafted 2026-11-14.

Syncfusion Video Studio is a desktop app for preparing videos with Claude Code and `cutline`. The user describes the video in a Markdown spec, refines it with Claude, attaches media by reference, picks brand and output settings, validates everything, then submits a production run in which Claude Code drives `cutline` to render the outputs.

---

## 0. Decisions from Q&A

| Topic | Decision |
|---|---|
| Tech stack | A shared **Rust core** crate (`video-studio-core`) holds the project model, validation, run orchestration, Claude Code and ElevenLabs clients, and the headless CLI. The **macOS shell is Swift** and reuses the Just Notes editor. It calls the core over FFI. A later Windows shell built on `windows-rs` reuses the same core. |
| Run model | **Claude Code drives cutline.** During a production run, Claude writes `project.json5` and runs `cutline validate / review / render` itself, following `CLAUDE_GUIDE.md`. The app allows Bash only for the `cutline` binary and the app's own helper. |
| ElevenLabs | **Claude generates audio during the run** through an **app-provided helper** (`video-studio eleven …`). The API key stays in the Keychain and never reaches Claude. The app enforces a per-run credit cap, caches identical requests and logs every call. |
| Project file | A **single JSON file** (`*.cutproj`). It holds the Markdown spec as a string, media references, intent, brand preset, output configs and run history. |
| Validation | **Static checks plus an optional cheap dry run.** The dry run uses an inexpensive model to write `project.json5` and run `cutline validate` and a draft review. It doesn't render finals or generate audio. |
| Outputs | **One run can produce several outputs**: one `project.json5` with cutline `exports` and variants, and one subfolder and log per output. |
| App name | **Syncfusion Video Studio.** The bundle ID is `com.syncfusion.videostudio`, the CLI binary is `video-studio`, and the Rust crates are `video-studio-core` and `video-studio-cli`. |
| Pasted images | Stored in a **`<name>.cutproj.assets/` sidecar** next to the project. This is the only exception to "reference, don't copy" (§5). |
| ElevenLabs licensing | **Confirmed:** the ElevenLabs plan covers commercial use of generated voice and music for business publishing. |

---

## 1. Goals and non-goals

### Goals
- Write a rich Markdown spec (text, images, Mermaid, tables, media references) that gives Claude clear context for the video.
- Improve the spec with Claude: edits reviewed as diffs, and questions Claude asks back.
- Choose the model used for spec help and, separately, the model used for production.
- Reference media without copying it, and relink it easily when files move.
- Keep brand presets and output configs as reusable, switchable sets.
- Catch failures and quality problems before an expensive run.
- Run productions from the UI or the command line, with progress, cancellation and detailed logs.
- Look professional enough for business use.
- Mac first, with every platform-specific piece behind an interface so Windows can follow.

### Non-goals (v1)
- A timeline or NLE-style manual editor. cutline is declarative, and Claude authors the edit.
- Running without Claude Code. The app requires it and says so when the CLI is missing.
- Cloud or multi-user collaboration, accounts, or a hosted render farm.
- Editing cutline's `project.json5` by hand in the UI. It's viewable as a run artifact only.

---

## 2. Glossary

| Term | Meaning |
|---|---|
| **Project** | A `.cutproj` file: the spec plus everything needed to produce the video. |
| **Spec** | The Markdown document describing the video. Stored inside the project. |
| **Media item** | A reference to an external file (video, audio, image, font, Scribe JSON, `.tape`, `.demo.json5`). Never copied. |
| **Brand preset** | A named set of logos, colors, fonts, voice and style rules, stored in the app library. |
| **Output config** | A named delivery target (for example "YouTube 1080p", "Shorts", "LinkedIn 4:5") mapped to cutline target and export settings. |
| **Run config** | A named, saved combination of output configs, production model, budget and output folder. Several can exist per project. |
| **Run** | One execution of a run config: dry run or production. |
| **Run folder** | The folder where a run writes its work files, outputs and logs. |

---

## 3. Architecture

```
┌──────────────── macOS app (Swift, AppKit/SwiftUI) ───────────────┐
│ Spec editor (Just Notes)  Media  Brand  Outputs  Runs  Settings  │
└──────────────────────────┬───────────────────────────────────────┘
                           │ UniFFI bindings
┌──────────────────────────▼───────────────────────────────────────┐
│ video-studio-core (Rust)                                        │
│  project model + schema │ media resolver │ validation             │
│  claude client (stream-json, permission broker)                   │
│  run orchestrator │ eleven client + cache + budget │ logs         │
│  platform traits: Secrets, FileRefs, Notifications, ProcessGroup  │
└───────┬───────────────────────────┬──────────────────────────────┘
        │                           │
  video-studio (CLI binary)   spawns: claude, cutline, ffprobe
```

- **Workspace placement:** `crates/video-studio-core` and `crates/video-studio-cli` in this repository, so the core can share types with `cutline-core` (for example target names, and validation of `project.json5`). The Swift app lives in `apps/macos/` and is built with XcodeGen, as Just Notes is.
- **FFI:** UniFFI generates the Swift bindings. The core exposes async operations as callbacks or event streams; the UI never blocks on the core.
- **Platform traits** (each has a macOS implementation now and a Windows stub):
  - `Secrets`: Keychain on macOS, Credential Manager on Windows (the `keyring` crate).
  - `FileRefs`: security-scoped bookmarks on macOS, plain paths plus a file ID on Windows.
  - `ProcessGroup`: spawning and killing a process tree (a process group on macOS, a Job Object on Windows).
  - `Notifications`, `RevealInFileManager` and `OpenWith`.
- **Rule for Windows:** no Mac-only API outside the Swift shell or a trait implementation. Paths are handled as `camino::Utf8PathBuf`, the core never assumes `/`, and line endings are normalized on write.
- **Reuse from Just Notes:** the TextKit 2 editor, slash commands, table tools, image paste and validation, the `WKWebView` preview with markdown-it, Mermaid, KaTeX and highlight.js, the Claude panel, stream-json parsing, the permission broker, AskUserQuestion cards, and per-hunk diff review. Where Just Notes logic is pure (for example `MarkdownCore`), it's either ported to the Rust core or kept in Swift if it's UI-only. Each piece's choice is recorded in `docs/decisions.md`.

---

## 4. Claude Code integration

### 4.1 Detection
- Claude Code is required. Detection works as in Just Notes: the path from Settings, then `command -v claude` in the login shell, then the usual install locations. It's re-checked at launch, on app activation and from Settings.
- If Claude Code is missing, the app opens a blocking setup screen with install instructions and a Re-check button. Projects can still be opened and the spec edited, but every Claude action, validation step that needs a model, and run is disabled.
- Sign-in is handled by the CLI. The app never asks for or stores Anthropic credentials.

### 4.2 Model selection
- There are two independent settings per project, each with an app-wide default:
  - **Spec assistant model**: used for chat, spec edits and questions.
  - **Production model**: used for runs. Set on the run config, and overridable per run.
  - The **dry run model** defaults to a cheaper model than production and is set separately.
- Picker: family (Opus, Sonnet, Haiku, Fable, …), then version, where **Latest** passes the CLI alias and a version passes the full model ID. A **Custom ID** field is also available.
- The model list ships with the app and can be refreshed from Settings. It's never hard-coded in more than one place.
- The effort level is shown only if `claude --help` lists `--effort`.
- The model ID actually used is recorded in every run log.

### 4.3 Spec assistant sessions
- These behave like Just Notes: `claude -p` with stream-json in and out, `--resume` per session, and a permission broker over stdio.
- The working folder is a per-project scratch folder that holds a materialized copy of the spec (`spec.md`), plus read access to referenced media folders.
- Tools allowed: Read, Edit, Write (limited to `spec.md`), Glob, Grep, AskUserQuestion, and WebSearch/WebFetch when toggled on. Bash is denied.
- **Edits are shown as a per-hunk diff** in the editor. When the user accepts a hunk, the change is written back into the project's `spec` field. Rejecting restores the previous text.
- **Questions from Claude** (AskUserQuestion) appear as cards, as in Just Notes. A "Gather missing info" action asks Claude to interview the user about gaps (audience, length, tone, call to action, must-include shots, and so on) and fold the answers into the spec.
- The system context given to Claude includes a summary of the project: intent, brand preset, output configs, and the media list with durations and kinds. Claude can then refer to media by ID.

---

## 5. The spec editor

- The full Just Notes editor: source view and preview (⌘/), tables, alerts, islands, slash commands, find and replace.
- **Images**:
  - A dragged existing image file becomes a media reference. The spec contains `![alt](media:<id>)`.
  - Pasted clipboard images have no source file, so they're saved to a sidecar folder next to the project (`<name>.cutproj.assets/`) and also referenced by `media:<id>`. This is the only case where the app writes media files.
  - The sidecar travels with the project. Duplicate copies it, Save As copies it next to the new file, and the relink sheet (§6.3) treats a missing sidecar like a missing media root.
  - Images in the sidecar are named by content hash (`<sha256>.png`), so pasting the same picture twice stores it once. Pasted images are converted to PNG, at most 4096 px on the longest side, as in Just Notes.
  - Unused sidecar images (no longer referenced by the spec) are listed by File › Clean Up Pasted Images… and moved to the Trash after confirmation. They are never deleted automatically.
- **Mermaid** and **KaTeX** render in the preview, offline.
- **Media embeds**: `![label](media:<id>)` in the preview shows a thumbnail with duration, kind and status (ok, missing, changed). For video and audio, hovering scrubs and clicking plays in Quick Look.
- **Structured blocks** (optional, offered by slash commands, all plain Markdown that Claude can read):
  - `/scene`: scene heading with fields such as duration, media, voice-over text and on-screen text.
  - `/vo`: voice-over script block (the text the helper will voice).
  - `/shot`: a reference to a media item with an in/out range and notes.
  - `/music`: music brief (mood, tempo, length, generated or file).
  - `/cta`: call to action.
- **Spec templates** are chosen at New Project time and depend on the intent: marketing, product demo, tutorial, explainer, social clip, testimonial, event recap, internal update, app store preview, and blank.
- **Spec outline** in the sidebar, generated from headings and scene blocks.
- **Lint** (live, non-blocking): unresolved `media:` IDs, scenes without media or narration, voice-over longer than the target duration at the chosen reading speed, and placeholder text.

---

## 6. Media references

### 6.1 Adding media
- Drag and drop onto the Media panel or the editor, File › Add Media…, or a folder (added recursively, with a filter by kind).
- **Nothing is copied.** Each media item stores:
  - `id`: stable, short, human-readable, for example `m-intro-take2`.
  - `kind`: video, audio, image, font, transcript, tape, demo script or other.
  - `role` (optional hint for Claude): footage, B-roll, screen recording, logo, music, SFX, voice or reference.
  - `paths`: the absolute path, the path relative to the project file, and a platform bookmark (base64) when available.
  - `fingerprint`: size, modification time, and a fast content hash (the first, middle and last 1 MiB plus the size).
  - Probed metadata from ffprobe: duration, resolution, fps, codecs, audio channels and loudness, cached.
  - Optional user notes and tags.

### 6.2 Resolution order
1. Bookmark (macOS), so the file is found after a rename or move on the same volume.
2. The relative path from the project file's folder.
3. The absolute path.
4. **Search roots**: folders the user marked as media roots (per project and app-wide), matched by file name and fingerprint.

A file found by steps 1, 2 or 4 whose path differs from the stored one is updated silently and the project is marked dirty. A file whose fingerprint differs is shown as **Changed**, with Accept and Relink actions.

### 6.3 Relinking (the "moved to a new machine" case)
- When a project opens, missing items are listed in a **Relink** sheet:
  - **Locate…** for one item, using a file picker.
  - **Search folder…**: the app scans the chosen folder and matches every missing item by name, then fingerprint.
  - **Apply prefix fix**: when one item is located, the app offers to rewrite the same old prefix for every other missing item (for example `/Users/a/Footage/…` becomes `/Volumes/Work/Footage/…`).
- Status badges stay visible in the Media panel, the spec preview and the validation report until every item resolves.
- The same functions are available from the CLI (`video-studio relink`, §13).

---

## 7. Intent

- **Intent** is a required project field: marketing, product demo, tutorial or how-to, explainer, social clip, testimonial, announcement, event recap, internal communication, app store preview, or other (free text).
- Optional fields: **audience**, **tone** (for example confident, friendly, technical or playful), **target length**, **primary call to action**, and **language and locales**.
- Intent sets defaults: the spec template, suggested output configs, cutline `init --style` (for example `business` for product demos and marketing), caption style, pacing, and music level. It's also passed to Claude in both the spec and production contexts.

---

## 8. Brand presets

- Stored in the **app library** (`~/Library/Application Support/Syncfusion Video Studio/brands/`, one JSON file per preset, with assets referenced like media). Several presets are supported.
- A preset holds:
  - Name and description.
  - Logos: primary, horizontal, icon, light and dark variants.
  - A color palette (primary, secondary, accent, background, text) as hex values with roles.
  - Fonts: heading and body, as files or installed family names.
  - Optional intro and outro bumpers, a watermark position and opacity, and a lower-third style.
  - Caption style, a default ElevenLabs voice (the voice ID and its settings), and a music mood.
  - **Rules**: do's and don'ts in free text, given to Claude verbatim.
- **Switching**: the toolbar's brand pop-up changes the project's preset in one click. The project stores the preset ID **and a snapshot** of it, so projects stay portable and reproducible. If the library copy is newer than the snapshot, a banner offers Update to latest.
- Import and export as a `.cutbrand` file (JSON plus its assets zipped) for sharing between machines.
- Brand assets are checked during validation (§11) like any other media.

---

## 9. Output configs and run configs

### 9.1 Output config presets
Each preset maps to a cutline target plus export settings. They're editable, and users can duplicate a preset to make their own.

| Preset | cutline target / export | Notes |
|---|---|---|
| YouTube 1080p | `long_1080p` | chapters, end-screen safe area |
| YouTube 4K | `long_4k` | |
| YouTube Shorts | `shorts` | ≤ 180 s, safe zones |
| TikTok | `shorts` | platform safe zones, ≤ 10 min |
| Instagram Reels | `shorts` | ≤ 90 s preset limit |
| Instagram Feed | `portrait_4x5` or `square` | |
| LinkedIn | `portrait_4x5` / `long_1080p` / `square` | captions on by default (feeds autoplay muted) |
| X / Twitter | `long_1080p` or `square` | size cap |
| Facebook | `long_1080p` / `square` / `portrait_4x5` | |
| Apple App Store preview | `custom` per device size (for example 886×1920, 1920×886, 1200×1600) | 15–30 s, 30 fps, no device frames, audio optional |
| Google Play promo | `long_1080p` | |
| Website hero loop | export `{audio: false, loop: true, max_bytes}` | WebM and MP4 |
| Custom | `custom` | free geometry, fps, codec, size cap |

Platform limits (duration, size, aspect, fps) are data in a bundled `platforms.json`, versioned and updatable, and validation checks them.

### 9.2 Run configs
- A project can hold several **run configs**, for example "Social pack" (Shorts + Reels + LinkedIn 4:5), "YouTube final" or "Website loop". A segmented control in the Run panel switches between them.
- A run config holds:
  - Its selected output configs.
  - The production model.
  - Draft or final quality.
  - Locales.
  - The **output folder**.
  - The ElevenLabs credit cap.
  - Whether to run the dry run first.
  - An optional maximum spend or turn count for Claude.
- **Output folders** are easy to set and change. Each run config has a folder picker with recent folders, and the app-wide default is `~/Movies/Syncfusion Video Studio/<project>/`. Runs write to `<output folder>/<yyyy-mm-dd-hhmm>-<run config>/`. Changing the folder never moves past runs, but run history keeps their locations and offers Locate if they move.

---

## 10. Workflow overview

```
New / open project ─▶ Write spec ◀─▶ Claude assist (edits, questions)
        │                  │
        ▼                  ▼
  Add media, intent, brand, run config
        │
        ▼
  Validate ── static checks ── (optional) dry run ─▶ report
        │  blocking issues must be fixed or explicitly waived
        ▼
  Submit production run ─▶ progress ─▶ outputs + QA + logs
```

---

## 11. Pre-submission validation

Validation runs on demand (⌘B, or the Validate button) and automatically before every production run. Results show in a **Validation report** with three severities: **Blocker**, **Warning** and **Info**. Each item links to the place to fix it. A run can't start with blockers, and warnings can be acknowledged per run.

### 11.1 Static checks (no model calls)
- **Environment**:
  - Claude Code found, signed in, with a version at or above the supported minimum.
  - `cutline` found and `cutline doctor` passing (required tools and filters, fonts, Chrome when HTML is used, whisper model when captions are on).
  - Free disk space in the output folder at least an estimate based on output configs and duration.
  - ElevenLabs key present (when the spec needs generated audio), with remaining credits fetched and compared with the estimate.
- **Media**:
  - Every referenced item resolves, is readable, and matches its fingerprint.
  - ffprobe succeeds; codecs are supported; there are no zero-length files.
  - VFR or rotation is noted as Info; very low resolution compared with the target is a Warning.
- **Spec**:
  - Not empty, and intent set.
  - Every `media:` ID resolves.
  - Voice-over length against the target duration at the chosen words-per-minute.
  - Required sections for the intent are present (for example a call to action for marketing).
  - No placeholder text left.
- **Brand**: assets resolve, fonts load, colors are valid, and logo contrast against the background is checked.
- **Outputs**:
  - At least one output config selected.
  - Durations and sizes are within platform limits.
  - Source aspect compared with each target's aspect (warns when reframing will be heavy).
  - The output folder is writable.
- **Budget**: estimated ElevenLabs characters and credits, and an estimated Claude cost range for the production model (labelled approximate, §22), both checked against the caps.

### 11.2 Dry run (optional, cheap model)
- A short Claude run with the dry run model and the same production prompt, in a scratch run folder. It:
  1. writes `project.json5` from the spec;
  2. runs `cutline validate` (machine-readable errors with JSON pointers), `cutline expand` and `cutline cues` where relevant;
  3. runs `cutline review` (contact sheets, cut pairs and QA, without a final render);
  4. returns a structured **readiness report**: ambiguities in the spec, assumptions it had to make, missing media, risky choices, and an estimated duration per output.
- The dry run can't call `cutline render` or the ElevenLabs helper. The permission broker enforces this.
- Findings are added to the Validation report as Warnings with links to the relevant spec lines. "Ask Claude to fix the spec" opens a spec-assistant session pre-filled with the findings.

---

## 12. Production runs

### 12.1 Submission
- **Run** (⌘R) validates first, then shows a confirmation sheet with the run config summary, the production model, outputs, estimated cost and the output folder.
- One production run per project at a time. Runs of different projects can run in parallel, up to a limit set in Settings (default 1, because renders are CPU-heavy).

### 12.2 What happens in a run
1. **Prepare**: create the run folder, write `run.json` (the resolved config), materialize `spec.md`, and write `media-manifest.json` (ID to resolved absolute path, kind, probe data) and `brand.json`.
2. **Launch Claude Code** with the production model in the run folder.
   - The system prompt includes: the cutline workflow (`cutline guide`), the manifest, brand rules, intent, the output configs translated to cutline `target`, `exports` and `variants`, the locales, and the budget.
   - Claude must write `project.json5`, use `cutline validate`, `review` and `render`, and review its own output before finishing.
3. **Permissions during the run** (enforced by the broker, not only by the prompt):
   - Read: the run folder, media folders in the manifest, brand assets, and the cutline home.
   - Write and Edit: the run folder only.
   - **Bash**: only commands whose executable resolves to the configured `cutline` binary or `video-studio eleven …`. Anything else is denied and logged.
   - No web access unless the run config enables it.
   - AskUserQuestion is allowed. The question appears as a notification and a card, the run waits, and an optional timeout makes Claude proceed on its stated assumption.
4. **Audio generation** goes through the helper (§14) as Claude needs it.
5. **Render**: Claude runs `cutline render`. Each export lands in `outputs/<output-config>/`.
6. **Finish**: Claude writes `summary.md` (what it made, its assumptions, QA results and known issues). The app collects outputs, QA reports, `.srt` files, thumbnails and chapters, and marks the run as Succeeded, Succeeded with warnings, Failed or Cancelled.

### 12.3 Progress
- The Run panel shows:
  - A **stage list** (Prepare, Audio, Authoring, Validate, Review, Render <output>…, QA, Done), derived from the tool calls in Claude's stream and from cutline's progress output.
  - The current stage's progress bar when cutline reports frames or segments.
  - Elapsed time, and an ETA when available.
  - Live cost: Claude usage from the stream, and ElevenLabs credits from the helper.
  - A collapsible live transcript of Claude's messages and tool calls.
- A Dock badge and a notification appear when a run finishes or needs an answer.

### 12.4 Cancellation
- **Stop** sends an `interrupt` control request to Claude. After a grace period (5 s) it kills the whole process tree (Claude, cutline, ffmpeg, Chrome) through `ProcessGroup`.
- Partial outputs are kept in place and flagged incomplete. The run's status becomes Cancelled, and its logs are complete up to the cancel point.

### 12.5 Results
- Each output gets a card: a player (AVFoundation), duration, size, QA verdict, Reveal in Finder, Open With…, and Copy path.
- **Re-run** repeats a run config. **Revise** opens a follow-up production session (`--resume` of the run's Claude session) with a feedback box, for changes such as "make the hook faster", and writes a new run folder that links to the previous one.

---

## 13. Headless CLI (`video-studio`)

The same core, without UI. It's suitable for scripts and CI.

```
video-studio validate  <project.cutproj> [--config <run-config>] [--dry-run] [--json]
video-studio run       <project.cutproj> --config <run-config> [--config …]
                         [--model <id|alias>] [--output <dir>] [--no-dry-run]
                         [--yes] [--json] [--log-level info|debug]
video-studio list      <project.cutproj>            # run configs, outputs, media status
video-studio relink    <project.cutproj> --search <dir> [--prefix <old>=<new>] [--write]
video-studio runs      <project.cutproj>            # history with paths and status
video-studio logs      <run-folder> [--zip <file>]
video-studio eleven    tts|music|sfx|scribe …       # helper used by Claude (§14)
```

- **Progress**: human-readable stage lines with a progress bar on a TTY. `--json` writes NDJSON events to stdout (`stage_started`, `progress`, `message`, `question`, `artifact`, `warning`, `stage_finished`, `run_finished`) using the same event schema as the UI.
- **Questions** in a headless run are prompted on a TTY. With `--yes` or without a TTY, the run proceeds on Claude's stated assumption, and the question is logged.
- **Cancellation**: Ctrl-C (SIGINT) triggers the same graceful stop as the UI. A second Ctrl-C kills immediately. Ctrl-Break on Windows does the same.
- **Exit codes**: 0 success, 1 success with warnings, 2 validation blockers, 3 run failed, 4 cancelled, 5 environment problem (Claude or cutline missing).
- The CLI can run while the app has the same project open. Run history is appended atomically, and the app picks up changes through file watching.

---

## 14. ElevenLabs helper (`video-studio eleven`)

- These subcommands call the ElevenLabs REST API. Claude invokes them through the restricted Bash permission:
  - `tts --voice <id> --text-file <f> --out <f> [--model] [--settings json]`
  - `music --prompt <p> --length <s> --out <f>`
  - `sfx --prompt <p> --length <s> --out <f>`
  - `scribe --in <audio> --out <json>` (cutline imports Scribe transcripts for captions and cues)
  - `voices [--search]`, to list voices
  - `quote <subcommand args>`, to estimate credits without generating
- **Key**: read from `Secrets`, never printed, and not passed through environment variables that Claude can see. The helper process receives its run context (run folder, budget, log path) through an environment variable that points to a file the app wrote. If the variable is missing, the helper refuses to run, so Claude can't use it outside a run.
- **Budget**: every call is quoted first. A call that would push the run past its credit cap fails with a clear message that Claude can act on, for example by shortening the text. The cap and the spend so far are visible in the UI.
- **Cache**: requests are keyed by a hash of the endpoint, parameters and text. A cache hit copies from the app cache instead of calling the API, so a re-run with an unchanged script costs nothing.
- **Outputs** go to `<run folder>/audio/`, and each is logged with its request, a response summary, credits, duration and file path.
- The brand preset's default voice is used unless the spec names another.

---

## 15. Logs

Every run (dry or production) produces a complete log set in `<run folder>/logs/`:

| File | Contents |
|---|---|
| `run.json` | resolved run config, app and core versions, Claude CLI version, model IDs, cutline version and `doctor` summary, OS, start and end times, status |
| `events.ndjson` | the app event stream (the same schema as `--json`) |
| `claude.jsonl` | raw Claude stream-json protocol, including permission requests and decisions |
| `cutline/<n>-<command>.log` | stdout and stderr of each cutline invocation, with argv, exit code and duration |
| `eleven.ndjson` | every helper call: request (minus the key), credits, cache hit or miss, output path |
| `validation.json` | the full validation report, including dry run findings |
| `qa/` | cutline QA reports and the review pack (copied or linked) |
| `summary.md` | Claude's end-of-run summary plus the app's status summary |

- **Access**: the run's Logs tab shows each file with search and filters by severity and source. Buttons: **Reveal**, **Export logs…** (a zip of the log folder plus `project.json5`, with optional media manifest redaction), and **Copy issue report**. The issue report is a Markdown summary of cutline errors, warnings and failed QA checks with their log excerpts, ready to paste into a cutline issue or a Claude session that fixes cutline itself.
- Logs are never pruned automatically. Settings › Storage shows their sizes and offers cleanup of runs older than N days.

---

## 16. Project file format (`.cutproj`)

JSON, UTF-8, LF line endings, keys in a stable order (so diffs are readable), and written atomically (to a temporary file, then renamed). A JSON Schema ships with the core, and the file carries a schema version so it can be migrated.

```jsonc
{
  "format": "video-studio-project",
  "version": 1,
  "id": "prj_01J…",                    // stable; a new ID on Duplicate
  "name": "Spring launch teaser",
  "created": "2026-11-14T10:00:00Z",
  "modified": "2026-11-14T12:30:00Z",
  "intent": { "kind": "marketing", "audience": "…", "tone": "confident",
              "target_length_s": 45, "cta": "Start a free trial", "locales": ["en"] },
  "spec": "# Spring launch teaser\n\n…markdown…",
  "media": [
    { "id": "m-hero", "kind": "video", "role": "footage",
      "paths": { "absolute": "/Users/…/hero.mov", "relative": "../footage/hero.mov",
                 "bookmark_macos": "base64…" },
      "fingerprint": { "size": 123, "mtime": "…", "quick_hash": "…" },
      "probe": { "duration_s": 32.1, "width": 3840, "height": 2160, "fps": "30000/1001" },
      "notes": "Use the second half" }
  ],
  "media_roots": ["../footage"],
  "brand": { "preset_id": "brd_acme", "snapshot": { /* full preset copy */ } },
  "models": { "assistant": "sonnet", "dry_run": "haiku" },
  "output_configs": [ { "id": "out_shorts", "preset": "youtube_shorts", "overrides": {} } ],
  "run_configs": [
    { "id": "rc_social", "name": "Social pack", "outputs": ["out_shorts"],
      "production_model": "opus", "quality": "final", "output_folder": "~/Movies/…",
      "eleven_credit_cap": 20000, "dry_run_first": true }
  ],
  "runs": [
    { "id": "run_…", "run_config": "rc_social", "kind": "production",
      "folder": "/Users/…/2026-11-14-1230-social-pack", "status": "succeeded",
      "started": "…", "finished": "…", "claude_session": "…" }
  ],
  "assistant_sessions": [ { "id": "…", "name": "…", "claude_session": "…" } ]
}
```

- **Autosave** runs one second after editing stops, and on window close and quit, as in Just Notes. If the file changes on disk (for example, the CLI appended a run), the app merges run history automatically. For any other external change while there are unsaved edits, a banner offers Reload, Keep Mine or Compare.
- **Duplicate / Clone** (File › Duplicate) copies the file and its `.assets` sidecar with a new `id`, clears `runs` and `assistant_sessions` (optional in the dialog), and rewrites relative paths for the new location.
- **Save As Template** removes media, runs and sessions but keeps the spec, intent, brand and configs.
- The spec is always plain Markdown inside the `spec` string. **File › Export Spec…** writes it as `.md` with `media:` links rewritten to file paths.

---

## 17. UI layout

**Main window (one per project):**

```
┌ Toolbar: [Brand ▾] [Run config ▾] [Validate] [Run ▶] [Stop ■]  status · cost ┐
├───────────┬───────────────────────────────────────────────┬─────────────────┤
│ Sidebar   │ Editor / Preview (spec)                       │ Inspector       │
│ • Outline │                                               │ Claude panel    │
│ • Media   │                                               │  or Validation  │
│ • Runs    │                                               │  or Run detail  │
└───────────┴───────────────────────────────────────────────┴─────────────────┘
```

- **Sidebar**: Outline (spec headings and scenes), Media (grouped by kind, with status badges, thumbnails and duration), Runs (history with status chips).
- **Inspector tabs**: Claude (assistant chat, questions, review), Project (intent, models), Outputs (output and run configs), Validation (report), Run (live progress, results, logs).
- **App library window** (⇧⌘B): brand presets and output config presets.
- **Visual design**:
  - Native macOS controls, SF Symbols, system fonts, and system accent color with an optional brand-tinted header.
  - Full light and dark mode.
  - Calm, dense-but-readable layouts like Final Cut Pro and Xcode inspectors, with no playful styling.
  - Every state (empty, loading, error) is designed.
  - Keyboard access to every action, VoiceOver labels and Dynamic Type where AppKit supports it.
- **Onboarding**: a first-run checklist covering Claude Code, cutline (with a `cutline deps install --all` button), the ElevenLabs key, a default brand preset, and the default output folder.

---

## 18. Settings

- **General**: default output folder, parallel run limit, media search roots, notifications.
- **Claude**: CLI path and Re-check, default assistant, dry run and production models, model list refresh, effort, web access default, question timeout in headless runs.
- **cutline**: binary path (default: `cutline` on PATH, then `bin/cutline` in a configured repo), `doctor` output, `deps` actions, default draft or final quality.
- **ElevenLabs**: API key (stored in the Keychain; shown masked, with a Test button), default voice, default credit cap per run, cache size and Clear.
- **Storage**: cache and log sizes, cleanup.

---

## 19. Security and privacy

- Anthropic credentials stay with the Claude CLI. The ElevenLabs key lives only in `Secrets` and is read only by the helper.
- The permission broker enforces scope for every Claude run (§4.3, §12.2). Denied requests are logged with reasons.
- Media is read in place. The app writes only the project file, the `.assets` sidecar for pasted images, the app library, caches, and run folders.
- Log export can redact absolute paths and file names in the media manifest.
- The app isn't sandboxed, because it spawns `claude`, `cutline` and `ffprobe`, as Just Notes does. A Developer ID build enables the hardened runtime.

---

## 20. Testing

- **Core (Rust)**:
  - Unit tests for project schema round trips and migrations, media resolution and relinking (prefix fix, fingerprint matching, moved files), validation rules, platform limit checks, run config to cutline exports mapping, the budget and cache logic, NDJSON event schema, and exit codes.
  - The permission broker is tested against recorded stream-json fixtures: allowed and denied Bash commands, writes outside the run folder.
- **Fakes**:
  - A fake `claude` binary that replays recorded streams, including questions, permission requests, cancellation and crashes.
  - A fake ElevenLabs server for the helper tests.
- **CLI**: end-to-end with the fakes (validate → run → logs), Ctrl-C cancellation, JSON event output, and exit codes.
- **macOS app**: an in-app self test, as in Just Notes, behind an environment variable. It covers spec editing, Claude edit review, the relink flow on a moved project, brand switching, a validation report with blockers, and a full run against the fake Claude and a real `cutline selftest --quick` asset set.
- **Optional live test**: one real end-to-end run with Haiku and a 10-second Shorts output, on demand only.

---

## 21. Milestones

| # | Scope | Acceptance |
|---|---|---|
| S0 | Workspace crates, UniFFI skeleton, Swift app shell, platform traits | app launches; core call round-trips; CI builds both |
| S1 | Project model, file I/O, media references and relinking, CLI `list` and `relink` | schema tests; relink flow tested on a moved fixture project |
| S2 | Spec editor port (Just Notes) with `media:` embeds, templates and lint | self test: edit, preview, Mermaid, media thumbnails |
| S3 | Claude assistant: sessions, model picker, questions, diff review | fake-Claude tests; per-hunk accept writes to `spec` |
| S4 | Intent, brand presets (library, snapshot, import and export), output and run configs | switching presets and configs; `platforms.json` limits |
| S5 | Static validation and report UI | every rule has a test; blockers stop runs |
| S6 | Run orchestrator, permission broker, progress, cancellation, logs; CLI `run`, `validate`, `logs` | fake-Claude end-to-end; Ctrl-C and Stop leave complete logs |
| S7 | ElevenLabs helper: budget, cache, logging | fake-server tests; cap enforcement; cache hits cost 0 |
| S8 | Dry run and readiness report | dry run can't render finals or call the helper |
| S9 | Results UI, Revise and Re-run, polish, accessibility, onboarding, packaging | in-app self test passes; live Haiku run succeeds |
| S10 (later) | Windows shell on `windows-rs` over the same core | — |

---

## 22. Implementation notes

There are no open product questions. The decisions from review are recorded in §0. These notes apply during implementation:

1. **Claude CLI version.** Verify the minimum supported version and the exact flags against the installed CLI at implementation time, as Just Notes did. This covers the permission prompt tool, `--effort`, `--resume`, and the cost and usage fields in the stream. Record the verified version and flags in `docs/decisions.md`. The app refuses to run with an older CLI and says which version is needed.
2. **Claude cost estimates.** The stream reports actual usage during and after a run. A pre-run estimate is a range based on spec size, media count, output count and past runs of the same project, and the UI always labels it approximate.
3. **ElevenLabs licensing.** Commercial use is confirmed (§0). The run's `summary.md` still includes cutline's publishing checklist (cutline spec §12.3), including the synthetic-content disclosure for platforms that require it.
