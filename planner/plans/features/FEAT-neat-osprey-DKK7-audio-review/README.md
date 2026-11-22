---
id: FEAT-neat-osprey-DKK7
type: feature
title: Audio Review
status: proposed
implementation: none
approved: yes
approved-on: 2026-11-21
priority: medium
created: 2026-11-21
updated: 2026-11-21
attachments: []
---

# Audio Review

## Summary

Before committing to a video render, the user can build the soundtrack from the current project spec, listen to it in Video Studio, and revise it. The review plays the assembled mix so the user can judge narration, pauses, music levels and effects together.

## Requirements

- [ ] Offer **Audio Review** alongside the existing run choices on macOS and Windows. Allow it as the first run for a project. If the project has multiple outputs, let the user select the output whose timing and audio settings to audition.
- [ ] Prepare a timeline and resolve the selected output's audio inputs from the spec, brand settings and attached media. Include audible source-clip tracks as well as voice-over, music and effects. Use the existing generation helper, cost confirmation, cache and credit cap when generated audio is needed.
- [ ] Provide a cutline audio-only entry point that accepts a project JSON5 file, the selected export and a destination WAV. Use the production audio plan for that export, including trims, offsets, gains, fades, ducking, silence and loudness processing. Preserve its start time and duration. Skip visual asset generation, screen recording, frame rendering and final video encoding.
- [ ] Save the playable WAV in the audio review run folder. Show the selected output, play/pause, seek position and duration in the run result. Reopening that result plays the saved file without starting another build.
- [ ] Accept revision feedback from an audio review and build a new review using the revised instructions. Reuse eligible cached audio inputs and retain the prior playable result so the user can compare takes.
- [ ] When the timeline contains no audible tracks, report that outcome before offering playback. For a missing required audio source or exceeded credit cap, identify the blocking input or limit. A stopped or failed build must not expose a partial WAV as a completed review.

## Acceptance criteria

- [ ] From a fresh project with a spec and local audio, complete Audio Review on each desktop platform before any production run. Play, pause, seek to the middle and reopen the result. Verify the run contains a playable WAV and the execution trace contains no visual render, capture or video encode.
- [ ] Use a local fixture with two exports of different lengths, a delayed voice track, ducked music and an offset effect. For each export, compare the decoded PCM of its review WAV with the production mix immediately before muxing. With matching PCM format, sample rate and channels, the samples and duration match.
- [ ] Change only the backing music gain and rebuild. Verify the new review reflects that gain, the narration generation count does not increase, and the first review still plays its original mix.
- [ ] Exercise an empty audio timeline, a missing referenced voice file, an over-budget generation request and cancellation during mixing. Each result reports its own outcome, offers no incomplete file for playback and leaves earlier reviews accessible.

## Design notes

This extends the run choices and revision flow in `ui/spec.md` and exposes the final mix described in `spec.md` §6. The reusable mix plan and final WAV are in `crates/cutline-render/src/audio_plan.rs`. Keep export resolution consistent with the production renderer. Carry the audio-review run choice through `crates/video-studio-core/src/run/` to the macOS and Windows run views so the workflow can start from a written spec as well as an existing timeline.

## Implementation log

### 2026-11-21

- Implemented: Defined the audio-only build, playback and revision workflow and its verification fixtures.
- Not done: Feature implementation and acceptance checks await approval.
- Files: This feature README.md.
