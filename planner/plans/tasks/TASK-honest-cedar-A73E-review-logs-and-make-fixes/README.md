---
id: TASK-honest-cedar-A73E
type: task
title: Fix capture, review and QA issues in vertical video production
status: in-progress
done: no
approved: yes
approved-on: 2026-11-21
priority: medium
created: 2026-11-21
updated: 2026-11-22
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

- [x] `grab()` in `crates/cutline-qa/src/review.rs` no longer fails with an empty reason. If ffmpeg writes nothing to stdout, `grab()` tries again with `-ss` after `-i` (accurate seek), then at the last frame at or before `t`. If that still fails, the error names the file, `t`, the clip duration, the frame count and ffmpeg's stderr, or says "(ffmpeg printed nothing)".
- [x] Find out why `reframe: {mode: "rect"}` on a VFR clip produces a file with no frame near 0.9 s, and fix it in the reframe path so the reframed clip has a frame for every output frame. Write down the cause in the Implementation log.
- [x] Test in `crates/cutline-qa/tests/`: a 7 s VFR fixture with about 25 frames (`tests/fixtures/vfr/sparse-7s.mov`), used with `reframe: {mode: "rect", rect: {x: 0.03, y: 0.055, w: 0.94, h: 0.945}}`. `cutline review` exits 0 and writes `cuts/` and `contact_sheet.jpg`.
- [x] After `record wait` finishes, `video-studio capture record` rewrites the recording as constant 30 fps (`ffmpeg -i in.mov -vf fps=30 -c:v libx264 -crf 18 -pix_fmt yuv420p`, written to a temp file, then renamed over `--out`). The rewrite happens in `crates/video-studio-core/src/capture/mod.rs` (`record_wait`), so it works the same on macOS and Windows. If the rewrite fails, the original file is kept and `record wait` prints a warning.
- [x] Test: `cutline probe` on a recording made through `record wait` reports `vfr: false` and about 30 fps.

### B. Smooth scroll and honest window size (findings 3, 4)

- [ ] New command: `video-studio capture scroll <x> <y> <lines> --over <seconds>`. It spreads `lines` over `seconds`, one line per step at even intervals (at least 10 steps a second, using pixel-precise scroll events where the platform has them). Without `--over`, it behaves as it does today. `--over` must be between 0.1 and 60, otherwise it exits with an error. Change `Pointer::Scroll` in `capture/mod.rs` and the macOS and Windows code.
  - [x] Shared part: `Pointer::Scroll { …, over }`, the `--over` flag, the 0.1–60 s check, the step plan (`scroll_plan`: at least one step per line and 10 a second, evenly spaced, adding up to the whole movement) and the fake backend.
  - [x] macOS: pixel-unit scroll events (10 px a line) posted on the plan; builds and the app builds. Not watched on screen.
  - [ ] Windows: high-resolution wheel deltas (fractions of 120) posted on the plan. Type-checks and is clippy-clean for `x86_64-pc-windows-msvc` (2026-11-22); not yet run on Windows.
- [x] Add `--over` to the broker's capture allowlist (`crates/video-studio-core/src/claude/broker.rs`, the `"scroll"` entry near line 458) and to the run prompt's capture help, with an example: `video-studio capture scroll 566 700 -12 --over 10`.
- [x] `video-studio capture window --size WxH` prints the size and position the window actually got (`window: 574x960 at 80,40`). If that differs from the request by more than 2 pt in either direction, it adds `(asked for 540x960; the app's minimum is larger)`.
- [x] Tests in `crates/video-studio-cli/tests/`: argument parsing for `--over` (valid, missing value, out of range) and the broker allowing `scroll … --over N`.

### C. Script check and voice pace (finding 5)

- [x] Before comparing, the script check (`W_SCRIPT_MISMATCH`) puts both texts through the same cleanup: lower-case; spoken numbers made into digits ("one-oh-eight" → "108", "one hundred and eight" → "108"); "dot" between two words made into "."; single letters separated by spaces joined ("a i" → "ai"); punctuation stripped. "thinkask dot A I" and "thinkask.ai" then both become "thinkask.ai".
- [x] When an asset has a `script`, `W_VOICE_PACE` counts the script's words, not the recognised words.
- [x] Test with the `vo7` case: script "Read the whole dossier. One-oh-eight, at thinkask dot A I." against the transcript words "Read the whole dossier. 108, at thinkask.ai." gives no `W_SCRIPT_MISMATCH` and no `W_VOICE_PACE`. A real skipped word ("Read the dossier") still warns.

### D. QA and review false alarms (findings 6, 7)

- [x] `hook`: if `first_text_at` is later than 1.0 s and the timeline has an HTML clip or layer covering 0–1 s, QA runs text recognition (the engine `cutline review` already uses: Vision on macOS, Tesseract elsewhere, `off` skips it) on the frame at 0.5 s. If it finds at least one word, the check passes with the message `text on screen at 0.50s (recognised)`. With recognition off or unavailable, the warning stays and adds `(text in HTML isn't checked)`.
- [x] `black_frames`: a span doesn't count as black if text recognition finds a word in its middle frame, or if the frame's luma variance is above 0.0004 (the same limit `first_frame` uses).
- [x] Review "No visual change": an HTML clip or layer whose seek function changes the picture inside the span (checked by comparing frames 0.5 s apart with a mean absolute difference above 0.5%) counts as a visual change.
- [x] Tests in `crates/cutline-qa/tests/`: a 3 s render of a #0D0D0F HTML card with "He quoted half a verse." visible from frame 0 has no `hook` warning and no `black_frames` warning. A truly black 1 s gap between two clips still gives `black_frames`.

### E. Fonts, prompt and readiness text (findings 8, 9, 10)

- [x] Bundle one open-licence serif font family (Source Serif 4 under the SIL OFL 1.1, regular, italic and bold) with cutline's fonts. `captions.font` and the text templates accept it by name. Add it to the licence list in spec.md §12.1 when the owner agrees (see Design notes).
- [x] `E_FONT_MISSING` lists the bundled fonts in its hint, for example `bundled: Inter, …, Source Serif 4`.
- [x] The run prompt's project example (`crates/video-studio-core/src/run/prompt.rs`) starts with `version: 1`.
- [x] `cutline_optional` in `crates/video-studio-core/src/validation.rs` says what each missing tool is for, and only that. For whisper alone: "Not installed: whisper. It's only needed to make captions from speech when no transcript is supplied."
- [x] Unit test for the `cutline_optional` text with only whisper missing.

## Local verification inputs

- **Capture and review.** Generate a 7 s MOV from 25 numbered frames with irregular presentation timestamps. Hold the same image across 0.900 s and confirm the duration and nonuniform frame spacing with ffprobe. Use the crop in step A on a 30 fps vertical timeline. For step B, use a local scrollable page with enough rows to scroll 12 lines. Test window-size reporting with requested 540×960 pt and a platform stub returning 574×960 pt at (80, 40).
- **Narration.** Use the exact script and recognized text in step C. Supply synthetic word timings spanning 4.0 s for the ten-word script (150 script words/minute) and select a 150 WPM target. Use the same duration for the skipped-word control. No speech service is required.
- **Visual QA.** Use white text on a #0D0D0F local HTML card and set the first timed caption to 1.32 s. Render the opening under the YouTube Shorts, Instagram Reels and TikTok presets. For the motion check, extend a card to 12 s and fade in a second line at 5.8 s. Check that the reported static intervals do not span that transition. Use step D’s black gap as the positive control.
- **Configuration.** Exercise step E with a minimal version-1 project, an unavailable font name and readiness inputs with only whisper missing. Use local fixtures or stubs for tool discovery.

## Done when

- [ ] Every box in Steps A–E is checked, or marked ` — dropped: <reason>` by the owner.
- [x] `cargo test --workspace` passes. `cargo fmt --check` and `cargo clippy` show no new warnings compared with `main` before the change.
- [ ] From a clean checkout, run the local fixtures below through review, validation, QA and capture. Review writes the contact sheet, the matching narration passes both voice checks, each of the three vertical presets passes the opening hook check with recognition enabled, and one timed scroll call completes the full requested movement.
- [x] The Implementation log names the commit(s).

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

### 2026-11-22

- Implemented:
  - **A1:** `grab()` falls back to an accurate seek and then the last frame at or before `t`, and its error names the file, `t`, the duration, the frame count and ffmpeg's stderr, or "(ffmpeg printed nothing)". Shared with TASK-silver-cherry-R3MZ.
  - **A2, the cause:** on a sparse recording, `-ss t -i` gives the first stored frame at or after `t`. Past the last stored frame (still inside the file's duration) ffmpeg writes nothing and exits 0, which is the empty `frame grab at 0.900s failed: `. Review grabs the raw source only for the reframe panels, which is why it showed up with reframing. The reframe path itself already holds sparse frames: a 25-frame, 7 s capture with `reframe: {mode: "rect", rect: {x: 0.03, y: 0.055, w: 0.94, h: 0.945}}` renders a 210-frame, 7.000 s, 30 fps segment. So the fix is in the grab: the reframe panels now use the frame on screen (`grab_source`, the held frame for sparse sources).
  - **A3:** `crates/cutline-qa/tests/vfr_review.rs::review_finishes_on_sparse_captures` generates `tests/fixtures/vfr/sparse-7s.mov` in a temp folder (25 frames, a hold across 0.9 s, checked with ffprobe) instead of committing a binary. `cutline review` (`review_with`, OCR off) succeeds and writes `cuts/` and `contact_sheet.jpg`.
  - **A4/A5:** `record_wait` rewrites the recording with `ffmpeg -vf fps=30 -c:v libx264 -crf 18 -pix_fmt yuv420p` (audio copied) to a temp file, then renames it over the original. If the rewrite fails it keeps the file and warns. ffmpeg comes from `CUTLINE_FFMPEG` or `cutline deps path ffmpeg`; that lookup moved from `capture/windows.rs` to `capture/mod.rs` so both platforms share it. The test `capture::tests::record_wait_rewrites_recordings_at_a_constant_frame_rate` gives a sparse 25-frame recording to the fake recorder. After `record wait`, cutline's probe reports `vfr: false`, 30 fps and 7.0 s. A broken file is kept byte for byte and the reply warns.
  - **B:**
    - `capture scroll … --over <seconds>` (sub-boxes above).
    - Prompt help with the example `video-studio capture scroll 566 700 -12 --over 10`.
    - The broker checks only subcommands, so `scroll … --over N` is already allowed; this is now tested.
    - `capture window --size` prints `window: 574x960 at 80,40 (asked for 540x960; the app's minimum is larger)`, and the prompt says to plan on the size the window actually got.
    - Tests: `capture::tests::{timed_scrolls_are_even_and_complete, window_reports_the_size_it_got}`, `e2e_tools::a_timed_scroll_is_parsed_and_bounded` (valid; missing value exits 2; 0, 0.05, 61 and `abc` refused), `broker` capture allowlist.
  - **C:** `cutline-transcribe` check now cleans up both sides' spoken units the same way:
    - Spoken digits such as "one-oh-eight" and "one oh eight" become the number.
    - "and" inside a number is dropped.
    - Runs of single letters are joined ("a i" → "ai").
    - "dot" between two words joins them.

    The 70% pre-check now uses `check::coverage` on those units instead of word alignment. `W_VOICE_PACE` counts the script's words and letters when there is a script (timing still comes from the transcript). The test `script_captions::spoken_numbers_and_domains_pass_the_script_and_pace_checks` uses three neighbouring passages and vo7 (10 script words over 4.0 s, 150 wpm): no `W_SCRIPT_MISMATCH` and no `W_VOICE_PACE`. On the old code the same test gives exactly the reported "only 60% of the script's 10 words were heard in `vo7` (4 missing, 0 extra in the audio)" plus "`vo7` is spoken 25% slower … (105 words a minute)". The skipped-word control ("Read the dossier") still warns that "whole" was not heard.
  - **D:**
    - `hook` reads the frame at 0.5 s when an HTML/Lottie/SVG clip or layer is on screen there; with recognition off or unavailable it adds "(text in HTML isn't checked)".
    - `black_frames` clears a span only when each of three sample frames (at 25, 50 and 75 %, not just the middle one) has luma variance > 0.0004 or a recognised word. That way a caption arriving over a black opening doesn't clear the black part before it.
    - Review's "No visual change" checks HTML-covered gaps in the rendered preview, comparing frames 0.5 s apart with a mean absolute difference > 0.5 %. On a 12 s #0D0D0F card with lines fading in at 3.9 and 5.8 s, the edit-only gap was (0, 12); with the preview checked, only (6.0, 12.0) is reported.
    - Tests: `opening_qa::{near_black_card_versus_a_black_gap, a_visible_dark_title_card_is_not_black_and_counts_as_the_hook, an_animated_html_card_is_a_visual_change}`.
  - **E2–E5:**
    - `E_FONT_MISSING` already listed the bundled fonts in its hint; there is now a test (Georgia → `bundled: Anton, Inter`).
    - The run prompt's project example starts with `"version": 1` and says so; tested in `production_prompt_carries_everything_the_spec_lists`.
    - `cutline_optional` now says what each missing tool is for, and only that. For whisper alone: "Not installed: whisper. It's only needed to make captions from speech when no transcript is supplied." Unit tested.
- Local fixture results:
  - `cutline review` on the 7 s rect capture: exit 0, contact sheet written.
  - Narration: both voice checks pass.
  - The 3 s #0D0D0F "He quoted half a verse." card with the first caption at 1.32 s, rendered with `youtube_shorts`, `instagram_reels` and `tiktok` exports (all `shorts`, 1080×1920): every export's `hook` reports "text on screen at 0.50s (recognised)" (Vision), with no black span.
  - Positive control: a 1 s black gap between two clips still gives `black_frames` at 3.0 s.
- Checks: `cargo test --workspace --no-fail-fast` passes (exit 0). `cargo fmt --all --check` is clean. `cargo clippy --workspace --tests` shows only the existing `xtask` manifest lints warning.
- Not done:
  - **E1, Source Serif 4:** needs the font files downloaded from the internet, which this unattended run may not do without the owner's permission. Adding it to spec.md §12.1 also waits on the owner.
  - **Windows timed scroll:** not compiled or run here.
  - **Done when, timed scroll:** "one timed scroll call completes the full requested movement" needs a real screen and app; the step plan's sum is unit tested.
  - **Done when, real recording:** the CFR rewrite was tested through `record wait` with the fake recorder, not a real screen recording.
- Files: `crates/cutline-qa/src/{review.rs,qa_media.rs,qa.rs,deliver.rs}`, `crates/cutline-transcribe/src/check.rs`, `crates/cutline-render/src/auto.rs`, `crates/video-studio-core/src/capture/{mod.rs,macos.rs,windows.rs}`, `crates/video-studio-core/src/{validation.rs,run/prompt.rs,claude/broker.rs}`, `crates/video-studio-cli/src/capture_cmd.rs`, and tests in `crates/cutline-qa/tests/`, `crates/cutline-render/tests/script_captions.rs`, `crates/cutline-core/tests/invalid_projects.rs` and `crates/video-studio-cli/tests/e2e_tools.rs`.
- Commit/PR: "Implement TASK-honest-cedar-A73E: capture, review and QA fixes for vertical video" on branch `implement-approved-plans-2026-11-22`; the review/QA part is in the TASK-silver-cherry-R3MZ commit before it.

### 2026-11-22 (second pass)

- Implemented:
  - **E1, Source Serif 4:** with the owner's go-ahead, `SourceSerif4-Regular.ttf`, `SourceSerif4-It.ttf` and `SourceSerif4-Bold.ttf` (adobe-fonts/source-serif release 4.005) and the licence as `OFL-sourceserif4.txt` are in `assets/fonts/`.
    - `captions.font` accepts "Source Serif 4", and validation takes its names from the bundled fonts.
    - The HTML text templates get `@font-face` rules in `assets/templates/_base.css`.
    - Video Studio's `CUTLINE_FONTS` includes it.
    - `E_FONT_MISSING`'s hint lists it.
    - `FontDb::get` now prefers upright faces, because `SourceSerif4-It.ttf` sorted before the Regular face and was picked for plain text.
    - spec.md §12.1 lists the font; the user guide, the Video Studio README and docs/decisions.md are updated.
  - **Windows timed scroll:** `video-studio-core` type-checks and is clippy-clean for `x86_64-pc-windows-msvc`. A stub C compiler stood in for ring's C build, which a type-check never links.
- Verified: `cutline-text` `fonts::tests::bundled_serif_has_regular_italic_and_bold`; `cutline-render --test bundled_serif`, which covers captions validation by name, the Georgia hint naming the serif, the template faces and CSS, and the licence file; `video-studio-core` brand tests.
- Not done:
  - The Windows timed scroll hasn't run on Windows.
  - The "Done when" fixture run still needs a real screen for the timed scroll and a real recording.
- Files: `assets/fonts/SourceSerif4-*.ttf`, `assets/fonts/OFL-sourceserif4.txt`, `assets/templates/_base.css`, `crates/cutline-text/src/fonts.rs`, `crates/cutline-render/tests/bundled_serif.rs`, `crates/video-studio-core/src/brand.rs`, `spec.md`, `docs/`.
- Commit/PR: "TASK-honest-cedar-A73E: bundle Source Serif 4; Windows scroll type-checks" on branch `implement-approved-plans-2026-11-22-pass2`.
