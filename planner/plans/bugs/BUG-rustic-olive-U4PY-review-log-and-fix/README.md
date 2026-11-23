---
id: BUG-rustic-olive-U4PY
type: bug
title: Codex Audio Review cannot read prepared inputs or produce the soundtrack
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

# Codex Audio Review cannot read prepared inputs or produce the soundtrack

## Summary

A macOS Audio Review using Codex default (resolved to `gpt-6.1-sol`) failed after 454 seconds with exit code 3. Codex could invoke cutline but could not obtain the project instructions through the file-reading route it tried. The selected YouTube Shorts export had no `outputs/audio-review.wav`. Preflight had zero blockers and the run spent zero of its 20,000 ElevenLabs credits.

## Evidence

| Observation | Consequence |
|---|---|
| The run recorded tools named Read, Write, Edit, Glob, Grep and Bash. The assistant reported that Read, Glob and Grep were unavailable and the instructions prohibited shell reads. It requested five prepared files through `node_repl/js`. The call ended with `timed out awaiting tools/call after 300s`. The broker recorded zero blocked calls. | A prepared workspace and a successful Codex handshake did not establish that Codex could consume the inputs. The evidence does not establish that the files were absent. |
| The only cutline invocations were schema, help and doctor. The event stream marked Validate progress 100% after schema inspection, marked Authoring successful after a summary write, and reported a successful wrapping-up stage before the run failed. | Progress implied project milestones that the run never reached. |
| The final record had provider Codex and kind `audio_review`, while the generated summary said “Kind: production” and the failure message began “Claude finished without mixing the soundtrack”. Validation also described dry-run and production choices as Opus. | The user saw a different provider, model or operation from the one being diagnosed. |
| The last live usage event showed 982,031 input tokens. The final record had 54,927 input and 927,104 cache-read tokens (sum 982,031), with 1,503 output tokens in both. The model repeatedly waited on the pending read. | Reconcile the inclusive and exclusive counters before using them for display or cost estimation. The figures alone do not establish overbilling. |
| Readiness said whisper was missing. Doctor found `ggml-base.en.bin` and all required FFmpeg capabilities. | A model file and a transcription executable are distinct dependencies. Verify which capability each check measured and make any missing-component message specific. |

## Steps to reproduce

1. Use a temporary library and project with a YouTube Shorts export named `youtube-shorts`. Generate a two-second, 440 Hz WAV locally and attach it. In the spec direct the assistant to use that clip once at unity gain, with no generated speech, music or pictures. Put a unique marker in the spec so tests can prove the assistant received its contents.
2. Select Codex default and start Audio Review with web access off. Provide the normal prepared `spec.md`, `media-manifest.json`, `outputs.json`, `brand-preset.json` and run metadata. In `crates/video-studio-cli/tests/codex_runs.rs`, drive the real preparation and orchestration paths with the fake Codex server. Model an app server without Claude-native read tools or a working Node REPL.
3. Have the fixture obtain the marker, media path and export slug through the actual supported input mechanism. Require those values before it authors `project.json5` or requests the mix. Repeat with an unreadable required input and with a simulated pending tool request. Use a controllable clock or short test deadline for the timeout case.

## Expected result

Codex receives the authorized project inputs, validates the authored timeline and mixes the selected export. A successful Audio Review exposes a playable WAV. If an input is inaccessible, the inspector identifies the failed input or operation and gives the user a remedy; no credits are spent on generation before required inputs are available.

## Actual result

The agent reported that it could not read the spec or settings and declined to invent narration. No timeline validation or mix command ran. A summary was written, the result was marked incomplete, and the failure message identified Claude.

## Fix steps

- [ ] In `crates/video-studio-core/src/run/prepare.rs` and `run/prompt.rs`, resolve the prepared inputs before starting audio authoring. Supply their contents in provider-appropriate context or expose a bounded workspace reader supported by the Codex adapter. Describe the mechanism actually available. Keep referenced media read-only and authorize writes only within the run workspace.
- [ ] In `codex/agent.rs` and `run/orchestrator.rs`, ensure every host-handled tool request receives a correlated success, error or cancellation. When the required input route fails, terminate with the underlying cause and release pending work. Verify that the happy path is independent of Node, MCP resource discovery and unrestricted shell access.
- [ ] In `run/events.rs`, `run/orchestrator.rs` and `run/collect.rs`, distinguish informational commands and summary writes from timeline authoring, project validation and audio mixing. Mark success only after the required output is confirmed playable. Preserve the first actionable error alongside the missing-output finding.
- [ ] Use the effective provider, model and run kind consistently in prompts, validation, inspector events, failure text and the generated summary footer. Record the actual Codex input capabilities. Suppress the dry-run cost comparison for an audio review unless it applies to the selected operation and effective models.
- [ ] Reconcile live and final Codex usage against the app-server counters, including cache reads, and exercise repeated identical cumulative updates. Verify optional transcription diagnostics against separate model-present and executable-present inputs.

## Acceptance criteria

- [ ] From a clean temporary project, the normal Codex Audio Review path reads all required inputs, runs `cutline validate project.json5` and `cutline audio project.json5 --export youtube-shorts --out outputs/audio-review.wav`, and produces a decodable two-second WAV containing the source tone. No picture render, capture or paid service is invoked.
- [ ] A missing spec, denied out-of-workspace read, failed tool response, and Stop during a pending request each produce one terminal outcome with the relevant cause. A subsequent run can start immediately. A summary alone never enables playback or marks the mix successful.
- [ ] Schema, help and doctor alone cannot complete project validation. A successful and a failed Codex fixture both identify Audio Review and Codex in the inspector, summary and run record. Failure output names the unavailable input or failed operation as well as the missing WAV.
- [ ] With 982,031 total input tokens including 927,104 cached and 1,503 output, live and final accounting agree on 54,927 uncached input, 927,104 cached input and 1,503 output. Repeating the final notification leaves totals unchanged. A project with a supplied audio clip and no transcription request can mix when whisper is unavailable.

## Design notes

This concerns the intersection of FEAT-neat-osprey-DKK7 and FEAT-spry-alpaca-5RQZ. The synthetic fixture and diagnostic values above are sufficient for implementation and regression testing. The original project, credentials and run folder are unnecessary.

## Implementation log

### 2026-11-22

- Implemented: Reviewed the failed run and documented the evidence, local fixture and verification criteria.
- Not done: Runtime reproduction, code fixes and verification await approval.
- Files: This bug README.md.


