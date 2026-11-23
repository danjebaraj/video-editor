---
id: BUG-green-cricket-P6HQ
type: bug
title: Codex Audio Review reports narration files written before the patch completes
status: fixed
fix: fixed
verified: no
approved: yes
approved-on: 2026-11-22
priority: medium
created: 2026-11-22
updated: 2026-11-22
attachments: []
---

# Codex Audio Review reports narration files written before the patch completes

## Summary

Codex received enough project context to plan the narration, then stalled on a four-file patch. Video Studio displayed four successful writes. The app-server trace shows an accepted approval but no matching file-change completion. The agent could not confirm its inputs and stopped before generating speech or mixing audio. This is a later failure point than BUG-rustic-olive-U4PY.

## Environment

- macOS desktop, Codex App Server, requested model Codex default, resolved model `gpt-6.1-sol`. Exact app build unknown.
- Audio Review, Default config, YouTube Shorts. The requested mix was 45 seconds with four narration passages, restrained 90 BPM music and a music-only closing five seconds.

## Steps to reproduce

1. In a temporary run workspace whose path contains spaces, prepare an Audio Review with four short local narration strings. Use the existing fake Codex server in the CLI integration tests and a local speech-helper fixture that returns generated tone WAVs. Supply the project context through the normal run entry point.
2. Emit one `item/started` of type `fileChange` listing four additions, `audio/scene1.txt` through `audio/scene4.txt`. Follow it with `item/fileChange/requestApproval` carrying numeric request ID 2, the same item ID, and null `grantRoot` and `reason`. The file paths come from the started item.
3. Capture the approval response. In the failure fixture, acknowledge receipt but withhold `item/completed`. Emit requests for `ls audio` and `cat audio/scene1.txt`, then run `cutline audio --help`. In a separate success fixture, apply the patch in the sandbox, emit its successful completion and continue to speech generation, validation and mixing.

## Expected result

Approval and successful application are distinct states. Confirmed workspace files can feed the authorized audio helper. An unresolved write terminates with a specific actionable diagnostic, and a help request never starts the Mix stage.

## Actual result

At 23:05:44 local time the inspector reported four “Wrote” events. The protocol trace contains approval response `{"id":2,"result":{"decision":"accept"}}` but no corresponding completed file-change item. The eventual on-disk state is unknown. The broker separately declined the two shell inspection requests. The agent reported the patch still pending. Only schema, doctor and audio help ran. Help started Mix. After 188 seconds the run exited 3, with no WAV, no agent-authored summary and zero ElevenLabs credits spent. The fallback summary correctly named Codex but referred the user to a permissions file absent from the supplied log directory.

## Fix checklist

- [x] Trace the four-path request through `crates/video-studio-core/src/codex/agent.rs`, including per-path decisions, request/response correlation and terminal item handling. Record whether the server applied the patch, failed it, or stopped responding. Treat the captured acceptance as evidence of host response only.
- [x] Validate every destination against the run workspace before approving the batch. Send one correlated response per request. Emit pending status on start and successful writes only after confirmed success. Surface failed, declined and missing completions distinctly. Bound the pending wait with a documented deadline and clear it on completion, Stop or disconnect. Keep the existing shell allowlist.
- [x] In `run/events.rs`, classify `cutline audio --help` as informational. In `run/orchestrator.rs`, retain the earliest unresolved operation in the terminal message, identify affected relative paths, and link only diagnostics that were written.
- [x] Extend `crates/video-studio-cli/tests/codex_runs.rs` with success, withheld completion, explicit failure and mixed allowed/outside-workspace batch cases. Assert exactly one approval response, no premature write success, no out-of-workspace change, and a single terminal outcome. Exercise timeout using a short test deadline.
- [x] Verify the success fixture passes the four script contents to the local helper and produces a decodable `outputs/audio-review.wav` through the normal audio command. Verify the stalled fixture spends zero credits, exposes no playback, reports the pending patch and allows a subsequent run. The two shell denials and the help command must not be reported as a completed write or an attempted mix.

## Implementation log

### 2026-11-22

- Implemented: Documented the protocol sequence, user-visible failures and local regression fixtures.
- Not done: Root-cause confirmation, implementation and verification await approval.
- Files: This bug README.md.

### 2026-11-22 (fix)

- Implemented: every fix checklist item.
  - Cause: `agent.rs` turned the `fileChange` item's start into Write tool calls, and the orchestrator showed "Wrote" for each one on the spot. The host's `accept` was the only evidence; nothing waited for Codex's `item/completed`. In the reported run Codex never sent it, so the patch's real outcome is unknown.
  - `codex/agent.rs`: every path in a batch is still checked before one correlated answer goes out, and one refused path declines the whole batch. An accepted change is now tracked until Codex reports it applied. Applied, failed (`Codex couldn't apply this change (failed)`) and declined changes are reported separately. Calls the broker declined are closed quietly, because the "Blocked" note already said why. The wait has a documented 120 s limit (`PATCH_TIMEOUT`; tests shorten it with `VIDEO_STUDIO_CODEX_PATCH_TIMEOUT_MS`). When it runs out, the run stops and names the files. The wait is cleared on completion, Stop, abort or disconnect. A turn that ends with a change still unapplied fails with the same cause. The shell allowlist is unchanged.
  - `run/orchestrator.rs`: Codex writes show "Writing `path`…" while pending and "Wrote `path`" only after confirmation. Writes that never completed are named, by relative path, in the failure message, or as a warning if the run otherwise succeeded. The summary links only log files that exist; Codex runs have no `permissions.ndjson`. `--help` calls don't count as progress.
  - `run/events.rs`: `--help`/`-h` never starts a stage, so `cutline audio --help` doesn't start Mix.
  - `run/prompt.rs`: Codex is told not to retry a refused read and not to read back files it has just written.
- Tests (`crates/video-studio-cli/tests/codex_runs.rs`, new fake Codex `patch` step): a successful four-file patch whose scripts feed the speech helper and produce the WAV; a withheld completion with a 1.5 s limit (no "Wrote", no Mix, zero credits, no WAV, files named, the next run works); a failed patch; and a batch with one path outside the run folder (declined whole, one answer, nothing written). Each checks for exactly one approval answer. `cargo test --workspace` passes.
- Not done: the fixture's run folder path has no spaces; the World test fixture puts the library in a temp folder without them. Not tried yet with the real Codex CLI, so the bug stays unverified.
- Files: `codex/agent.rs`, `run/orchestrator.rs`, `run/events.rs`, `run/prepare.rs`, `run/prompt.rs`, `assistant.rs`, `src/bin/fake_codex.rs`, `tests/codex_runs.rs`.

