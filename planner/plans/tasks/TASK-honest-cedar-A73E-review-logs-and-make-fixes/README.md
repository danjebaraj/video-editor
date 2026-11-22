---
id: TASK-honest-cedar-A73E
type: task
title: Fix capture, review and QA issues in vertical video production
status: todo
done: no
approved: yes
approved-on: 2026-11-21
priority: medium
created: 2026-11-21
updated: 2026-11-21
attachments: []
---

# Fix capture, review and QA issues in vertical video production

## Summary

A production run delivered three 45.7 s vertical videos with warnings. Sparse recordings triggered repeated review failures, and QA flagged visible text cards. Use the observations below and generate the fixtures locally. All project files, media, transcripts and expected results needed for verification can be constructed from this task.

## Findings

1. Four review attempts exited 4 with `error: frame grab at 0.900s failed: ` and no FFmpeg diagnostic. The clips used rectangular reframing. Switching to `fit_pad` plus a mask allowed review to finish.
2. Screen capture wrote frames only when the screen changed. One MOV had 25 frames over 7 s and another had 31 over 4.6 s. Sparse timing and rectangular reframing are the combination to exercise.
3. A scroll required twelve calls to `video-studio capture scroll 566 700 -1` about 0.8 s apart. Motion appeared stepped and began about 6 s into an 18 s capture.
4. A request for a 540×960 pt Safari window yielded a 1148×1898 pixel recording. The app enforced a minimum width of 574 pt, and the caller had to inspect an image to discover the actual geometry.
5. The script pronounced “One-oh-eight” and “thinkask dot A I”, while the transcript used “108” and “thinkask.ai”. The checker reported a 60% match on ten script words and a pace of 100 WPM, 32% slower than expected.
6. The opening HTML card displayed “He quoted half a verse.” from frame zero. QA warned that the first caption/text was at 1.32 s because its input considered caption boxes only.
7. Review flagged two black spans longer than 0.3 s on #0D0D0F text cards. It also reported no visual change from 4.0 to 11.5 s on a card with words fading in at 3.9 and 5.8 s.
8. Selecting Georgia produced `E_FONT_MISSING` at `/captions/font`. The operator used HTML for serif cards and Inter for captions.
9. An initial project omitted `version` and failed validation. Adding `version: 1` resolved it.
10. With only whisper missing, readiness described HTML graphics and reframing alongside speech captions as uses for the missing tool. The project supplied transcript timings.

Not proposed as fixes:
- The music prompt asked for 90 BPM and cutline detected 120 BPM. One run isn't enough to tell whether the music or the beat detector is wrong.
- The dry-run model is the same as the production model (a `dry_run_model` info). That's a setting the owner chose.
- The Luke 9:54–55 paragraph is missing from the article. That's a content issue on the article page.

## Steps

### A. Review crash on sparse recordings (findings 1, 2)

- [ ] `grab()` in `crates/cutline-qa/src/review.rs` no longer fails with an empty reason. If ffmpeg writes nothing to stdout, `grab()` tries again with `-ss` after `-i` (accurate seek), then at the last frame at or before `t`. If that still fails, the error names the file, `t`, the clip duration, the frame count and ffmpeg's stderr, or says "(ffmpeg printed nothing)".
- [ ] Find out why `reframe: {mode: "rect"}` on a VFR clip produces a file with no frame near 0.9 s, and fix it in the reframe path so the reframed clip has a frame for every output frame. Write down the cause in the Implementation log.
- [ ] Test in `crates/cutline-qa/tests/`: a 7 s VFR fixture with about 25 frames (`tests/fixtures/vfr/sparse-7s.mov`), used with `reframe: {mode: "rect", rect: {x: 0.03, y: 0.055, w: 0.94, h: 0.945}}`. `cutline review` exits 0 and writes `cuts/` and `contact_sheet.jpg`.
- [ ] After `record wait` finishes, `video-studio capture record` rewrites the recording as constant 30 fps (`ffmpeg -i in.mov -vf fps=30 -c:v libx264 -crf 18 -pix_fmt yuv420p`, written to a temp file, then renamed over `--out`). The rewrite happens in `crates/video-studio-core/src/capture/mod.rs` (`record_wait`), so it works the same on macOS and Windows. If the rewrite fails, the original file is kept and `record wait` prints a warning.
- [ ] Test: `cutline probe` on a recording made through `record wait` reports `vfr: false` and about 30 fps.

### B. Smooth scroll and honest window size (findings 3, 4)

- [ ] New command: `video-studio capture scroll <x> <y> <lines> --over <seconds>`. It spreads `lines` over `seconds`, one line per step at even intervals (at least 10 steps a second, using pixel-precise scroll events where the platform has them). Without `--over`, it behaves as it does today. `--over` must be between 0.1 and 60, otherwise it exits with an error. Change `Pointer::Scroll` in `capture/mod.rs` and the macOS and Windows code.
- [ ] Add `--over` to the broker's capture allowlist (`crates/video-studio-core/src/claude/broker.rs`, the `"scroll"` entry near line 458) and to the run prompt's capture help, with an example: `video-studio capture scroll 566 700 -12 --over 10`.
- [ ] `video-studio capture window --size WxH` prints the size and position the window actually got (`window: 574x960 at 80,40`). If that differs from the request by more than 2 pt in either direction, it adds `(asked for 540x960; the app's minimum is larger)`.
- [ ] Tests in `crates/video-studio-cli/tests/`: argument parsing for `--over` (valid, missing value, out of range) and the broker allowing `scroll … --over N`.

### C. Script check and voice pace (finding 5)

- [ ] Before comparing, the script check (`W_SCRIPT_MISMATCH`) puts both texts through the same cleanup: lower-case; spoken numbers made into digits ("one-oh-eight" → "108", "one hundred and eight" → "108"); "dot" between two words made into "."; single letters separated by spaces joined ("a i" → "ai"); punctuation stripped. "thinkask dot A I" and "thinkask.ai" then both become "thinkask.ai".
- [ ] When an asset has a `script`, `W_VOICE_PACE` counts the script's words, not the recognised words.
- [ ] Test with the `vo7` case: script "Read the whole dossier. One-oh-eight, at thinkask dot A I." against the transcript words "Read the whole dossier. 108, at thinkask.ai." gives no `W_SCRIPT_MISMATCH` and no `W_VOICE_PACE`. A real skipped word ("Read the dossier") still warns.

### D. QA and review false alarms (findings 6, 7)

- [ ] `hook`: if `first_text_at` is later than 1.0 s and the timeline has an HTML clip or layer covering 0–1 s, QA runs text recognition (the engine `cutline review` already uses: Vision on macOS, Tesseract elsewhere, `off` skips it) on the frame at 0.5 s. If it finds at least one word, the check passes with the message `text on screen at 0.50s (recognised)`. With recognition off or unavailable, the warning stays and adds `(text in HTML isn't checked)`.
- [ ] `black_frames`: a span doesn't count as black if text recognition finds a word in its middle frame, or if the frame's luma variance is above 0.0004 (the same limit `first_frame` uses).
- [ ] Review "No visual change": an HTML clip or layer whose seek function changes the picture inside the span (checked by comparing frames 0.5 s apart with a mean absolute difference above 0.5%) counts as a visual change.
- [ ] Tests in `crates/cutline-qa/tests/`: a 3 s render of a #0D0D0F HTML card with "He quoted half a verse." visible from frame 0 has no `hook` warning and no `black_frames` warning. A truly black 1 s gap between two clips still gives `black_frames`.

### E. Fonts, prompt and readiness text (findings 8, 9, 10)

- [ ] Bundle one open-licence serif font family (Source Serif 4 under the SIL OFL 1.1, regular, italic and bold) with cutline's fonts. `captions.font` and the text templates accept it by name. Add it to the licence list in spec.md §12.1 when the owner agrees (see Design notes).
- [ ] `E_FONT_MISSING` lists the bundled fonts in its hint, for example `bundled: Inter, …, Source Serif 4`.
- [ ] The run prompt's project example (`crates/video-studio-core/src/run/prompt.rs`) starts with `version: 1`.
- [ ] `cutline_optional` in `crates/video-studio-core/src/validation.rs` says what each missing tool is for, and only that. For whisper alone: "Not installed: whisper. It's only needed to make captions from speech when no transcript is supplied."
- [ ] Unit test for the `cutline_optional` text with only whisper missing.

## Local verification inputs

- **Capture and review.** Generate a 7 s MOV from 25 numbered frames with irregular presentation timestamps. Hold the same image across 0.900 s and confirm the duration and nonuniform frame spacing with ffprobe. Use the crop in step A on a 30 fps vertical timeline. For step B, use a local scrollable page with enough rows to scroll 12 lines. Test window-size reporting with requested 540×960 pt and a platform stub returning 574×960 pt at (80, 40).
- **Narration.** Use the exact script and recognized text in step C. Supply synthetic word timings spanning 4.0 s for the ten-word script (150 script words/minute) and select a 150 WPM target. Use the same duration for the skipped-word control. No speech service is required.
- **Visual QA.** Use white text on a #0D0D0F local HTML card and set the first timed caption to 1.32 s. Render the opening under the YouTube Shorts, Instagram Reels and TikTok presets. For the motion check, extend a card to 12 s and fade in a second line at 5.8 s. Check that the reported static intervals do not span that transition. Use step D’s black gap as the positive control.
- **Configuration.** Exercise step E with a minimal version-1 project, an unavailable font name and readiness inputs with only whisper missing. Use local fixtures or stubs for tool discovery.

## Done when

- [ ] Every box in Steps A–E is checked, or marked ` — dropped: <reason>` by the owner.
- [ ] `cargo test --workspace` passes. `cargo fmt --check` and `cargo clippy` show no new warnings compared with `main` before the change.
- [ ] From a clean checkout, run the local fixtures below through review, validation, QA and capture. Review writes the contact sheet, the matching narration passes both voice checks, each of the three vertical presets passes the opening hook check with recognition enabled, and one timed scroll call completes the full requested movement.
- [ ] The Implementation log names the commit(s).

## Design notes

- This task touches cutline (A, C, D, part of E) and Video Studio (A's CFR rewrite, B, part of E). If the owner prefers, A and D can be filed as bugs and B as a feature. The steps can be copied over unchanged.
- spec.md is affected in these places. Edit them only when the owner asks:
  - §6.3 Text & graphics: the bundled font list gains a serif.
  - §6.5 Transcription & captions: the script check's number and domain cleanup.
  - §7.2 Post-render QA: `hook` and `black_frames` use text recognition for HTML content.
  - §7.3 Review pack: frame grabs on VFR sources, and HTML changes count as visual changes.
  - §12.1 Licensing: add Source Serif 4 (SIL OFL 1.1).
- The CFR rewrite in step A makes recordings larger (about 2–4× for a 10 s clip at CRF 18). That's fine for clips of a few seconds. Long recordings may want a lower frame rate later.
- Text recognition adds about 0.2–0.5 s per checked frame. It runs only when the cheap checks fail, so passing renders don't pay for it.

## Implementation log

### 2026-11-21

- Implemented: Replaced external evidence and the original-project rerun with inline observations and local verification fixtures.
- Not done: Implementation and verification of steps A–E.
- Files: This task README.md.
