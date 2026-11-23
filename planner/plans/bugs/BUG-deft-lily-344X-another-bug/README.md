---
id: BUG-deft-lily-344X
type: bug
title: Audio Review helper reports a configured ElevenLabs key as missing and misreports two pending Codex patches
status: fixed
fix: fixed
verified: yes
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

- [x] Compare the Settings and validation credential lookup with the launched helper's `secrets.get(ELEVEN_KEY)` in `crates/video-studio-core/src/eleven/mod.rs`. Check the store backend, entry identifier, process identity and execution context. Distinguish an empty result from a store access error. Verify through the production lookup path without using `test_api_key` or putting the credential in agent context or logs.
- [x] In `crates/video-studio-core/src/codex/agent.rs`, track approval time, paths and completion by file-change item. At the first deadline identify exactly which item expired. If stopping abandons a younger item, record it separately. Preserve the deadline and keep unconfirmed writes pending. See BUG-green-cricket-P6HQ for the single-item case.
- [x] Check `fileChange` completions against a new run with the real CLI: codex-cli 0.171.0 sent `item/completed` for all four approved file changes, and the driver matched each one.
- [ ] Explain the missing completions in the original run; that needs its `logs/codex.jsonl`, since the new run didn't reproduce them.
- [x] In `run/orchestrator.rs` and the summary collection path, retain the structured generation error and credential lookup context alongside any terminal patch timeout. Derive stage outcomes from confirmed results. Preserve actual helper attempts, credit usage and missing-WAV status.

- [x] Found by the real run: when ElevenLabs answers 401 because a key lacks one permission, name that permission and how to turn it on instead of saying the key was rejected.
- [x] Found by the real run: block an Audio Review when the cutline found has no `cutline audio`, and prefer a build that has it.
- [x] Found by the real run: `cutline beats` failed in Codex's sandbox because it couldn't write cutline's global cache. Cache writes are now best effort.
- [x] Make a stale cutline detectable: cutline reports a fingerprint of the source it was built from, and validation blocks a run when it isn't the one this Video Studio was built with.

## Verification

- [x] With the same saved test credential throughout and no helper override, the local service accepts the helper's authenticated request and returns audio. Simulate a helper-only lookup failure: the inspector identifies that operation, avoids instructing the user to replace a configured key, and records zero spend. Retain a separate truly unconfigured control and the local-audio-only control.
- [x] Repeat the saved-key run with the real Codex CLI on macOS, whose sandbox the fake only imitates: the helper's calls reach ElevenLabs with the saved key through the host relay, and narration is generated.
- [x] Repeat it with a key that has the `music_generation` permission and a cutline with `cutline audio`, and confirm music and a mixed `outputs/audio-review.wav`.
- [x] Replay both items using distinct IDs. Only the older is labeled timed out. Repeat with its successful completion just before expiry: its timer clears, the newer item keeps its own deadline, and neither item emits duplicate completion or terminal events.
- [x] Verify the inspector and generated summary name the failed music operation and an actionable remedy, report the patch states accurately, and offer no playback for the missing WAV. With a stub credential, a local helper returning a generated tone and confirmed file completions, the same workflow produces a decodable WAV without a paid service.

## Implementation log

### 2026-11-23

- Implemented: Documented the run evidence, both failure paths and self-contained verification fixtures.
- Not done: Runtime reproduction, fixes and verification await approval.
- Files: This bug README.md.

### 2026-11-23 — Credential clarification

- Updated the reproduction and acceptance criteria to use an existing saved key and exercise the launched helper’s lookup. Runtime verification awaits approval.

### 2026-11-23 — Fix

- Root cause (credential): Codex runs every command in its sandbox, approved or not. On macOS, `codex sandbox --log-denials` with codex-cli 0.171.0 showed the helper's Keychain lookup (`video-studio secret status`) denied as `mach-lookup com.apple.SecurityServer`. The `keyring` crate reports that as "no entry", so the helper said no key was set. The same sandbox also denies outbound network (`curl` to api.elevenlabs.io failed inside it and succeeded outside), so the helper couldn't have called ElevenLabs even with the key. Validation runs in the host, outside the sandbox, which is why it saw the key. Commands run inside the sandbox carry `CODEX_SANDBOX=seatbelt` and `CODEX_SANDBOX_NETWORK_DISABLED=1`.
- Implemented: the ElevenLabs relay (`run/relay.rs`). In Codex runs the helper context names `logs/relay/`. A helper that finds itself in Codex's sandbox writes its arguments there. The host serves that folder for the length of the run and runs the same `video-studio eleven …` command outside the sandbox, with the run context. It passes back the output and exit code. Only `eleven` subcommands are relayed, in the run folder or a folder inside it, and the credit cap is still enforced by the helper. The key never enters the sandbox or the agent's view, and the call is logged once in `helper.ndjson`.
- Implemented: `eleven::stored_key` now tells three cases apart: a key-store error ("couldn't read … not a missing key"), a lookup from inside the sandbox ("can't reach the system key store … don't ask the user to add it again"), and a key that really isn't set (unchanged text). `eleven voices` uses it too.
- Implemented: in the Codex driver, each accepted file change keeps its own acceptance time. Only a change past its own deadline is said to have timed out. A younger change still waiting is named apart, with its age, as abandoned and unconfirmed. The claim "nothing after them ran" is gone. A change completed in time clears only its own timer.
- Implemented: in the orchestrator, a stage's failure is cleared only by a later success of the same operation (`video-studio eleven music` isn't cleared by a quote or another command). ElevenLabs calls that failed and weren't made again are added to the outcome with their own error, beside a patch timeout, which also reaches `logs/summary.md`. When the run succeeded, they become a warning instead. The orchestrator no longer repeats paths the patch deadline already named.
- Tests: `a_sandboxed_codex_helper_makes_its_elevenlabs_call_through_the_host`, `only_the_expired_codex_patch_times_out_and_the_music_failure_is_kept` and `a_codex_patch_completed_in_time_clears_only_its_own_deadline` (in `tests/codex_runs.rs`); `a_sandboxed_helper_says_the_key_store_is_out_of_reach_not_that_the_key_is_missing` (in `tests/e2e_tools.rs`); unit tests for the relay and for `operation`. The fake Codex gained `"sandboxed": true` commands (sandbox markers and an empty key store), `sleep` and `finish_patch`.
- Not done: the missing `fileChange` completion itself is not explained; without the failing run's `codex.jsonl` it isn't clear whether Codex never sent it or sent it in a shape the driver doesn't match. A real-CLI run with a saved key hasn't been made (this machine has no saved key). The bug stays `verified: no` until both are done.
- Design notes: this changes how the spec's helper reaches ElevenLabs in Codex runs: the call is made by the host's copy of the same binary, so the "the key is touched by one binary" decision still holds. `spec.md` hasn't been edited.
- Files: `crates/video-studio-core/src/run/relay.rs`, `run/prepare.rs`, `run/orchestrator.rs`, `eleven/mod.rs`, `codex/agent.rs`, `crates/video-studio-cli/src/eleven_cmd.rs`, `src/main.rs`, `src/bin/fake_codex.rs`, `tests/codex_runs.rs`, `tests/e2e_tools.rs`.

### 2026-11-23 — Real Codex run

- Ran an Audio Review (two spoken lines, 15 s of generated music, 2,000-credit cap) with codex-cli 0.171.0, the saved ElevenLabs key from the system keychain, no test-secret or base-URL override, and a scratch library.
- Verified: the helper's calls ran inside Codex's sandbox and went through the host relay. Both `eleven tts` calls succeeded with the saved key (18 and 10 credits, 28 in all, matching the run total), and the relay folder ended with only its `closed` marker. The key appears nowhere in the run folder, `codex.jsonl` or the event stream.
- Verified: the real CLI sent `item/completed` for all four approved file changes, and no patch deadline fired. The missing completions of the original run weren't reproduced.
- Verified: failed calls kept their own errors in the run outcome and `logs/summary.md`.
- Not verified: music and the mixed WAV.
  - ElevenLabs answered 401 because the key isn't allowed music, transcripts or the voice list: the `music_generation`, `speech_to_text` and `voices_read` permissions are missing. The helper's message still says "the API key was rejected", which is misleading when only one permission is missing.
  - The repository's `bin/cutline` 0.1.0 has no `audio` subcommand, so no `outputs/audio-review.wav` could be mixed, and nothing flagged that before the run.
  - Both are outside this bug and the bug stays `verified: no`.

### 2026-11-23 — Follow-ups from the real run

- Implemented: an ElevenLabs 401 that names a missing permission now reads "this API key works, but it isn't allowed `music_generation`. Turn that permission on for the key in ElevenLabs (Developers › API Keys), or save a key without restrictions in Settings › ElevenLabs", followed by ElevenLabs' own text. A 401 for an invalid key still says the key was rejected. `Client::missing_permission` and Settings › Test read the permission the same way as before.
- Implemented: `CutlineInfo` records whether a cutline has `cutline audio` (`audio`, from `cutline audio --help`, like `still`). `find_cutline` prefers a build with both `still` and `audio`. Validation adds the blocker `cutline_no_audio` for an Audio Review when the cutline found lacks it; production runs aren't affected. The FFI re-asks the cutline when the shell sends `audio: false` or leaves it out (as for `still`, BUG-0003), and the macOS and Windows models carry the field.
- Tests: `a_key_without_a_permission_is_not_an_invalid_key` (extended), `find_cutline_prefers_one_with_audio`, the validation environment test (Audio Review blocked, production not), and the FFI recheck test (extended). The fake cutline answers `cutline audio --help`. The workspace suite passes (552 tests).
- Not done: this machine's `bin/cutline` (and `~/.local/bin/cutline`, which links to it) is a 0.1.0 build from 2026-11-19 without `cutline audio`; `scripts/build-bin.sh` rebuilds it. The saved key still lacks `music_generation`, so the music half of the real-CLI verification is still open.

### 2026-11-23 — Stale cutline builds, and the full real run

- Implemented: cutline's version stays 0.1.0 as commands are added, so a stale build looked current. A build script shared by `cutline-cli` and `video-studio-core` (`crates/cutline-cli/source_fingerprint.rs`) fingerprints the cutline source: every `crates/cutline-*` crate's `Cargo.toml`, `build.rs` and `src/`, and `assets/`, with line endings normalized. cutline reports its fingerprint with the hidden `cutline build-info --json`. Video Studio builds in the one it expects (`locate::EXPECTED_CUTLINE_SOURCE`).
  - `find_cutline` prefers the build with the expected fingerprint.
  - Validation adds the blocker `cutline_stale` for any other build, older or newer, or one without `build-info`. Its fix names `scripts/build-and-release.sh` in the repo the cutline came from, or in Settings › cutline's repo.
  - `video-studio cutline` reports `current` and `expected_source`, and `apps/macos/build.sh` checks `current` after building cutline.
  - The FFI re-asks the cutline when the shell leaves out `source`, and the macOS and Windows models carry it.
- Tests: the validation environment test (stale and pre-`build-info` builds block every run; one blocker, not two), the FFI recheck test, and the fake cutline's `build-info` (`FAKE_CUTLINE_SOURCE` overrides it). The workspace suite passes (552 tests).
- Rebuilt `bin/cutline` with `scripts/build-and-release.sh`. `video-studio cutline` reported the old build as out of date before the rebuild, and reports the new one as current.
- Verified with the real Codex CLI, the saved key (now with `music_generation`) and the rebuilt cutline:
  - The relayed `eleven music` call generated the bed (570 credits), and the narration came from the cache.
  - `outputs/audio-review.wav` was mixed: 15.00 s, 48 kHz stereo float, about −14 LUFS.
  - The run succeeded with warnings. The only warning is the key's missing `speech_to_text`, and its message names the permission.
  - The relay ended with only `closed`, and the key appears in none of the logs.
  - With every verification item passing, `verified` is now `yes`.
- Found: `cutline beats project.json5` failed with "Operation not permitted (os error 1)". Codex ran it without asking, as a safe command, inside its sandbox, so beat alignment couldn't be checked (fixed below).
- Still open: why the original run's `fileChange` items never completed. Neither real run reproduced it, and only the original run's `logs/codex.jsonl` can explain it.

### 2026-11-23 — cutline beats in the sandbox, and install.sh

- Verified: with the key's `speech_to_text` permission turned on, a real run made every ElevenLabs call (music, two voice lines, two transcripts) and succeeded with no warnings.
- Root cause (`cutline beats`): beat analysis is cached in cutline's global cache (`<cutline home>/cache/beats`, here `.cutline-home/cache` in the repo). Inside Codex's sandbox only the run folder is writable, so saving the result failed and the whole command failed with it. A sandboxed run reproduced it, and a run with the cache already filled didn't.
- Implemented: `Cache::store_json` saves when it can and logs a warning when it can't. Beats, transcripts (whisper and imported Scribe), silence detection, reframe analysis, demo activity and OCR all use it now, so an unwritable cache never fails the work it caches. Reads from the global cache still work in the sandbox.
- Tests: `a_cache_that_cant_be_written_doesnt_fail_the_caller` (cutline-media). The workspace suite passes (553 tests).
- Implemented: `install.sh` in the repo root does the whole install:
  1. Rebuilds cutline into `bin/` and links `~/.local/bin/cutline`.
  2. Installs cutline's tools when `cutline doctor` finds required ones missing.
  3. Builds the release app with its helper, quits a running copy and installs it as `/Applications/Syncfusion Video Studio.app`.
  4. Checks that the installed app uses that cutline, built from the same sources.

  It replaces `scripts/install-video-studio-app.sh`, which is removed, and the README describes it.
- Verified: `./install.sh` ran end to end on this Mac. A real Audio Review with the installed app's helper then succeeded with no warnings: music (570 credits), cached voice and transcripts, and a 15.00 s WAV. Inside the sandbox, `cutline beats` exited 0 with a "couldn't save to the cache" warning.
- Not done: whisper transcription inside the sandbox would still write its work folder under the global cache (`cache/transcripts/work`). whisper isn't installed here, so it's untested.

