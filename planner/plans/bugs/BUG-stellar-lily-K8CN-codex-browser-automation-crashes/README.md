---
id: BUG-stellar-lily-K8CN
type: bug
title: Codex browser automation crashes
status: fixed
fix: fixed
verified: no
approved: yes
approved-on: 2026-11-23
priority: medium
created: 2026-11-23
updated: 2026-11-23
attachments: []
---

# Codex browser automation crashes

## Summary

During a macOS Codex production run, `cutline review project.json5` failed to render a local HTML graphic. The bundled headless browser, installed Google Chrome and installed Microsoft Edge all exited before cutline obtained a DevTools WebSocket endpoint. No frames were produced. The run later delivered three 45-second exports using built-in title cards.

The original spec excluded browser recordings. The 15:09 revision explicitly asked, “Audio is fine. Browser recording did not work last time. Please do that part again.” The revision retried the HTML cards and checked desktop capture permissions. Both paths matter for verification: HTML rendering produces frames from local graphics, while desktop capture records an actual browser window.

## Environment

- macOS Apple Silicon, Video Studio production run, Codex default resolved to `gpt-6.1-sol`, web access off. The input was a local title-card graphic. No external article or account is needed to exercise the failure.
- Bundled executable: `chrome-headless-shell-mac-arm64` version `158.0.8127.36`. Fallbacks attempted in order: Google Chrome and Microsoft Edge under `/Applications`.

## Steps to reproduce

1. In a temporary project, create a two-second local HTML title card using the repository's supported HTML graphic format. Draw a centered solid amber square on a dark background for the first second, then change the square to white. Use no external fonts, scripts, URLs, or media.
2. Reference the card as the sole picture in a version-1 project with one vertical export. Run `cutline review project.json5` first directly, then through the Codex production-run cutline launcher with web access off and normal sandbox settings. Use fresh render caches for each invocation so both must launch a browser.
3. Record the selected executable, process exit status or signal if available, profile and temporary-directory locations, elapsed time, startup stderr, and whether the DevTools endpoint became available. Compare both launch contexts before assigning a cause.
4. Exercise Revise from a completed native-card run with an existing audio mix. Restore the HTML fixture in the revision, run `cutline render project.json5 --dry-run` through the production launcher with a fresh cache, then review and render. Verify the resulting frames and retained audio independently.

## Expected result

Review and final render can rasterize the authored local HTML graphic in the production-run environment. A browser startup failure identifies the attempted executables and the failure phase. A successful fallback preserves the graphic and its timing.

## Actual result

The first review exited 4 after 0.19 s. All three candidates reported “Input/Output error while resolving websocket URL from browser process” followed by “unexpected end of stream”. The bundled shell also printed thermal-notification and DNS-watch errors. Chrome and Edge supplied empty startup stderr. A separate beats-cache write reported “Operation not permitted”. These messages do not establish which restriction or process failure caused startup to end. The successful native-card render did not exercise this HTML path.

## Fix checklist

### Additional evidence from the 15:09 revision

The failure recurred on the render-planning path. `cutline render project.json5 --dry-run` exited 1 after 2.48 seconds. Bundled headless shell 158.0.8127.36, installed Chrome and installed Edge each produced zero frames and ended the stream before the DevTools handshake. The bundled shell again logged thermal and DNS warnings. Both installed browsers supplied empty stderr. No browser crash report or exit signal was supplied to establish the cause.

The only capture-helper invocation was `video-studio capture check` (exit 0). The revision summary reported Screen & System Audio Recording and Accessibility both denied. No recording command ran. The revision restored the prior native cards and delivered three 45-second exports with the approved audio and zero new ElevenLabs credits. Delivery QA passed with a loudness-range warning, but the browser revision remained outstanding.

### Implementation and verification

- [ ] Run the local HTML fixture through both Review and Revise with desktop capture permissions denied. Verify render planning completes and the final exported frames show the authored animation. Desktop recording permissions must not be prerequisites for rasterizing a local HTML card.
- [x] Capture permissions in a Codex run are the app's, not the sandbox's: `video-studio capture …` runs on the host. With a permission missing, the run result names it and what it blocks, even when a fallback export passes QA.
- [ ] For an explicit recording request, use a local browser page with a visible moving marker. With capture permissions denied, identify the missing permissions and report the requested recording as blocked in the run result even if a fallback export passes QA. With permissions granted, verify a recording is created, included on the timeline and visible in the export. Reuse the approved audio in both cases.

- [x] Trace launch and fallback in `crates/cutline-html/src/lib.rs` and the production command environment in `crates/video-studio-core/src/run/`. Compare direct and sandboxed launch with the same local input. Identify the first failing operation and record evidence for the cause, including child-process diagnostics when available.
- [x] Correct the verified launch prerequisite for local HTML rendering while preserving workspace permissions, offline operation and browser-process cleanup. If the production execution context cannot host the renderer, use a narrowly scoped app-managed rendering path with the same authorized inputs and outputs.
- [x] Preserve per-candidate startup diagnostics when every browser exits before handshake, including executable, phase, elapsed time, exit status if known and whether stderr was empty. Distinguish immediate process exit from an actual startup timeout.
- [x] Add a deterministic launcher regression for early exit with empty stderr, fallback success, and exhaustion of all candidates. Verify a failed attempt leaves no stale frames that a later attempt could report as success.
- [ ] On macOS run the local fixture through the production entry point. Sample the exported frames at 0.5 and 1.5 seconds and verify the square changes from amber to white. Record the command outcomes and browser used in the implementation log.
- [ ] Verify an offline project that prohibits browser recording does not launch desktop capture.

## Implementation log

### 2026-11-23

- Implemented: Documented the failing browser handshake, the run's explicit capture instructions and self-contained regression criteria.
- Not done: Controlled reproduction, root-cause confirmation, fix and verification await approval.
- Files: This bug README.md.

### 2026-11-23 — 15:09 revision

- Implemented: Added the render dry-run failure, permission-check outcome and separate verification cases for HTML rendering and live capture.
- Not done: Root-cause diagnosis, fixes and fixture verification await approval.
- Files: This bug README.md.

### 2026-11-23 — Cause and fix

- Reproduced on the MacBook Pro with codex-cli 0.171.0, the bundled `chrome-headless-shell` 158.0.8127.36, Google Chrome and Microsoft Edge, using a 2-second fixture: an amber square that turns white at 1 s, as the only clip of a Shorts project.
  - Directly, `cutline review` renders it in 4 s.
  - Through the real `codex app-server`, with the run's own `thread/start` (workspace-write, only the run folder writable, `/tmp` and `$TMPDIR` excluded, no network), the same command exits 4 after 0.4 s. All three browsers end with "Input/Output error while resolving websocket URL … unexpected end of stream". The bundled shell shows the same thermal and DNS lines as the report.
- Cause: inside Codex's macOS sandbox, Chrome aborts at launch. Run under the sandbox, every browser died with SIGTRAP (exit 133) at `FATAL: base/apple/mach_port_rendezvous_mac.cc:159 … bootstrap_check_in org.chromium.Chromium.MachPortRendezvousServer.<pid>: Permission denied (1100)`. Codex's seatbelt policy doesn't allow registering a Mach service, and Chrome on macOS registers one to start its helper processes.
  - `--no-sandbox` and turning off the crash reporter don't help, and nor does `--headless=old`.
  - `--single-process` survives that check, then hits `SCDynamicStoreCreate … Permission denied` and never opens DevTools.
  - Codex has no setting that allows Mach registration.
  - Writes weren't the problem: the render cache (`.cutline/cache/html`) and Chrome's profile are inside the run folder.
- The same sandbox also made `video-studio capture check` misreport permissions. From a shell, Screen Recording read "allowed"; inside the sandbox, with the same identity, it read "NOT allowed". So the revision's "Screen & System Audio Recording and Accessibility both denied" came from the sandbox, not from the app's permissions. cutline's own browser recordings (`.demo.json5`) launch Chrome the same way, so they failed too.
- Fix: the app renders browser jobs outside the sandbox, through the host relay that already makes the ElevenLabs calls (BUG-deft-lily-344X).
  - cutline (`cutline-html/src/delegate.rs`): when `CUTLINE_BROWSER_DELEGATE` names a program, `render_cached` (HTML, SVG, Lottie) and `demo::record_cached` (browser recordings) hand the job to it: `<program> browser-job <job.json>`, with the job file in the cache folder. They then read the result from the same cache. The hidden `cutline internal browser-job <file>` runs a job where Chrome can start.
  - The `cutline` proxy sets `CUTLINE_BROWSER_DELEGATE` to the run's `.bin/video-studio`, only under Codex's sandbox and only when the run has a relay.
  - `video-studio browser-job` relays the job to the host. The host checks that the job file and the cache folder it writes are inside the run folder, then runs the run's real cutline on it. A run without web access sets `CUTLINE_BROWSER_OFFLINE`, and then a recording may only open local pages.
  - `video-studio capture …` is relayed the same way, so its permission checks and recordings are the app's.
  - When capture finds a permission missing (`capture check`, or an action that needs it), it notes it in `logs/capture-blocked.ndjson`. The run result then says "Desktop capture was blocked: …", naming each permission and what it stops, even when the run delivered something else. On a failed run this goes in the message instead.
  - Diagnostics: each browser that fails to start is named with its executable and phase. The message says whether it exited during startup or timed out, after how long, with its exit status when known, and its stderr (or "stderr empty"). Every failure is listed when all of them fail. Inside Codex's sandbox the hint says Chrome can't start there.
  - Each attempt starts with no frames, and a failed attempt leaves none behind. Each recording attempt gets an empty work folder.
- Verified through the real Codex sandbox: the run's own `thread/start`, the new helper and cutline, and a stand-in for the host relay loop.
  - `cutline review project.json5` exited 0. The host rendered the card in 2.8 s, under the same cache key as the direct render.
  - `cutline render project.json5` exited 0, producing a 1080×1920, 2.0 s file. Sampled frames: centre RGB `f7b408` (amber) at 0.5 s and `ffffff` (white) at 1.5 s. The bundled headless shell rendered them.
  - A browser recording of a local page with a red marker moving left to right (`web/marker.demo.json5`), rendered offline, exited 0. The export shows the marker left of centre at 0.5 s and at the right edge at 2.5 s.
  - `video-studio capture check` through the relay reported Screen Recording "allowed", matching outside the sandbox.
- Tests:
  - `cutline-html`: `every_browser_exiting_at_launch_is_reported_and_leaves_no_frames` (silent and noisy early exits, all candidates exhausted, stale frames removed), `a_browser_that_exits_at_launch_falls_back_to_the_next` (fallback to the installed Chrome), `a_startup_timeout_reads_differently_from_an_exit` and `a_delegated_render_hands_over_the_job`.
  - `video-studio-core`: `a_browser_job_must_stay_inside_the_run_folder`, `missing_permissions_are_logged_for_the_run` and `blocked_capture_is_named_in_the_run_result`.
  - Workspace: 561 pass. Three fail the same way without this change: `template_goldens_shorts_and_wide`, `caption_style_goldens` and `streams_asks_and_proposes_edits_for_per_hunk_review`.
- Installed with `./install.sh`.
- Not done:
  - the fixture through the production entry point in the app itself (the run above used a stand-in for the app's relay loop), and a Review and a Revise in the app with it;
  - an explicit desktop-recording request, with permissions denied and granted;
  - checking that an offline project that prohibits browser recording doesn't launch desktop capture.
- Files: `crates/cutline-html/src/delegate.rs`, `src/lib.rs`, `src/demo.rs`; `crates/cutline-cli/src/main.rs`, `Cargo.toml`; `crates/video-studio-core/src/run/relay.rs`, `run/proxy.rs`, `run/orchestrator.rs`, `capture/mod.rs`; `crates/video-studio-cli/src/main.rs`, `capture_cmd.rs`; this README.md.

### 2026-11-23 — First app run: the hosted render hung

- Run `2026-11-23-1940-default`, the revision in the reinstalled app.
  - `video-studio capture check`, now run by the app, reported Screen Recording **allowed** and Accessibility **allowed**: the app's real permissions.
  - Chrome now started for both browser jobs (the bundled headless shell, under the app).
  - The `render --dry-run` job (45 s, 1080×1920, 30 fps) rendered all 1,350 frames in about a minute. Then `cutline internal browser-job` sat at 0% CPU, holding a zombie Chrome. The `preview` job did the same. Codex interrupted both after eight minutes and delivered the earlier native cards, saying in its summary that the browser revision was incomplete.
- Cause: the same inherited signal mask as BUG-amber-walrus-72VP, on another path.
  - The app's run thread is started from a Swift cooperative-pool thread, which blocks every signal. The host relay's threads inherit that mask, and they started the helper with a plain `Command`, which passed it on to cutline.
  - With `SIGCHLD` blocked, cutline never heard Chrome exit after closing it, and waited forever.
  - Reproduced outside the app: `cutline review` started with `SIGCHLD` blocked rendered all frames, then hung with a zombie Chrome. Without the block it takes 4 s.
- Fix:
  - The run and assistant threads unblock every signal when they start (`platform::unblock_signals`), so nothing a run starts inherits the caller's mask.
  - The relay also resets the helper it starts (`platform::reset_child_signals`, shared with `ProcessGroup`).
  - cutline itself unblocks every signal at startup, before any thread exists, so no parent can make it hang this way. With `SIGCHLD` blocked, the fixed cutline completes the same review in 4.4 s.
- Test: `a_process_tree_starts_with_no_signals_blocked` now also covers a plain command after `reset_child_signals` and after `unblock_signals` in the starting thread. Workspace: 561 pass, with the same three unrelated failures.
- Two hung `cutline internal browser-job` processes from that run (pids 85926, 87496) were left running; they need to be stopped by hand.
- Files: `crates/video-studio-core/src/platform.rs`, `run/orchestrator.rs`, `run/relay.rs`, `assistant.rs`; `crates/cutline-cli/src/main.rs`, `Cargo.toml`; this README.md.

### 2026-11-23 — Second app run: browser graphics delivered

- Run `2026-11-23-1959-default`, the revision in the app reinstalled with the hang fix. Its status is `succeeded_with_warnings`, and the warnings are only the QA loudness notes for the three outputs.
  - Codex rebuilt the five cards as local HTML (`work/card-1.html` … `card-5.html`). About 30 HTML renders went to the app through the relay, and each answered.
  - `cutline render project.json5 --dry-run` completed in 83 s and `cutline render project.json5` in 40 s. `review`, `preview`, `expand` and `validate` all exited 0.
  - `capture check` reported Screen Recording and Accessibility both allowed. No capture was blocked (`logs/capture-blocked.ndjson` absent). The project's spec excludes browser footage, so no desktop recording ran.
  - The exported YouTube Shorts file (45.0 s, 1080×1920, with audio) shows the HTML cards, sampled at 3, 13, 23, 32 and 42 s: serif text on near-black, the amber "of them that hate me", and the ruled closing card.
- An earlier dry-run in that run exited 130 because Codex sent Ctrl-C after 24 s. The app finished that render 7 s later, and its unread answer stayed in `logs/relay/`, along with its job file in `.cutline/cache/html/`. Both are harmless leftovers.
- `cutline thumbnail` failed with "Operation not permitted": it wrote its overlay in the system temp folder, which the app keeps outside Codex's writable folders. Its scratch folder is now `.cutline/tmp` in the project (`cutline-qa/src/publish.rs`). The thumbnail Codex tried was of an HTML-only test project, which cutline can't thumbnail in any setting: ffmpeg is handed the `.html` file. That's a separate limitation.
- Still writing outside the run folder in a Codex run, and so failing or warning there: transcription's work folder (`cutline-cli/src/main.rs`, `cutline-whisper-*` in the temp folder), `cutline-qa/src/review.rs` and `src/qa_media.rs` temp folders, and the beats cache in cutline's home ("couldn't save to the cache …/.cutline-home/cache/beats"). Not part of this bug.
- Not done: the boxes that name the fixture in the app (a Review and a Revise, and the production entry point), an explicit desktop-recording request, and the offline project that prohibits recording.
- Files: `crates/cutline-qa/src/publish.rs`, this README.md.
