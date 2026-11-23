---
id: BUG-deft-lily-344X
type: bug
title: Audio Review helper reports a configured ElevenLabs key as missing and misreports two pending Codex patches
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

# Audio Review helper reports a configured ElevenLabs key as missing and misreports two pending Codex patches

## Summary

The owner confirmed the ElevenLabs key was configured and available as before, with no settings change. Audio Review passed validation, but its music helper reported “No ElevenLabs API key is set”. The cause of the lookup discrepancy is unverified. While two separate file changes were awaiting confirmation, the older change reached its 120-second deadline. The final diagnostic grouped all six paths under that deadline, said nothing subsequently ran and omitted the helper's actionable failure. No playable soundtrack was produced.

## Environment and evidence

Codex default resolved to `gpt-6.1-sol`, Default run config, YouTube Shorts, 45-second target, four narration passages and a music-only final five seconds. The following elapsed times are relative to run start. They identify the protocol sequence needed for a fixture.

| Elapsed | Recorded observation |
|---|---|
| Before start | Validation estimated 71 voice-over words and 1,854 ElevenLabs credits against a 20,000 cap. It returned zero warnings or blockers and one informational finding about optional whisper transcription. |
| 21 s | File-change approval request ID 1 covered four additions, `audio/scene1.txt` through `audio/scene4.txt`. The app replied `accept` and displayed “Writing”. The trace contains no matching completion. |
| 70–98 s | Doctor succeeded, the broker declined `ls audio`, and a music quote succeeded. The quote did not establish credential availability. |
| 106 s | `video-studio eleven music --prompt "…" --length 49 --out music.mp3` exited 1 on a cache miss. The helper recorded “No ElevenLabs API key is set”, 1,470 quoted credits, zero charged, zero bytes and no output. The assistant reported no attached audio fallback. |
| 130 s | Audio was marked successful. File-change approval request ID 7 covered `project.json5` and `summary.md`. The app replied `accept`. No completion was recorded for this second item either. |
| 142 s | Exit 3. The final message described all six files as one change pending for 120 s, although the second item had been pending about 12 s. It claimed “nothing after them ran” despite the helper invocation. The host summary correctly reported zero credits and missing `outputs/audio-review.wav`. |

## Steps to reproduce

1. Create a temporary project with one Shorts output. Request four short spoken passages over a 45-second music bed, ending with five seconds of music alone. Configure generation through ElevenLabs with no attached audio, an empty cache, a 20,000-credit limit and a persistent test secret store seeded once through the Settings credential path. Confirm the host can retrieve the entry. Run Audio Review through the normal Codex-launched helper against a local ElevenLabs fixture service. Keep the same stored credential throughout; leave the helper’s test-key override unset.
2. Separately exercise an execution-time failure: keep the stored entry available to the host and simulate an empty or failed lookup only in the launched helper. Run against the fake Codex app server and local helper fixtures. Send one file-change item with the four scene paths, approve it, and withhold its completion. While it is pending, have the quote succeed and music generation return the missing-key error above.
3. Just before the first deadline, send a second file-change item with distinct item and request IDs for the timeline and summary. Withhold its completion. Advance an injected clock or short test deadline past the first deadline only. Inspect progress, structured run result and summary.

## Expected result

The launched helper retrieves the existing credential and generates music without asking the user to re-enter it. If helper lookup fails while the entry exists, the run identifies the failed lookup and preserves the underlying diagnostic. A patch timeout identifies the expired item and accurately distinguishes other pending work. Approval alone leaves the on-disk state unverified.

## Actual result

The helper reported a missing key despite the owner's confirmed configuration. The final message omitted that discrepancy, attributed the age of the first patch to the second, and the Audio stage showed success despite the failed music request.

## Fix checklist

- [ ] Compare the Settings and validation credential lookup with the launched helper's `secrets.get(ELEVEN_KEY)` in `crates/video-studio-core/src/eleven/mod.rs`. Check the store backend, entry identifier, process identity and execution context. Distinguish an empty result from a store access error. Verify through the production lookup path without using `test_api_key` or putting the credential in agent context or logs.
- [ ] In `crates/video-studio-core/src/codex/agent.rs`, track approval time, paths and completion by file-change item. At the first deadline identify exactly which item expired. If stopping abandons a younger item, record it separately. Investigate the missing completion against the supported app-server protocol, preserving the deadline and keeping unconfirmed writes pending. See BUG-green-cricket-P6HQ for the single-item case.
- [ ] In `run/orchestrator.rs` and the summary collection path, retain the structured generation error and credential lookup context alongside any terminal patch timeout. Derive stage outcomes from confirmed results. Preserve actual helper attempts, credit usage and missing-WAV status.

## Verification

- [ ] With the same saved test credential throughout and no helper override, the local service accepts the helper's authenticated request and returns audio. Simulate a helper-only lookup failure: the inspector identifies that operation, avoids instructing the user to replace a configured key, and records zero spend. Retain a separate truly unconfigured control and the local-audio-only control.
- [ ] Replay both items using distinct IDs. Only the older is labeled timed out. Repeat with its successful completion just before expiry: its timer clears, the newer item keeps its own deadline, and neither item emits duplicate completion or terminal events.
- [ ] Verify the inspector and generated summary name the failed music operation and an actionable remedy, report the patch states accurately, and offer no playback for the missing WAV. With a stub credential, a local helper returning a generated tone and confirmed file completions, the same workflow produces a decodable WAV without a paid service.

## Implementation log

### 2026-11-23

- Implemented: Documented the run evidence, both failure paths and self-contained verification fixtures.
- Not done: Runtime reproduction, fixes and verification await approval.
- Files: This bug README.md.

### 2026-11-23 — Credential clarification

- Updated the reproduction and acceptance criteria to use an existing saved key and exercise the launched helper’s lookup. Runtime verification awaits approval.
