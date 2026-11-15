# Syncfusion Video Studio

A desktop app and a command-line tool for preparing videos with Claude Code and cutline. You describe the video in a Markdown spec, refine it with Claude, attach media by reference, choose a brand preset and outputs, validate, then submit a run in which Claude Code drives `cutline` to render the outputs.

The specification is `ui/spec.md`. Decisions made while building it are in `docs/decisions.md` (2026-11-14, Syncfusion Video Studio).

## Parts

| Path | What it is |
|---|---|
| `crates/video-studio-core` | The shared Rust core: `.cutproj` model, media references and relinking, brand and output presets, validation, the Claude Code client and permission broker, the run orchestrator, the ElevenLabs helper, logs |
| `crates/video-studio-cli` | The `video-studio` binary: the headless CLI, the helper Claude uses during runs, the pre-tool hook and the cutline logging proxy |
| `crates/video-studio-ffi` | UniFFI bindings for the shells |
| `apps/macos` | The macOS app (Swift, AppKit and SwiftUI, built with XcodeGen) |
| `schema/cutproj.schema.json` | JSON Schema of the project file (`video-studio schema`) |

## Requirements

- Claude Code 2.1.0 or later, signed in (`claude` in Terminal). The app never asks for Anthropic credentials.
- cutline, with its dependencies installed (`cutline deps install --all`).
- An ElevenLabs API key, only if a spec needs generated voice-over or music.
- To build the app: Xcode with the macOS 26 SDK, XcodeGen (`brew install xcodegen`), and a Rust toolchain.

## Building

```bash
cargo build --release -p video-studio-cli     # the CLI alone
apps/macos/build.sh                           # the app (Release)
apps/macos/build.sh debug                     # Debug, with the self test and test doubles
apps/macos/build.sh install                   # Release, copied to /Applications
```

`build.sh` builds the core, generates the Swift bindings into `apps/macos/Generated/`, generates the Xcode project and builds the app. The bundle carries the `video-studio` helper in `Contents/Helpers/`.

## The app

One window per project: a sidebar (Outline, Media, Runs), the spec in the middle (source or preview, ⌘/), and an inspector (Claude, Project, Outputs, Validation, Run). The toolbar has the brand preset, the run config, Validate (⌘B), Run (⌘R) and Stop (⌘.). The App Library (⇧⌘B) holds brand presets and output presets.

- **Spec.** Type `/` for commands; `/scene`, `/vo`, `/shot`, `/music` and `/cta` insert the structured blocks. Drop a file on the editor to reference it as `![name](media:<id>)`. A pasted image is stored in `<name>.cutproj.assets/` next to the project.
- **Claude.** Claude edits a copy of the spec; each change is shown as hunks to accept or reject. "Gather missing info" has Claude interview you about gaps.
- **Media.** Files are used where they are. When they move, the app follows them through bookmarks, relative paths, absolute paths and search folders, and lists what is still missing in the Relink sheet (Locate, Search Folder, prefix fix).
- **Runs.** Run validates first and shows what will happen and roughly what it costs. A dry run with a cheaper model can go first. Progress, cost, questions from Claude, results and logs are in the Run inspector.

Projects autosave one second after editing stops. Settings shared with the CLI are in `~/Library/Application Support/Syncfusion Video Studio/settings.json`.

## The CLI

```
video-studio validate  <project.cutproj> [--config <run-config>] [--dry-run] [--json]
video-studio run       <project.cutproj> --config <run-config> [--config …]
                         [--model <id|alias>] [--output <dir>] [--no-dry-run]
                         [--yes] [--json] [--log-level info|debug]
video-studio list      <project.cutproj> [--json]
video-studio relink    <project.cutproj> [--search <dir>] [--prefix <old>=<new>] [--write] [--json]
video-studio runs      <project.cutproj> [--json]
video-studio logs      <run-folder> [--zip <file>] [--redact] [--issue-report]
video-studio new       <project.cutproj> [--name …] [--intent …] [--media <file-or-folder>]…
video-studio schema
video-studio secret    set|status|test|delete        # the ElevenLabs key; `set` reads it from stdin
video-studio eleven    tts|music|sfx|scribe|voices|quote …   # used by Claude inside a run
video-studio capture   check|open|window|screenshot|click|move|drag|scroll|type|key|menu|wait|record …   # used by Claude inside a run
video-studio standin   video --out <file> --seconds <n> | audio --out <file> (--seconds <n> | --text-file <script>)   # dry runs only: holds the place of a file the run will make
```

- `run` validates, does the dry run when the run config asks for one, then the production run. Ctrl-C stops gracefully; a second Ctrl-C kills immediately.
- `--json` writes NDJSON events to stdout, the same schema as `logs/events.ndjson`: `run_started`, `stage_started`, `progress`, `message`, `question`, `question_answered`, `artifact`, `warning`, `cost`, `stage_finished`, `run_finished`.
- Without a terminal, or with `--yes`, questions from Claude are not asked: Claude proceeds on its stated assumption and the question is logged. Dry run findings stop a non-interactive `run` unless `--yes` is given.
- Exit codes: 0 success, 1 success with warnings, 2 validation blockers, 3 run failed, 4 cancelled, 5 environment problem (Claude Code or cutline missing).

## A run folder

```
<output folder>/<yyyy-mm-dd-hhmm>-<run config>/
  run.json  spec.md  media-manifest.json  brand-preset.json  brand/default.json  outputs.json   # written by the app
  project.json5  summary.md  audio/  work/  review/                  # written by Claude
  outputs/<output>/<name>.mp4 (+ .srt .vtt .qa.json .poster.jpg)
  logs/run.json  events.ndjson  claude.jsonl  eleven.ndjson  validation.json
       permissions.ndjson  summary.md  cutline/<n>-<command>.log  qa/
```

Logs are never removed automatically. `video-studio logs <folder> --issue-report` prints a Markdown summary of cutline errors, warnings, failed QA checks and blocked tool calls, ready for a cutline issue.

## What Claude may do in a run

Enforced by the permission broker, not only by the prompt:

- Read: the run folder, the folders of media in the manifest, brand assets, cutline's home.
- Write: the run folder only (not `logs/`, `run.json` or the manifest).
- Shell: `cutline …`, `video-studio eleven …`, `video-studio capture …` and (dry runs only) `video-studio standin …`, one plain command at a time.
- Web: only when the run config allows it.
- In a dry run, no `cutline render`, no audio generation and no driving or recording of apps.

## Recording apps during a run

When a spec asks for footage of an app that isn't attached (a `- **Record:**` line naming a desktop app or a web address, with the steps to perform), the production run makes it. Claude Code opens the app, looks at screenshots, clicks, types and uses menus, and records the window, all through `video-studio capture`. cutline is not involved; it only edits the recordings, which land in the run folder's `captures/`. The spec assistant knows this and writes such scenes instead of saying the footage can't be captured.

- Project › App recording turns this on or off (on by default) and can limit it to named apps. With no list, any app can be driven except terminals, System Settings, password managers and Finder.
- Every action is checked by the helper: it acts only on the app it was pointed at, opens only app names and http(s) addresses, and writes only under `captures/`. `osascript`, `screencapture` and `open` themselves stay blocked.
- Recordings have a fixed length (`record start --seconds <n>`, then `record wait`), because the system recorder can't be stopped early without losing the file. The edit trims them.
- macOS must allow **Screen & System Audio Recording** and **Accessibility** for Syncfusion Video Studio (or for the terminal, when using the CLI). The Project inspector shows both and links to the settings. The app is signed ad hoc, so a rebuilt app may need the permissions granted again.
- The pointer and keyboard are shared with whoever is at the Mac while a run drives an app.
- macOS only for now. The platform part is one trait (`capture::Backend`), with a fake for tests (`VIDEO_STUDIO_CAPTURE_FAKE`).

## Tests

```bash
cargo test -p video-studio-core -p video-studio-cli -p video-studio-ffi
apps/macos/build.sh selftest
scripts/video-studio-live-test.sh        # optional: one real run on Haiku, costs a few cents
```

The Rust end-to-end tests use `vs-fake-claude`, which speaks the stream-json protocol and replays a script (its tool calls go through the hook and the permission prompt as the real CLI's do), `vs-fake-cutline`, and a fake ElevenLabs server. The in-app self test drives the real app with the fake Claude and the real cutline over `cutline selftest --quick` assets, and saves window snapshots next to its log.

Environment variables for tests and unusual setups: `VIDEO_STUDIO_HOME` (library folder), `VIDEO_STUDIO_CLAUDE` and `VIDEO_STUDIO_CUTLINE` (binary paths), `VIDEO_STUDIO_NO_CLAUDE` (pretend the CLI is missing), `VIDEO_STUDIO_ELEVEN_BASE_URL`.

## Porting to Windows

Everything outside `apps/macos` is platform-neutral: paths are `camino` UTF-8 paths, files are written with LF, and OS-specific code sits behind the traits in `platform.rs` (`Secrets`, `FileRefs`, `ProcessGroup`, `Notifications`, `RevealInFileManager`, `OpenWith`). A Windows shell built on `windows-rs` links `video-studio-ffi` (or the core directly), supplies `FileRefs`, and replaces the `taskkill` fallback in `ProcessGroup` with a Job Object.

## What a dry run checks, and what it leaves to production

A dry run records and generates nothing, so its edit points at recordings and audio that don't exist yet. To keep cutline from stopping at the missing files, Claude writes a stand-in for each one with `video-studio standin` (a flat-colored clip or a quiet sound of the planned length) and then runs `cutline validate`, `expand` and `review` on the whole edit. That covers timing, text layout, captions against the watermark and each output's framing.

- Files the run makes itself are listed under `planned` in `readiness.json` and shown as "Made during the production run". They are not missing media and don't count against the report.
- `missing_media` is only for media the user has to supply.
- Zooms onto parts of an app can only be measured from the real recording, so the dry run leaves them out.
- A voice-over stand-in made with `--text-file` is as long as its words take to speak and comes with a stand-in transcript (`audio/<name>.json`, word timings spread over that length). The asset gets `script` and `transcript` as it will in production, so captions and cue timing are checked too.
- `cutline review` shows the first export only. For each other export with a different shape, the dry run reviews a scratch copy (`check-<output>.json5`) with only that export.

Validation (before any run) also checks what a run would otherwise find the expensive way: no voice chosen for the voice-over, a spec that asks for app recordings while App recording is off or the computer doesn't allow it, voice-over that doesn't fit its scene, and burned-in captions asked for on an output that delivers them another way.

## The brand in a run folder

The app writes the brand twice: `brand-preset.json` is the preset as the user entered it (rules, voice, music mood, styles in their own words), for Claude to read. `brand/default.json` is the same brand in cutline's brand format, which cutline loads without being told. cutline draws text only with its bundled fonts (Anton, Bebas Neue, Inter, Poppins); another brand font becomes Inter and Claude is told to say so.
