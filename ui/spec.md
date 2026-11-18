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
| Windows port | **Full port now** (§23). A Rust shell on **`windows-rs`** with the UI in **WebView2** (CodeMirror 6 editor and the Just Notes preview scripts), linking the core directly. Feature parity includes app recording (`video-studio capture`). Targets Windows 11 22H2 or later on x64 and ARM64, packaged as MSIX, plus a portable CLI zip. Tests: the Rust suites on Windows CI, an in-app self test, UI Automation tests and a live Haiku run. |

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
- Run on macOS and Windows with the same features, project files and run folders. Every platform-specific piece sits behind an interface (§3), and the Windows shell is specified in §23.

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
┌──── macOS app (Swift, AppKit/SwiftUI) ────┐ ┌── Windows app (Rust, windows-rs + WebView2) ──┐
│ Spec editor (Just Notes)  Media  Brand    │ │ CodeMirror editor  Media  Brand  Outputs      │
│ Outputs  Runs  Settings                   │ │ Runs  Settings  (web UI ⇄ JSON bridge)        │
└──────────────────────────┬────────────────┘ └──────────────────────┬────────────────────────┘
                           │ UniFFI bindings                         │ direct Rust calls
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
- **Platform traits** (each has a macOS and a Windows implementation; §23.4 specifies the Windows ones):
  - `Secrets`: Keychain on macOS, Credential Manager on Windows (the `keyring` crate).
  - `FileRefs`: security-scoped bookmarks on macOS, a volume serial plus a 128-bit file ID on Windows.
  - `ProcessGroup`: spawning and killing a process tree (a process group on macOS, a Job Object on Windows).
  - `Notifications`, `RevealInFileManager` and `OpenWith`.
  - `capture::Backend`: driving and recording apps during a run (`osascript` and `screencapture` on macOS, UI Automation, `SendInput` and Windows.Graphics.Capture on Windows).
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
1. Bookmark (macOS) or file ID (Windows), so the file is found after a rename or move on the same volume. Each platform ignores the other's entry and keeps it when saving.
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

Validation runs on demand (⇧⌘V, or the Validate button) and automatically before every production run. Results show in a **Validation report** with three severities: **Blocker**, **Warning** and **Info**. Each item links to the place to fix it. A run can't start with blockers, and warnings can be acknowledged per run.

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
1. **Prepare**: create the run folder, write `run.json` (the resolved config), materialize `spec.md`, and write `media-manifest.json` (ID to resolved absolute path, kind, probe data) `brand-preset.json` (the preset as entered) and `brand/default.json` (the same brand in cutline's brand format).
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
                 "bookmark_macos": "base64…",
                 "file_id_windows": "<volume serial>:<128-bit file id, hex>" },
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
- **App library window** (⇧⌘L): brand presets and output config presets.
- **Visual design**:
  - Native macOS controls, SF Symbols, system fonts, and system accent color with an optional brand-tinted header.
  - Full light and dark mode.
  - Calm, dense-but-readable layouts like Final Cut Pro and Xcode inspectors, with no playful styling.
  - Every state (empty, loading, error) is designed.
  - Keyboard access to every action, VoiceOver labels and Dynamic Type where AppKit supports it.
- **On Windows** the layout and panels are the same, with Fluent styling and Windows shortcuts (§23.6).
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
- The app isn't sandboxed, because it spawns `claude`, `cutline` and `ffprobe`, as Just Notes does. A Developer ID build enables the hardened runtime. On Windows, the MSIX package runs as a full-trust desktop app (§23.8).

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
- **Windows app**: the same suites run on Windows CI, plus a Windows in-app self test and UI Automation tests (§23.9).
- **Optional live test**: one real end-to-end run with Haiku and a 10-second Shorts output, on demand only, on each platform.

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
| S10 | Windows port: shell on `windows-rs` and WebView2 over the same core, at full parity (milestones W0–W8 in §23.10) | everything in §23.11 passes |

---

## 22. Implementation notes

There are no open product questions. The decisions from review are recorded in §0. These notes apply during implementation:

1. **Claude CLI version.** Verify the minimum supported version and the exact flags against the installed CLI at implementation time, as Just Notes did. This covers the permission prompt tool, `--effort`, `--resume`, and the cost and usage fields in the stream. Record the verified version and flags in `docs/decisions.md`. The app refuses to run with an older CLI and says which version is needed.
2. **Claude cost estimates.** The stream reports actual usage during and after a run. A pre-run estimate is a range based on spec size, media count, output count and past runs of the same project, and the UI always labels it approximate.
3. **ElevenLabs licensing.** Commercial use is confirmed (§0). The run's `summary.md` still includes cutline's publishing checklist (cutline spec §12.3), including the synthetic-content disclosure for platforms that require it.

---

## 23. Windows port

This section is the brief for Claude Code working on a Windows machine. The goal is a full port of the macOS app to Windows, with the same features, project files and run folders, verified by the test battery in §23.9. Where this section is silent, the macOS behavior in §1–§22 and `docs/video-studio/README.md` is the reference.

### 23.1 Ground rules
- **The macOS app keeps working.** Shared code changes keep `cargo test` green and leave every `#[cfg(target_os = "macos")]` path unchanged. Shared changes are kept small and listed in the final report, so they can be checked on a Mac (the CI macOS job, §23.9).
- **Windows code lives in Windows places.** That means `apps/windows/`, `#[cfg(windows)]` modules in the core (`platform/windows.rs`, `capture/windows.rs`) and Windows CI jobs. Nothing in the core calls a Windows API outside those modules.
- **Same files on both platforms.** A `.cutproj`, `.cutbrand`, `settings.json` or run folder made on one platform opens on the other. New Windows-only fields are optional, and each platform keeps the other's fields when it saves (§6.2, §16).
- **Verify before relying on it.** The Claude Code version and flags (§22.1), and how Claude Code runs Bash on Windows, are checked against the installed CLI. The results are recorded in `docs/decisions.md`.
- **Record decisions.** Every choice not fixed here, such as library versions or an API that didn't work, goes into `docs/decisions.md` under a "Windows port" heading.
- **Report honestly.** The final report lists each item in §23.11 with pass, fail or not run, and gives output for any failure.

### 23.2 Prerequisites on the Windows machine
- Windows 11 22H2 or later, with the WebView2 Evergreen runtime (built into Windows 11).
- The Rust stable MSVC toolchain, with targets `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`. Rust 1.80 or later, as for the rest of the workspace.
- Visual Studio Build Tools with the Windows 11 SDK (for `makeappx`, `signtool` and the ARM64 linker).
- Node.js LTS, to build the web UI.
- Git for Windows, which Claude Code needs on Windows for its shell tool. Claude Code 2.1.0 or later, signed in.
- cutline built for Windows with its dependencies installed (`cutline deps install --all`). `cargo test` and `cutline selftest --quick` pass for cutline itself before the app work starts.

### 23.3 Shell architecture
```
apps/windows/
  Cargo.toml          # crate video-studio-win (a workspace member), binary "Syncfusion Video Studio.exe"
  src/                # windows-rs: windows, menus, dialogs, taskbar, toasts, single instance, WebView2 host
  ui/                 # TypeScript + Vite: the whole window content
    editor/           # CodeMirror 6 spec editor
    preview/          # markdown-it, Mermaid 11, KaTeX, highlight.js (the versions Just Notes uses)
    panels/           # sidebar, inspector tabs, sheets, library window
  package/            # AppxManifest.xml template, assets, signing scripts
  uitests/            # UI Automation test runner (§23.9)
  build.ps1           # debug | release | selftest | package | uitest
```
- **Linking the core:** the shell links `video-studio-core` directly as a Rust dependency. UniFFI isn't needed on Windows. The surface the shell uses mirrors `video-studio-ffi`, so both shells call the same operations.
- **Native host (`src/`):** `windows-rs` handles the top-level window and the Win32 menu bar with accelerators. It also provides the common file dialogs (`IFileOpenDialog`), taskbar progress (`ITaskbarList3`), toast notifications, the recent-files jump list, and single-instance handling (a named mutex plus a named pipe that forwards file-open requests to the running instance). WebView2 is hosted with the `webview2-com` crate.
- **Web UI (`ui/`):** the window content is one WebView2 instance per window.
  - It is served from a virtual host (`SetVirtualHostNameToFolderMapping`, `https://app.video-studio.invalid/`), with a strict CSP and no remote content.
  - Media thumbnails and pasted images are served by a `WebResourceRequested` handler. It answers only for files that are in the project's media list or in the `.assets` sidecar.
  - DevTools and the default context menu are off in Release builds.
- **Bridge:** the UI and the host exchange typed JSON messages through `postMessage`/`WebMessageReceived`. Each message is a request with an id, a response, or an event. Rust types are the source of truth, and their TypeScript definitions are generated at build time (`ts-rs` or `specta`), so the two sides can't drift. Core events, such as run progress, Claude stream deltas and validation results, are pushed as events, and the UI never blocks on the host.
- **Editor parity with Just Notes (§5):** CodeMirror 6 with Markdown, source and preview toggle, slash commands (`/scene /vo /shot /music /cta` and the Just Notes set), table tools, image paste into the sidecar, `media:` drop and embed, find and replace, and lint markers. Claude's edits are reviewed per hunk with `@codemirror/merge`. The preview reuses Just Notes' preview scripts and CSS unchanged where possible, so a spec renders the same on both platforms.

### 23.4 Platform trait implementations (`platform/windows.rs`)
| Trait | Windows implementation |
|---|---|
| `Secrets` | Credential Manager through the `keyring` crate, service `Syncfusion Video Studio`, account `elevenlabs`. |
| `FileRefs` | `create` stores `<volume serial>:<file id>` from `GetFileInformationByHandleEx(FileIdInfo)`. `resolve` opens the file with `OpenFileById` on the volume with that serial and returns its current path (`GetFinalPathNameByHandleW`). This finds files after a rename or move on the same volume, like a macOS bookmark. |
| `ProcessGroup` | Each spawned tree (`claude`, `cutline`, `ffprobe`) is started with `CREATE_NEW_PROCESS_GROUP` and assigned to a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. This replaces the `taskkill` fallback. Graceful stop sends the stream-json `interrupt`, then `CTRL_BREAK_EVENT` to the group, waits 5 s, and then calls `TerminateJobObject` (§12.4). If the app exits or crashes, the whole tree goes with it. |
| `Notifications` | Toast notifications, using the package identity. |
| `RevealInFileManager` | `SHOpenFolderAndSelectItems`. |
| `OpenWith` | `ShellExecuteExW` (default app), and the "Open with" dialog via `SHOpenWithDialog`. |

### 23.5 Windows-specific behavior
- **Locations** (`paths.rs` already follows the first two):
  - The library (brands, `settings.json`) is in `%APPDATA%\Syncfusion Video Studio\`.
  - The default output root is the Videos known folder (`FOLDERID_Videos`) `\Syncfusion Video Studio\<project>\`. Resolve it through the known folder, not `%USERPROFILE%\Videos`, because it can be redirected.
  - Caches go in `%LOCALAPPDATA%\Syncfusion Video Studio\Cache\`.
- **Paths:**
  - The app manifest declares `longPathAware`, and every path API handles paths over 260 characters.
  - Paths are compared case-insensitively after normalizing separators. This matters most in the permission broker's scope checks (§4.3, §12.2): `C:\Runs\x` and `c:/runs/X/` are the same folder.
  - Drive letters, UNC paths (`\\server\share\…`) and mapped drives work in media references and in the relink prefix fix (§6.3). For example, `/Volumes/Footage` can be rewritten to `D:\Footage`.
  - Generated file names avoid reserved names (`CON`, `NUL`, `COM1`…) and trailing dots or spaces. `project.rs` already replaces invalid characters.
- **Line endings:** files are written with LF (§3), and input with CRLF is accepted everywhere, including the spec, Scribe JSON and `.tape` files.
- **Claude Code detection (§4.1):** in order:
  1. The configured path.
  2. `claude.exe` or `claude.cmd` on `PATH`.
  3. `%USERPROFILE%\.local\bin\claude.exe`.
  4. `%APPDATA%\npm\claude.cmd`.

  A `.cmd` shim is resolved to the real executable or Node script before spawning, so arguments aren't re-parsed by `cmd.exe`.
- **Bash commands during runs:** Claude Code runs Bash commands through Git Bash on Windows, so the broker and the pre-tool hook parse commands as that shell sees them.
  - An allowed executable matches `cutline`, `cutline.exe`, `video-studio` and `video-studio.exe`, given as a bare name, a POSIX path (`/c/…`) or a Windows path.
  - Anything else is denied, as on macOS. This includes `cmd /c`, `powershell` and `start`.
  - The exact behavior is verified against the installed CLI and covered by broker fixtures recorded on Windows.
- **CLI:**
  - Ctrl-C and Ctrl-Break both trigger the graceful stop, and a second press kills (§13).
  - Exit codes and NDJSON events are identical on both platforms.
  - Progress output is plain text when the console doesn't support virtual terminal sequences.
- **cutline:** found as the configured path, then `cutline.exe` on `PATH`. The MSIX doesn't bundle cutline. Onboarding offers to locate it and runs `cutline deps install --all`, as on macOS.

### 23.6 Look and feel
- **Fluent design:**
  - The Mica backdrop (`DWMWA_SYSTEMBACKDROP_TYPE`) and Segoe UI Variable.
  - Fluent UI web components for controls, the Windows accent color, and Segoe Fluent Icons in place of SF Symbols.
  - The same layout as §17: toolbar, sidebar, editor and preview, inspector tabs, and the App Library window.
- **Themes and scaling:** follows the system light or dark theme, supports high-contrast themes, and is per-monitor DPI aware (v2). The window is checked at 100 %, 150 % and 200 % scaling.
- **Accessibility:** every control has an accessible name and role, which WebView2 exposes through UI Automation. That makes them usable with Narrator, and it's what the UI tests use. Every action can be reached from the keyboard, and the focus order is logical.
- **Shortcuts:** these replace the macOS ones and follow Windows conventions:

  | Action | macOS | Windows |
  |---|---|---|
  | Toggle source/preview | ⌘/ | Ctrl+/ |
  | Validate | ⇧⌘V | Ctrl+Shift+B |
  | Run | ⌘R | F5 |
  | Stop | ⌘. | Shift+F5 |
  | App Library | ⇧⌘L | Ctrl+Shift+L |
  | Send to Claude | ⌘↩ | Ctrl+Enter |
  | Settings | ⌘, | Ctrl+, |

- **File types:** `.cutproj` and `.cutbrand` are registered, with icons, through the package manifest. Double-clicking opens the file in the running instance.

### 23.7 App recording on Windows (`capture/windows.rs`)
The `video-studio capture` commands and their checks behave as on macOS (`docs/video-studio/README.md`, "Recording apps during a run"). The Windows backend implements `capture::Backend` as follows:
- **Finding and opening apps:**
  - Apps are opened by AUMID, by executable name, or as an http(s) address in the default browser (`ShellExecuteExW` or `IApplicationActivationManager`).
  - Windows are found with `EnumWindows`, filtered by process.
  - Windows are focused and raised through UI Automation, with `SetForegroundWindow` as a fallback.
- **Input:**
  - `click`, `move`, `drag`, `scroll`, `type` and `key` use `SendInput`. Coordinates are relative to the target window and DPI-aware.
  - `menu` uses UI Automation's ExpandCollapse and Invoke patterns on the app's menu bar.
- **Stills and recording:**
  - Windows.Graphics.Capture records the target window (a `GraphicsCaptureItem` for its HWND). Media Foundation encodes it to H.264 MP4.
  - App audio comes from WASAPI process loopback, so only the target app's sound is captured.
  - The capture border is turned off where the OS allows it (`IsBorderRequired`).
  - `record start --seconds <n>` keeps fixed-length semantics, for parity with macOS. Windows recordings can also be stopped early without losing the file.
- **Guard rails:**
  - The backend acts only on the target app's windows.
  - It refuses terminals (`cmd`, `powershell`, `pwsh`, Windows Terminal), Settings, Registry Editor, Task Manager, password managers, File Explorer and UAC prompts.
  - It writes only under `captures/`.
  - It reports a clear error when the target runs elevated, because Windows blocks input to elevated windows from a normal-integrity app.
- **Permissions:** Windows has no equivalent of macOS's Screen Recording and Accessibility prompts. The Project inspector instead shows whether graphics capture is supported and whether the target is elevated.
- **Tests:** the fake backend (`VIDEO_STUDIO_CAPTURE_FAKE`) as on macOS, plus real-backend tests against `vs-capture-target.exe`. That is a small Win32 test app built with the tests, with a button, a text box, a menu and a tone generator.

### 23.8 Packaging and distribution
- **MSIX:**
  - One package per architecture (x64, ARM64), combined into an `.msixbundle`. It runs as a full-trust desktop app (`runFullTrust`).
  - It includes the app, `video-studio.exe`, and an app execution alias so `video-studio` works from any terminal.
  - It also declares the file type associations and toast activation.
- **Shared files:** file-system write virtualization is turned off for the app's folders (`desktop6:FileSystemWriteVirtualization` with the `unvirtualizedResources` capability). That way the app, the packaged CLI and the portable CLI read and write the same `%APPDATA%` and `%LOCALAPPDATA%` files. Verify this on a clean machine and record the result.
- **Signing:**
  - Release builds are signed with the Syncfusion code-signing certificate, through `signtool`, with the certificate taken from the environment and never committed.
  - `build.ps1 package` also makes a self-signed test certificate for local installs, and prints how to trust it.
- **Portable CLI:** `video-studio-<version>-windows-<arch>.zip` contains `video-studio.exe` and the README section on the CLI, for CI and headless machines.
- **WebView2:** at start-up the app checks for the runtime. If it's missing, the app explains how to install it instead of crashing.

### 23.9 Test battery
Every item below runs on the Windows machine, and items 1–4 also run in CI.
1. **Rust suites on Windows:**
   - `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` for `video-studio-core`, `video-studio-cli`, `video-studio-ffi` and `video-studio-win`, on x64 and ARM64. The cutline workspace tests also pass on Windows.
   - The end-to-end tests use `vs-fake-claude.exe`, `vs-fake-cutline.exe` and the fake ElevenLabs server, as on macOS.
   - New Windows tests cover:
     - Job Object tree kill, including grandchildren; Ctrl-Break graceful stop; and Ctrl-C twice.
     - `FileRefs` after rename and move, and fallback across volumes.
     - Long paths, UNC paths, drive-letter prefix relink, and case-insensitive broker scope.
     - Git Bash command forms in the broker and hook, using fixtures recorded from the real CLI on Windows.
     - CRLF inputs.
     - A Credential Manager round trip, skipped with a message when no interactive session is available.
     - `.cmd` shim resolution.
     - Opening a project made on macOS and saving it without losing `bookmark_macos`.
2. **CI:** GitHub Actions jobs on Windows x64 and Windows ARM64 runners run item 1, build the MSIX bundle and keep it as an artifact. The existing macOS job keeps running, so shared changes are checked on both platforms.
3. **In-app self test** (`build.ps1 selftest`, or `VIDEO_STUDIO_SELFTEST=1`): the Windows equivalent of the Mac self test (§20). It drives the real app with the fake Claude and the real cutline over `cutline selftest --quick` assets. It covers:
   - Spec editing, preview, and a rendered Mermaid diagram.
   - Claude edit review with per-hunk accept and reject.
   - The relink flow on a moved project.
   - Brand switching, and a validation report with blockers.
   - A full run, Stop during a run, and log export.

   It saves window snapshots (WebView2 `CapturePreview`) and a log next to each other, and exits non-zero on any failure.
4. **UI Automation tests** (`build.ps1 uitest`): a runner in `apps/windows/uitests` uses `IUIAutomation` (windows-rs) against the installed MSIX. It covers:
   - Launch and onboarding.
   - Creating a project, adding media by drag and drop, and opening by double-click (file association).
   - Editing the spec, slash commands, image paste, and Claude diff review.
   - Relink with Locate and Search Folder.
   - The App Library: create, import and export brand presets.
   - Validate, Run and Stop with the fake Claude, and the run inspector updates.
   - Exporting logs.
   - Every §23.6 shortcut.
   - A keyboard-only pass, plus checks that every control has an accessible name.
   - High contrast and dark theme.
   - Screenshots at 100 %, 150 % and 200 % scaling.
5. **App recording tests:** `video-studio capture` against `vs-capture-target.exe` covers open, window, screenshot, click, type, key, menu, scroll, drag, wait, and a 5-second recording. ffprobe then checks the recording's duration, size and audio stream, and the guard rails refuse a terminal and an elevated window.
6. **Live Haiku run** (`scripts/video-studio-live-test.ps1`, on demand): one real end-to-end run on Haiku with a 10-second Shorts output, using the installed MSIX and the real cutline. It costs a few cents. It checks the output file, the QA JSON and complete logs, then runs the CLI `video-studio run --json` on the same project.

### 23.10 Work order
| # | Scope | Done when |
|---|---|---|
| W0 | Toolchains, workspace builds on Windows, cutline tests and selftest pass, Windows CI jobs | `cargo test` for the existing crates is green on Windows x64 and ARM64 |
| W1 | `platform/windows.rs`: Secrets, FileRefs, ProcessGroup (Job Object), Notifications, Reveal, OpenWith; Windows paths, Claude detection, Git Bash broker rules | §23.9 item 1 Windows tests pass |
| W2 | `video-studio-win` shell: window, menus, single instance, WebView2 host, typed bridge, file dialogs, taskbar progress | app opens a `.cutproj` and shows its spec read-only |
| W3 | Web UI: CodeMirror editor, preview, sidebar, inspector, App Library, Settings, onboarding | editing, preview and Mermaid parity with the Mac app |
| W4 | Claude assistant, diff review, questions; validation report | fake-Claude flows work in the app |
| W5 | Runs: confirmation, progress, Stop, results, Revise and Re-run, logs, toasts | full fake-Claude run and Stop from the UI |
| W6 | App recording backend and its tests | §23.9 item 5 passes |
| W7 | MSIX packaging, signing, execution alias, file associations, portable CLI zip | bundle installs on a clean Windows 11 x64 and ARM64 machine |
| W8 | In-app self test, UI Automation tests, live Haiku run, docs | everything in §23.11 passes |

### 23.11 Definition of done
- Every §23.9 item passes on Windows 11 x64. Items 1–5 also pass on ARM64.
- The macOS CI job is still green, and the macOS self test still passes. The final report says which shared changes it touched.
- A project, brand preset and run folder made on macOS open correctly on Windows, and the reverse.
- `docs/video-studio/README.md` has Windows sections covering requirements, building with `build.ps1`, locations, shortcuts, app recording and tests. Its "Porting to Windows" section is replaced by them.
- `docs/decisions.md` records the Windows decisions, including the verified Claude Code version, Bash behavior and MSIX virtualization result.
