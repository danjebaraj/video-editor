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
```

- `run` validates, does the dry run when the run config asks for one, then the production run. Ctrl-C stops gracefully; a second Ctrl-C kills immediately.
- `--json` writes NDJSON events to stdout, the same schema as `logs/events.ndjson`: `run_started`, `stage_started`, `progress`, `message`, `question`, `question_answered`, `artifact`, `warning`, `cost`, `stage_finished`, `run_finished`.
- Without a terminal, or with `--yes`, questions from Claude are not asked: Claude proceeds on its stated assumption and the question is logged. Dry run findings stop a non-interactive `run` unless `--yes` is given.
- Exit codes: 0 success, 1 success with warnings, 2 validation blockers, 3 run failed, 4 cancelled, 5 environment problem (Claude Code or cutline missing).

## A run folder

```
<output folder>/<yyyy-mm-dd-hhmm>-<run config>/
  run.json  spec.md  media-manifest.json  brand.json  outputs.json   # written by the app
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
- Shell: `cutline …` and `video-studio eleven …` only, one plain command at a time.
- Web: only when the run config allows it.
- In a dry run, no `cutline render` and no audio generation.

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
