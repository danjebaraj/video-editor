# Syncfusion Video Studio

A desktop app and a command-line tool for preparing videos with Claude Code and cutline. You describe the video in a Markdown spec, refine it with Claude, attach media by reference, choose a brand preset and outputs, validate, then submit a run in which Claude Code drives `cutline` to render the outputs.

**Using the app:** see the [User Guide](USER_GUIDE.md), which covers the workflow, writing specs, brand and output presets, runs and every setting. This README is about building and the CLI.

The specification is `ui/spec.md`. Decisions made while building it are in `docs/decisions.md` (2026-11-14, Syncfusion Video Studio).

## Parts

| Path | What it is |
|---|---|
| `crates/video-studio-core` | The shared Rust core: `.cutproj` model, media references and relinking, brand and output presets, validation, the Claude Code client and permission broker, the run orchestrator, the ElevenLabs helper, logs |
| `crates/video-studio-cli` | The `video-studio` binary: the headless CLI, the helper Claude uses during runs, the pre-tool hook and the cutline logging proxy |
| `crates/video-studio-ffi` | UniFFI bindings for the shells |
| `apps/macos` | The macOS app (Swift, AppKit and SwiftUI, built with XcodeGen) |
| `apps/windows` | The Windows app (Rust on `windows-rs`, UI in WebView2), its MSIX packaging and UI Automation tests; see [Windows](#windows) |
| `schema/cutproj.schema.json` | JSON Schema of the project file (`video-studio schema`) |

## Requirements

- Claude Code 2.1.0 or later, signed in (`claude` in Terminal). The app never asks for Anthropic credentials.
- cutline, with its dependencies installed (`cutline deps install --all`).
- An ElevenLabs API key, only if a spec needs generated voice-over or music.
- To build the app: Xcode with the macOS 26 SDK, XcodeGen (`brew install xcodegen`), and a Rust toolchain. For Windows, see [Requirements (Windows)](#requirements-windows).

## Building

```bash
cargo build --release -p video-studio-cli     # the CLI alone
apps/macos/build.sh                           # the app (Release)
apps/macos/build.sh debug                     # Debug, with the self test and test doubles
apps/macos/build.sh install                   # Release, copied to /Applications
apps/macos/build.sh signing                   # fill in Signing.local.xcconfig from your certificates
apps/macos/build.sh distribute                # Developer ID, notarized and stapled zip in build/dist/
apps/macos/build.sh docshots                  # retake the screenshots in USER_GUIDE.md (images/)
```

`build.sh` builds the core, generates the Swift bindings into `apps/macos/Generated/`, generates the Xcode project and builds the app. The bundle carries the `video-studio` helper in `Contents/Helpers/`.

### User guide screenshots

The pictures in [USER_GUIDE.md](USER_GUIDE.md) (`images/`) are taken from the real app, not drawn. After a change to the UI, `apps/macos/build.sh docshots` retakes them all: it builds the Debug app, makes sample media for a made-up product ("Acme Diagrams") in `/Users/Shared/Demo`, and runs the app in a screenshot mode (`Sources/VideoStudio/App/DocShots.swift`) that builds a polished demo project, talks to the scripted Claude, runs the real cutline, and captures each window with ScreenCaptureKit. The app opens and closes its windows on screen for about a minute. Allow Screen Recording for the app you run it from (Terminal, say) or the pictures fall back to a plainer drawing without sheets or video. It uses its own library and a stand-in API key, and restores your recent projects. Then `npm run deploy` in `site/` publishes them with the guide.

### Signing (macOS)

By default the app is signed ad hoc, which needs no certificate. macOS ties the recording permissions to the signature, and an ad-hoc signature changes with every build, so the permissions must be allowed again after each one.

To keep them across builds, sign with your Apple developer certificates:

1. In Xcode → Settings → Accounts, add your Apple ID. Then select the team → **Manage Certificates…** → **+** → **Apple Development**. To share the app with others, also add **Developer ID Application** (only the team's Account Holder can create it; anyone else imports a `.p12` exported for them).
2. Run `apps/macos/build.sh signing`. It reads the certificates from the keychain and writes `apps/macos/Signing.local.xcconfig` (not in source control, so each machine uses its own; the previous file is kept as `.bak`). Everyday builds use the Apple Development certificate, or the Developer ID one if that is the only one. `DEVELOPMENT_TEAM` comes from the certificate. To edit the file by hand, start from `Signing.local.xcconfig.example`.
3. Run `apps/macos/build.sh` or `install` as before. The app, `MarkdownCore` and the helpers are signed with the certificate in every `build.sh` mode and when building from Xcode.

#### Distributing (Developer ID + notarization)

`apps/macos/build.sh distribute` builds Release into `build/DD-dist`:

- Signs with `VS_DISTRIBUTION_IDENTITY`, with the hardened runtime and secure timestamps (helpers included) and without `get-task-allow`. The app's only entitlement is `com.apple.security.automation.apple-events` (`Resources/VideoStudio.entitlements`), which lets production runs operate other apps.
- Verifies the signature, then submits it to Apple's notary service with the `notarytool` keychain profile `VS_NOTARY_PROFILE` (default `videostudio-notary`).
- Staples the ticket, checks it with Gatekeeper, and writes `apps/macos/build/dist/Syncfusion-Video-Studio-<version>.zip`.

Create the notary profile once. It asks for an app-specific password (account.apple.com → Sign-In and Security → App-Specific Passwords) and keeps it in the keychain:

```bash
xcrun notarytool store-credentials videostudio-notary --apple-id <your Apple ID> --team-id <Team ID>
```

Without the profile, `distribute` stops after signing and says so. If Apple rejects the submission, it prints the notary log.

The distributed app needs macOS 26 and an Apple silicon Mac: the Rust core is built for the building Mac's architecture only (`ONLY_ACTIVE_ARCH`).


## The app

One window per project: a sidebar (Outline, Media, Runs), the spec in the middle (source or preview, ⌘/), and an inspector (Claude, Project, Outputs, Validation, Run). The toolbar has the brand preset, the run config, Validate (⌘B), Run (⌘R) and Stop (⌘.). The App Library (⇧⌘B) holds brand presets and output presets.

- **Spec.** Type `/` for commands; `/scene`, `/vo`, `/shot`, `/music` and `/cta` insert the structured blocks. Drop a file on the editor to reference it as `![name](media:<id>)`. A pasted image is stored in `<name>.cutproj.assets/` next to the project.
- **Claude.** Claude edits a copy of the spec; each change is shown as hunks to accept or reject. "Gather missing info" has Claude interview you about gaps. What validation found goes along with each message by default (the "Validation findings" checkbox above the message box), split into what the spec can fix and what it can't; Validation › Ask Claude to Fix, or the sparkle on one finding, writes the request for you.
- **Media.** Files are used where they are. When they move, the app follows them through bookmarks, relative paths, absolute paths and search folders, and lists what is still missing in the Relink sheet (Locate, Search Folder, prefix fix).
- **Choosing a voice.** A run uses the voice of the project's brand preset, else the default voice ID in Settings › ElevenLabs. When neither is set Claude chooses one, and Settings › ElevenLabs says how: *Tried voices* (voices earlier videos used come first, most used at the top, then the longest-standing voices of the account) or *Latest voices* (newest first), and any, female or male. `video-studio eleven voices [--gender female|male|any] [--prefer tried|latest]` lists the account's voices in that order.
- **Cartesia for the voice-over.** Settings › ElevenLabs › *Voice-over by* chooses who speaks the narration: ElevenLabs (the default) or Cartesia (cartesia.ai, model `sonic-3.6`). Cartesia has its own API key, stored in the Keychain beside the ElevenLabs one (`video-studio secret set --provider cartesia`), and its own voice IDs: one in the brand preset (*Cartesia voice ID*), else the one in Settings, else Claude chooses from Cartesia's list by the same preferences. Inside a run the commands don't change (`video-studio eleven tts` and `eleven voices` reach Cartesia); `--settings` takes Cartesia's `speed`, `emotion` and `volume`, a character counts as one credit against the run's cap, and requests are cached and logged like the others. Music, sound effects and word timings stay with ElevenLabs; without an ElevenLabs key a run takes word timings from `cutline transcribe` and can't generate music. Neighbouring-text context (`--before-file`, `--after-file`) is ElevenLabs only. The Windows app has no control for this yet: set `"voice_provider": "cartesia"` in the library's `settings.json`.
- **One voice across scenes.** The voice-over is generated a passage per scene. So that it sounds like one reading, every passage uses the same voice and settings and is sent with the text of its neighbours as context (`eleven tts --before-file … --after-file …`); a wrong take is asked for again as `--take 2` rather than with other settings; the render brings every passage to the same loudness; and `cutline validate` reports a passage spoken much faster or slower than the rest.
- **Finish.** Settings › Finish holds how every video is finished: how lively the voice and the music are (calm, balanced or upbeat, the default: an animated read with short pauses over bright music at about 120 beats a minute), the transition between scenes (smooth, plain cuts or lively) and its length, cuts on the music's beat, eased on-screen text and close-ups, the lead-in before the first word, the hold after the last, and the music's fade in and out. Every run is given these as instructions. A project follows them, including later changes, unless Project › Finish is set to "This project's own". The spec can still ask for something different.
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
       permissions.ndjson  helper.ndjson  prompt-system.md  prompt.md
       summary.md  cutline/<n>-<command>.log  qa/
```

Each run keeps what is needed to study it afterwards: `prompt-system.md` and `prompt.md` are the instructions Claude was given, `claude.jsonl` is everything Claude Code said and did, `permissions.ndjson` is every tool call the app allowed or denied, `helper.ndjson` lists each `video-studio` command Claude ran with its exit code and time, and `run.json` records the versions of the app, cutline, Claude Code and the operating system.

**Export Logs…** (the Run inspector's Logs tab, the Run menu, or a right-click in the Runs list; `video-studio logs <folder> --zip <file>` in a terminal) writes one zip to send to whoever maintains the app. It opens with `README.md`, an index of the run and of every file, and `issue-report.md`, and adds the spec, the edit, the brand files, Claude's summary and the text files of the review packs. No media is included. With redaction on, media paths, media file names and the home folder are replaced with placeholders.

In the app, text the app shows (validation results, dry-run findings, Claude's messages, errors, logs) has a copy button beside it, and Markdown (Claude's messages, the run summary, Markdown logs) opens as a preview with a button to show the source.

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
- macOS must allow **Screen & System Audio Recording** and **Accessibility** for Syncfusion Video Studio (or for the terminal, when using the CLI). The Project inspector shows both and links to the settings. Unless the app is signed with a certificate (see [Signing](#signing-macos)), a rebuilt app needs the permissions allowed again.
- The pointer and keyboard are shared with whoever is at the Mac while a run drives an app.
- The platform part is one trait (`capture::Backend`), with a fake for tests (`VIDEO_STUDIO_CAPTURE_FAKE`). Windows has its own backend: see [Recording apps on Windows](#recording-apps-on-windows).

## Tests

```bash
cargo test -p video-studio-core -p video-studio-cli -p video-studio-ffi
apps/macos/build.sh selftest
scripts/video-studio-live-test.sh        # optional: one real run on Haiku, costs a few cents
```

The Rust end-to-end tests use `vs-fake-claude`, which speaks the stream-json protocol and replays a script (its tool calls go through the hook and the permission prompt as the real CLI's do), `vs-fake-cutline`, and a fake ElevenLabs server. The in-app self test drives the real app with the fake Claude and the real cutline over `cutline selftest --quick` assets, and saves window snapshots next to its log.

Environment variables for tests and unusual setups: `VIDEO_STUDIO_HOME` (library folder), `VIDEO_STUDIO_CLAUDE` and `VIDEO_STUDIO_CUTLINE` (binary paths), `VIDEO_STUDIO_NO_CLAUDE` (pretend the CLI is missing), `VIDEO_STUDIO_ELEVEN_BASE_URL`.

## Windows

The Windows app (`apps/windows`, ui/spec.md §23) is a Rust shell on `windows-rs` with its UI in WebView2: TypeScript, CodeMirror 6 and Fluent UI web components, with the Mac app's preview scripts reused as they are. It links the core directly, so projects, brand presets, run folders and the CLI are the same on both platforms; a project saved on one opens on the other.

### Requirements (Windows)

- Windows 11 22H2 or later, x64 or ARM64. The WebView2 Runtime (part of Windows 11; the app says so if it is missing).
- Claude Code 2.1.0 or later, signed in (`claude` in a terminal). Verified with 2.1.319.
- **Git for Windows.** Claude Code runs its Bash tool and hooks through Git Bash. The app finds `bash.exe` (Git's install folder, or `CLAUDE_CODE_GIT_BASH_PATH`) and passes it on; validation reports a blocker when it is missing.
- cutline with its dependencies (`cutline deps install --all`), and an ElevenLabs key only when a spec needs generated audio.
- To build: a Rust toolchain (with the `aarch64-pc-windows-msvc` target for ARM64), Node.js 20 or later, and Visual Studio Build Tools with the Windows 11 SDK (`makeappx`, `signtool`). ARM64 builds also need clang (LLVM, or the "C++ Clang tools for Windows" component), which the TLS crates use for their ARM64 code.

### Building (Windows)

```powershell
apps\windows\build.ps1             # Web UI + debug build, laid out in target\debug\app\
apps\windows\build.ps1 release     # release builds for x64 and ARM64 (-Arch x64 for one)
apps\windows\build.ps1 package     # MSIX per architecture, the .msixbundle, portable CLI zips
apps\windows\build.ps1 selftest    # debug build, then the in-app self test
apps\windows\build.ps1 uitest      # debug build, then the UI Automation tests
apps\windows\build.ps1 test        # fmt, clippy -D warnings, cargo test for the workspace
```

The bridge between the shell and the UI is typed from one source: `apps/windows/src/protocol.rs`. Its JSON Schema (`ui/src/generated/bridge.schema.json`, refreshed with `VIDEO_STUDIO_BLESS=1 cargo test -p video-studio-win`) generates the TypeScript types on every UI build, as does `schema/cutproj.schema.json` for the project model.

`package` signs with `VIDEO_STUDIO_SIGN_PFX` / `VIDEO_STUDIO_SIGN_PASSWORD` when set. Otherwise it makes a self-signed test certificate once (in `CurrentUser\My`, exported to `target\package\test-signing.cer`) and prints the two commands that trust it and install the bundle. The package installs the `video-studio` execution alias, the `.cutproj` and `.cutbrand` file associations, and the toast activator. The portable zip holds `video-studio.exe` and the CLI section of this README.

### Locations (Windows)

| What | Where |
|---|---|
| Library (settings, brand and output presets, assistant sessions) | `%APPDATA%\Syncfusion Video Studio\` (`VIDEO_STUDIO_HOME` overrides) |
| Caches (thumbnails, ElevenLabs cache) | `%LOCALAPPDATA%\Syncfusion Video Studio\Cache\` |
| Default run folders | the Videos known folder, `Syncfusion Video Studio\` |
| ElevenLabs key | Windows Credential Manager, "Syncfusion Video Studio" / `elevenlabs` |
| Pasted images | `<name>.cutproj.assets\` next to the project, as on macOS |

Media references keep a Windows file ID (`file_id_windows`, the NTFS file ID with its volume serial) beside the Mac bookmark, so a file renamed or moved on the same volume is still found. Paths are stored as typed; long paths work (the app is long-path aware), and file names that Windows reserves (`CON`, `NUL`, `COM1`…) are avoided when the app names files.

### Shortcuts (Windows)

| Action | Keys | Action | Keys |
|---|---|---|---|
| New Project / Open / Save | Ctrl+N / Ctrl+O / Ctrl+S | Validate | Ctrl+Shift+B |
| Save As | Ctrl+Shift+S | Run / Stop | F5 / Shift+F5 |
| Source / Preview | Ctrl+/ | Send to Claude | Ctrl+Enter |
| Outline / Media / Runs | Ctrl+1 / 2 / 3 | Add Media | Ctrl+Shift+M |
| Claude / Project / Outputs / Validation / Run tabs | Ctrl+Alt+1 … 5 | App Library | Ctrl+Shift+L |
| Find / Replace | Ctrl+F / Ctrl+H | Settings | Ctrl+, |
| Undo / Redo | Ctrl+Z / Ctrl+Y | Close window | Ctrl+W |

The menu bar shows the same shortcuts. The app follows the system light or dark theme, accent color, high contrast and text scaling, and uses Mica where transparency effects are on.

### Recording apps on Windows

`video-studio capture` works as on macOS, with a Windows backend: UI Automation to find windows and menus, `SendInput` for the pointer and keys (`cmd` in a key combo means Ctrl), and Windows.Graphics.Capture for recordings, encoded by ffmpeg (H.264) with the app's own audio through WASAPI process loopback. Recordings are `.mp4`.

- No permission prompts are needed. Windows draws a yellow border around a window being recorded.
- Terminals (cmd, PowerShell, Windows Terminal), Settings and Control Panel, Registry Editor, Task Manager, MMC, password managers and File Explorer can't be driven. Neither can a window running as administrator (UI Automation and input from a normal process can't reach it); the helper refuses it and says so.
- The pointer and keyboard are shared with whoever is at the PC while a run drives an app.

### Tests (Windows)

```powershell
cargo test -p video-studio-core -p video-studio-cli -p video-studio-ffi -p video-studio-win
apps\windows\build.ps1 selftest
apps\windows\build.ps1 uitest
scripts\video-studio-live-test.ps1        # optional: one real run on Haiku, costs a few cents
```

- The Rust suites run on Windows CI for x64 and ARM64, including the Windows-only tests: Job Object process trees and Ctrl-Break, Credential Manager, file IDs across a rename, Git Bash command forms in the broker, CRLF projects and reserved names. `tests/capture_windows.rs` drives `vs-capture-target.exe` (open, window, screenshot, click, type, key, menu, scroll, drag, a 5-second recording checked with ffprobe, and the terminal and elevated-window guards).
- The in-app self test (`--selftest`) drives the app with the fake Claude and the real cutline, as on macOS, and saves WebView2 snapshots next to its log.
- `vs-uitest` drives the app only through UI Automation and synthesized input: onboarding, opening a project by path, editing and slash commands, every Windows shortcut, the App Library, Validate, Run and Stop, log export through the Save dialog, an accessible-name audit and a keyboard-only pass. Screenshots go next to its log.
- The live test uses the installed MSIX's `video-studio` alias when present, else the debug build.

`.gitattributes` keeps the repository LF on Windows checkouts, so the schema and snapshot tests compare the same bytes on every platform.

## What a dry run checks, and what it leaves to production

A dry run records and generates nothing, so its edit points at recordings and audio that don't exist yet. To keep cutline from stopping at the missing files, Claude writes a stand-in for each one with `video-studio standin` (a flat-colored clip or a quiet sound of the planned length) and then runs `cutline validate`, `expand` and `review` on the whole edit. That covers timing, text layout, captions against the watermark and each output's framing.

- Files the run makes itself are listed under `planned` in `readiness.json` and shown as "Made during the production run". They are not missing media and don't count against the report.
- `missing_media` is only for media the user has to supply.
- Zooms onto parts of an app can only be measured from the real recording, so the dry run leaves them out.
- A voice-over stand-in made with `--text-file` is as long as its words take to speak and comes with a stand-in transcript (`audio/<name>.json`, word timings spread over that length). The asset gets `script` and `transcript` as it will in production, so captions and cue timing are checked too.
- `cutline review` shows the first export only. For each other export with a different shape, the dry run reviews a scratch copy (`check-<output>.json5`) with only that export.

Validation (before any run) also checks what a run would otherwise find the expensive way: no voice chosen for the voice-over, a spec that asks for app recordings while App recording is off or the computer doesn't allow it, voice-over that doesn't fit its scene, burned-in captions asked for on an output that delivers them another way, an output the spec asks for that the run doesn't make, and a watermark asked for when the brand preset has none.

## The brand in a run folder

The app writes the brand twice: `brand-preset.json` is the preset as the user entered it (rules, voice, music mood, styles in their own words), for Claude to read. `brand/default.json` is the same brand in cutline's brand format, which cutline loads without being told. cutline draws text only with its bundled fonts (Anton, Bebas Neue, Inter, Poppins, Source Serif 4); another brand font becomes Inter and Claude is told to say so.
