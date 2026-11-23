---
id: FEAT-neat-osprey-DKK7
type: feature
title: Audio Review
status: partial
implementation: partial
approved: no
priority: medium
created: 2026-11-21
updated: 2026-11-22
attachments: []
---

# Audio Review

## Summary

Before committing to a video render, the user can build the soundtrack from the current project spec, listen to it in Video Studio, and revise it. The review plays the assembled mix so the user can judge narration, pauses, music levels and effects together.

## Requirements

- [ ] Offer **Audio Review** alongside the existing run choices on macOS and Windows. Allow it as the first run for a project. If the project has multiple outputs, let the user select the output whose timing and audio settings to audition.
  - [x] Core and CLI: `RunKind::AudioReview` (`audio_review`), allowed as a project's first run (no dry run first); `RunRequest.audio_output` picks the output by ID or name, default the first; `video-studio run --audio-review [--audio-output NAME]`.
  - [ ] The run choice and output picker in the macOS app.
  - [ ] The run choice and output picker in the Windows app.
- [x] Prepare a timeline and resolve the selected output's audio inputs from the spec, brand settings and attached media. Include audible source-clip tracks as well as voice-over, music and effects. Use the existing generation helper, cost confirmation, cache and credit cap when generated audio is needed.
- [x] Provide a cutline audio-only entry point that accepts a project JSON5 file, the selected export and a destination WAV. Use the production audio plan for that export, including trims, offsets, gains, fades, ducking, silence and loudness processing. Preserve its start time and duration. Skip visual asset generation, screen recording, frame rendering and final video encoding.
- [ ] Save the playable WAV in the audio review run folder. Show the selected output, play/pause, seek position and duration in the run result. Reopening that result plays the saved file without starting another build.
  - [x] Core: the mix is saved as `outputs/audio-review.wav` in the run folder (under the library's `audio-reviews/<project>/`), and `run.results` (`load_results`) returns it as the run's one output with its duration, so reopening needs no build.
  - [ ] macOS run result: selected output, play/pause, seek position and duration.
  - [ ] Windows run result: the same.
- [ ] Accept revision feedback from an audio review and build a new review using the revised instructions. Reuse eligible cached audio inputs and retain the prior playable result so the user can compare takes.
  - [x] Core: a revised Audio Review gets its own user prompt (mix again with `cutline audio`, reuse unchanged audio through the helper's cache). The previous run's project.json5, audio/ and work/ are copied as for production, and the earlier WAV stays in its own folder.
  - [ ] Revise… on an Audio Review result in both apps, and an end-to-end check of a revised review.
- [ ] When the timeline contains no audible tracks, report that outcome before offering playback. For a missing required audio source or exceeded credit cap, identify the blocking input or limit. A stopped or failed build must not expose a partial WAV as a completed review.
  - [x] `cutline audio` reports a silent timeline (`audible: false`, "the timeline has no audible tracks"), and the run adds the warning "The timeline has no audible tracks: the mix is silence." `cutline audio` writes its WAV only when the whole mix succeeded. A run that stops or fails without the mix is `failed` and its result is `incomplete`, so nothing is offered for playback.
  - [ ] Naming a missing audio source or an exceeded credit cap as the blocking input in the app's result view. Today they reach the user through `cutline validate`'s `E_ASSET_NOT_FOUND` and the helper's credit-cap refusal in Claude's summary.

## Acceptance criteria

- [ ] From a fresh project with a spec and local audio, complete Audio Review on each desktop platform before any production run. Play, pause, seek to the middle and reopen the result. Verify the run contains a playable WAV and the execution trace contains no visual render, capture or video encode.
- [x] Use a local fixture with two exports of different lengths, a delayed voice track, ducked music and an offset effect. For each export, compare the decoded PCM of its review WAV with the production mix immediately before muxing. With matching PCM format, sample rate and channels, the samples and duration match.
- [ ] Change only the backing music gain and rebuild. Verify the new review reflects that gain, the narration generation count does not increase, and the first review still plays its original mix.
- [ ] Exercise an empty audio timeline, a missing referenced voice file, an over-budget generation request and cancellation during mixing. Each result reports its own outcome, offers no incomplete file for playback and leaves earlier reviews accessible.

## Design notes

This extends the run choices and revision flow in `ui/spec.md` and exposes the final mix described in `spec.md` §6. The reusable mix plan and final WAV are in `crates/cutline-render/src/audio_plan.rs`. Keep export resolution consistent with the production renderer. Carry the audio-review run choice through `crates/video-studio-core/src/run/` to the macOS and Windows run views so the workflow can start from a written spec as well as an existing timeline.

## Implementation log

### 2026-11-21

- Implemented: Defined the audio-only build, playback and revision workflow and its verification fixtures.
- Not done: Feature implementation and acceptance checks await approval.
- Files: This feature README.md.

### 2026-11-22

- Implemented:
  - **cutline:** `cutline audio project.json5 [--export NAME] --out mix.wav [--no-cache]` (`pipeline::prepare_audio` + `pipeline::render_audio`). It uses the production audio plan unchanged (`audio_plan::plan` and `execute`: trims, offsets, gains, fades, ducking, silence, two-pass loudness). Clip dialogue comes from audio-only segment renders, using the dialogue part of the same segment filter (`SegmentSpec::audio_filter`, `segment::audio_job`), cached under `.cutline/cache/audio-segments`. An export `range` is cut out as the final encode does. `extras::compute_audio` does only speech spans, SFX files and the script and pace checks: no HTML capture, reframing, frames, annotations, segment video or video encode. The WAV is written to a temp file and renamed only on success.
  - **Video Studio core:** `RunKind::AudioReview`:
    - Helper mode, budget and credit cap as in production.
    - Run folder under `<library>/audio-reviews/<project>/`.
    - The broker refuses `cutline render|review|preview|thumbnail|record` and app capture; `cutline audio` is allowed.
    - An AUDIO REVIEW section in the system prompt, plus user prompts for new and revised reviews.
    - Validation estimates.
    - The orchestrator collects `outputs/audio-review.wav` and fails a run without it.
    - `load_results` returns the WAV.
    - The project schema and the generated Windows types are updated.
  - **CLI:** `video-studio run --audio-review [--audio-output NAME]`.
- Verified:
  - `cutline-render --test audio_only`: a fixture with a full export and a 1.0–4.0 s cut-out, a voice-over starting at 1.0 s, ducked music with a fade, an SFX at 4.2 s and the clip's own sound. For each export the review WAV has the same length as, and samples within 1e-6 of, the production render's `final_wav` (cut to the range). No video segment is rendered. Changing only the music gain rebuilds the mix with the clip's sound reused from cache, and the take differs. An empty timeline gives `audible: false` and the right duration.
  - `video-studio-cli --test e2e_runs audio_review_builds_only_the_soundtrack` (fake Claude, fake cutline, fake ElevenLabs): TTS allowed; render, review, preview and capture denied; `cutline audio` allowed. The run succeeds with the WAV and no video, and reopening returns it without a build. A run without the mix fails, offers nothing to play, and leaves the first review in place.
  - All tests of video-studio-core, -cli and -ffi pass; the Windows UI builds; clippy and fmt are clean.
- Not done:
  - The macOS and Windows UI: run choice, output picker, player and Revise on Audio Review results. These need the desktop GUIs, which this unattended run can't drive or check.
  - Acceptance 1, 3 and 4 as written. They need the apps (1), a revised review counting narration generations end to end (3), and the over-budget and cancellation cases through the app (4).
  - Export `loop` crossfades are not applied to the review WAV (`range` is).
- Files: `crates/cutline-render/src/{pipeline.rs,segment.rs,extras.rs}`, `crates/cutline-cli/src/{main.rs,edit.rs}`, `crates/video-studio-core/src/{project.rs,validation.rs,assistant.rs,claude/broker.rs,run/{prepare.rs,prompt.rs,orchestrator.rs,collect.rs}}`, `crates/video-studio-cli/src/{main.rs,bin/fake_cutline.rs}`, `schema/cutproj.schema.json`, `apps/windows/ui/src/generated/project.ts`, and tests.
- Commit/PR: "Implement FEAT-neat-osprey-DKK7 (partial): Audio Review core and cutline audio" on branch `implement-approved-plans-2026-11-22`.
