---
id: BUG-amber-walrus-72VP
type: bug
title: Codex Audio Review stalls after patch approval and issue report mislabels the failure
status: open
fix: none
verified: no
approved: yes
approved-on: 2026-11-23
priority: medium
created: 2026-11-23
updated: 2026-11-23
attachments: []
---

# Codex Audio Review stalls after patch approval and issue report mislabels the failure

## Summary

Codex received approval to write four narration files. Video Studio stopped the run after 120 seconds without confirmation that the change had been applied. The newer timeout message correctly distinguishes a second pending change. The underlying stall persists. The exported issue report also attributes Codex output to Claude, promotes schema descriptions to warnings, and presents an in-progress summary alongside a terminal failure.

## Environment

- Run `run_01m84n2c6e9z9p3yamvm`, Audio Review, macOS aarch64, app core and cutline 0.1.0, Codex model `gpt-6.1-sol`. The report does not establish the Codex CLI version.
- Selected output: YouTube Shorts, 1080×1920. Requested mix: 45 seconds with restrained 90 BPM music and no narration from 40–45 seconds. The previously confirmed ElevenLabs key remains configured. This report supplies no credential error.

## Steps to reproduce

1. In a temporary project, select Codex and start Audio Review for one Shorts output. Supply four short narration passages and a local audio fixture. Use the normal run preparation, sandbox and approval broker. The reference run folder is unnecessary.
2. In the app-server fixture, emit one file-change item for additions to `audio/scene1.txt` through `audio/scene4.txt`, then request approval for its item ID. After acceptance, leave completion pending. At 111 seconds request a separate change to `summary.md` with its own item ID. Advance a test clock to 120 seconds. Also exercise a normal server that applies and confirms the authorized changes.
3. Export an issue report from a local failed-run fixture with provider Codex, the model above, three exit-zero cutline logs, and no output WAV. In a schema log include JSON description strings containing `W_FIT_HOLD` and `W_SCRIPT_MISMATCH`. Put “Audio generation is in progress” in the draft summary. These strings reproduce the reporting defects independently of the live stall.

## Expected result

A valid workspace patch completes through the normal Codex protocol and the review can proceed to a playable WAV. If completion genuinely remains unknown, the app stops safely and reports each item's state. The exported report identifies the actual provider and terminal outcome, and lists only diagnostics emitted for the run.

## Actual result

The first patch timed out. The change to `summary.md` was abandoned nine seconds after acceptance. The report correctly says file state is unknown and no soundtrack was mixed. All three cutline invocations exited successfully. Nevertheless, the report displays “Claude Code: unknown”, “Claude's summary”, and two schema definitions under “cutline warnings”. The draft says audio generation is in progress. Its presence does not establish that the specific pending patch completed.

## Fix checklist

- [ ] Trace one normal multi-file patch from `item/started` through `item/fileChange/requestApproval`, broker decisions, response delivery and server completion in `crates/video-studio-core/src/codex/agent.rs`. Compare with a supported local Codex app server using a disposable workspace; record the CLI version, item/request IDs, terminal notifications and observed file contents. Identify the first divergence. A fixture that deliberately omits completion tests the deadline only and cannot establish the cause.
- [ ] Correct the confirmed divergence. Verify a clean run creates all four expected script contents, accepts only authorized workspace changes, and produces a decodable `outputs/audio-review.wav` using local audio. Retain separate deadlines, one response per request and unknown file state for genuinely unconfirmed writes (BUG-green-cricket-P6HQ and BUG-deft-lily-344X).
- [ ] Update report generation in `crates/video-studio-core/src/logs.rs` to use recorded provider and effective model. Use a neutral label for legacy records without reliable attribution. On failed runs identify assistant-authored summary text as a draft and give the host's terminal result precedence while retaining the draft as evidence.
- [ ] Replace the warning collector's substring match on “warning” or `W_` with recognition of emitted diagnostics. Schema/help prose must contribute zero warnings. Preserve genuine structured validation findings and supported plain-text warning lines, including warnings on commands that exited zero.
- [ ] Extend `crates/video-studio-cli/tests/codex_runs.rs` and the report tests in `logs.rs` or `tests/e2e_runs.rs`. Verify the successful patch path and the 120 s/9 s timeout distinction. For the supplied report fixture, assert Codex attribution, failed status, missing WAV, three successful commands and no schema warnings. Add a real diagnostic as a positive control and exercise Claude and legacy report attribution.

## Implementation log

### 2026-11-23

- Implemented: Documented the recurring stall and issue-report defects with independent reproduction and verification fixtures.
- Not done: Runtime diagnosis, product fixes and verification await approval.
- Files: This bug README.md.
