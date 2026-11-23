---
id: BUG-green-cricket-P6HQ
type: bug
title: Codex Audio Review reports narration files written before the patch completes
status: open
fix: none
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

- [ ] Trace the four-path request through `crates/video-studio-core/src/codex/agent.rs`, including per-path decisions, request/response correlation and terminal item handling. Record whether the server applied the patch, failed it, or stopped responding. Treat the captured acceptance as evidence of host response only.
- [ ] Validate every destination against the run workspace before approving the batch. Send one correlated response per request. Emit pending status on start and successful writes only after confirmed success. Surface failed, declined and missing completions distinctly. Bound the pending wait with a documented deadline and clear it on completion, Stop or disconnect. Keep the existing shell allowlist.
- [ ] In `run/events.rs`, classify `cutline audio --help` as informational. In `run/orchestrator.rs`, retain the earliest unresolved operation in the terminal message, identify affected relative paths, and link only diagnostics that were written.
- [ ] Extend `crates/video-studio-cli/tests/codex_runs.rs` with success, withheld completion, explicit failure and mixed allowed/outside-workspace batch cases. Assert exactly one approval response, no premature write success, no out-of-workspace change, and a single terminal outcome. Exercise timeout using a short test deadline.
- [ ] Verify the success fixture passes the four script contents to the local helper and produces a decodable `outputs/audio-review.wav` through the normal audio command. Verify the stalled fixture spends zero credits, exposes no playback, reports the pending patch and allows a subsequent run. The two shell denials and the help command must not be reported as a completed write or an attempted mix.

## Implementation log

### 2026-11-22

- Implemented: Documented the protocol sequence, user-visible failures and local regression fixtures.
- Not done: Root-cause confirmation, implementation and verification await approval.
- Files: This bug README.md.
