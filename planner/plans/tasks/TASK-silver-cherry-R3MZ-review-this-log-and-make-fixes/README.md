---
id: TASK-silver-cherry-R3MZ
type: task
title: Fix cutline review failures and verify opening-frame QA
status: done
done: yes
approved: yes
approved-on: 2026-11-21
priority: medium
created: 2026-11-21
updated: 2026-11-22
attachments: []
---

# Fix cutline review failures and verify opening-frame QA

## Summary

An 85-second edit eventually rendered for YouTube Shorts, Instagram Reels and TikTok at 1080×1920, 30 fps. The three exports shared one timeline and each passed QA with two warnings. Use the observations below and generate all test inputs locally from a clean checkout.

| Case | Observed behavior |
|---|---|
| HTML overlay | During `cutline review project.json5`, FFmpeg tried to open `graphics/ov04.html` as an input to the final encode, reported “Invalid data found when processing input” and review exited 3. Whether the declaration was valid remains to be established. |
| Sparse screen recording | Review exited 4 with “frame grab at 11.433s failed”. Disabling OCR produced the same error. A smaller normalization project failed review at 7.967 s. The recorder emitted frames on picture changes. Rendering 30 fps copies and substituting them for the original recordings allowed review to finish. The workaround points to frame timing but does not establish the root cause. |
| Opening-frame QA | Each export reported six `black_frames` spans lasting 0.4–3.6 s and a `hook` warning for the first caption at 1.34 s. The edit used near-black (#141211) cards with sparse off-white text. The operator reported a visible title from frame zero and speech starting at 1.2 s. Verify the detector behavior with controlled frames. |

## Steps

- [x] Create minimal local JSON5 projects for the cases below using the repository schema and generated media. Run the affected commands against the current checkout and record the actual exit status, diagnostic and output for each fixture before changing code.
- [x] Use a local HTML file drawing an opaque amber rectangle over a blue video. Exercise the schema-supported HTML graphic declaration and a media overlay that incorrectly uses the same HTML file as ordinary video. Trace validation and review preprocessing. Correct any path that passes raw HTML to FFmpeg and document the supported declaration in the authoring guidance.
- [x] Generate a 12-second MOV with distinguishable frame colors at 0, 4, 8 and just before 12 s, using variable frame timestamps. Verify the timestamps with ffprobe. Put the whole clip on the timeline and exercise review with OCR off, including a sample at 11.433 s through the review frame-grab path in `crates/cutline-qa/src/review.rs`. Use a 30 fps version of the same timeline as the control. Trace any failure through intermediate encoding and seeking.
- [x] Generate two otherwise identical portrait clips with first speech at 1.2 s and first caption at 1.34 s. Open one with a visible off-white title on #141211 for 3.6 s and the other with solid black for the same interval. Inspect the rendered frames and the `black_frames` and `hook` results from the normal QA path. Trace each finding in `crates/cutline-qa/src/qa.rs` and correct any confirmed detection or reporting defect.

## Done when

- [x] The overlay fixture either produces a review showing the declared graphic or, for invalid input, fails validation with a field-specific correction before FFmpeg starts.
- [x] Review produces a nonempty image showing the expected held frame at an in-range sample time on the sparse capture. The same check passes for a constant-frame-rate control.
- [x] QA regression checks distinguish a visible dark title card from a genuinely black span and describe the evidence used for the hook warning. Record the observed results for a visible opening title with delayed speech and for an empty opening.
- [x] Each distinct failure has a documented disposition and each product fix has a passing regression check through its affected command. Record the commands, fixture inputs and results.

## Implementation log

### 2026-11-21

- Implemented: Reviewed the run evidence and specified investigation and verification steps.
- Not done: Reproduction, product fixes and regression checks await approval.
- Files: This task README.md.

### 2026-11-21

- Implemented: Made the task self-contained with observed diagnostics, synthetic fixture inputs and command-level verification steps.
- Not done: Reproduction, product fixes and regression checks await approval.
- Files: This task README.md.

### 2026-11-22

- Implemented: all four steps and all four "done when" checks.
- Fixtures, all generated locally under a scratch folder (commands below), run with `target/debug/cutline` built from this checkout. The before results come from the code before the fix.

| Case | Fixture | Command | Before | After |
|---|---|---|---|---|
| HTML overlay, supported declaration | 4 s blue 1080×1920 `blue.mp4`; `graphics/ov04.html` draws an opaque #FFB000 rectangle; layer `{type: "html", asset: "ov04", start: 0, end: 4}`, asset `duration: 4` | `cutline validate`, `cutline review` | valid; review exit 0, contact sheet shows the amber rectangle over blue | unchanged |
| HTML overlay used as video | same files; layer `{type: "video", asset: "ov04", …}` | `cutline validate`, `cutline review` | validate: `✔ valid`; review exit 3: `ffmpeg failed (final encode): … Error opening input: Invalid data found when processing input … graphics/ov04.html` | validate and review exit 1 before ffmpeg starts: `E_WRONG_ASSET_KIND /layers/0/type: `ov04` is an HTML graphic, which a `video` layer can't play` with the hint to use `{type: "html", asset: "ov04", …}` |
| Sparse capture, whole clip | 12 s 1080×1920 MOV, 9 frames at 0, 0.5, 1.7, 4.0, 4.3, 8.0, 9.1, 11.9 and 11.967 s (red from 0 s, green from 4 s, blue from 8 s, white from 11.9 s; `select` + `-fps_mode passthrough`, `-bf 0`), duration 12.000 s checked with ffprobe; CFR control from `-vf fps=30` | `cutline review vfr.json5 --ocr off`; the `grab` path on the source | review exit 0 (the render path already holds sparse frames). On the source, a grab at 11.433 s returned the white 11.9 s frame, not the blue frame on screen, and a grab at 11.99 s (after the last stored frame, inside the 12 s file) gave **0 bytes and no ffmpeg message**, which is the empty `frame grab at …s failed: ` | the grab at 11.99 s returns the white frame; the held-frame grab at 11.433 s returns blue; the review preview at 11.433 s is blue for both the sparse clip and the CFR control |
| Opening title, #141211 card with off-white "He quoted half a verse." for 3.6 s, speech from 1.2 s, text layer at 1.34 s | HTML card + 3 s body clip + voice tone | `cutline render title.json5` (normal QA path) | `black_frames` warn `[[0.0, 3.6]]`; `hook` warn `first caption/text at 1.33s` | `black_frames` ok, `no black spans > 0.3 s (1 dark span(s) show content)`, evidence `luma variance 0.0050 at 0.90s (> 0.0004)` and two more; `hook` ok `text on screen at 0.50s (recognised)` with the words; with `CUTLINE_OCR=off`: `hook` warn `first caption/text at 1.33s (caption and text layers) (text in HTML isn't checked)` |
| Empty opening, solid black for 3.6 s, same audio and text | `color:#000000` card | `cutline render black.json5` | `black_frames` warn `[[0.0, 3.6]]`; `hook` warn | `black_frames` warn `[[0.0, 3.6]]` (sample at 0.90 s is flat with no words); `hook` warn `first caption/text at 1.33s (caption and text layers)` |

- Dispositions:
  - **HTML file as a video overlay:** the declaration was invalid. The product defect was that validation let it through to ffmpeg. Fixed with a field-specific `E_WRONG_ASSET_KIND` at `/layers/N/type` for `video`, `broll`, `image` and `presenter` layers that use an HTML, Lottie or SVG asset. `docs/CLAUDE_GUIDE.md` (Generated graphics) and `docs/PROJECT_FORMAT.md` (Layers) now give the supported `html` layer declaration.
  - **Frame grab on sparse recordings (root cause):** `grab()` seeked with `-ss` before `-i`, so it took the first stored frame at or after `t`. On a recording that stores frames only on change, that is the next change, not the picture on screen. After the last stored frame, ffmpeg writes nothing and exits 0, so the error had an empty reason. Fixed: `grab()` now tries an accurate seek and then the last frame at or before `t`. On failure its error names the file, `t`, the duration and the frame count, plus ffmpeg's messages or "(ffmpeg printed nothing)". The new `grab_held()` and `grab_source()` return the frame on screen, and review's reframe panels use them for sparse sources. The reported 11.433 s failure didn't reproduce through the render path, which already holds sparse frames. It reproduced on the source grab path when the time fell after the last stored frame.
  - **`black_frames`:** a confirmed detection defect. blackdetect's 98 % below 8 % luma rule counts a near-black card with sparse text. A span now stays black only if one of its frames at 25, 50 and 75 % has luma variance at most 0.0004 (the `first_frame` limit) and no recognised word. The cleared spans are reported under `dark_with_content` with their evidence.
  - **`hook`:** a confirmed reporting defect. Its input is only the caption and text layers, so a title drawn by an HTML clip or layer was invisible to it. When `first_text_at` is after 1 s and an HTML/Lottie/SVG clip or layer is on screen at 0.5 s, QA reads that frame (Vision on macOS, Tesseract elsewhere, `qa.ocr`/`CUTLINE_OCR`/`--ocr`). With recognition off or unavailable, the warning stays and adds "(text in HTML isn't checked)". Every hook message now names its source ("(caption and text layers)").
- Regression checks (all pass):
  - `cargo test -p cutline-core --test invalid_projects`: `e20b_html_asset_as_media_layer`.
  - `cargo test -p cutline-qa --test vfr_review`: held frame, CFR control, error text, and review on the sparse clips.
  - `cargo test -p cutline-qa --test opening_qa`: visible card vs black opening, OCR on and off, and the black gap control through `render_all`.
  - `cargo test -p cutline-qa -p cutline-core -p cutline-cli` passes; clippy and fmt are clean for the touched crates.
- Not done: nothing. The real 85-second project wasn't available, so the three-preset (Shorts/Reels/TikTok) rerun of the original edit wasn't done; TASK-honest-cedar-A73E covers the presets with local fixtures.
- Files: `crates/cutline-core/src/timeline.rs`, `crates/cutline-qa/src/{review.rs,qa_media.rs,qa.rs,deliver.rs}`, `crates/cutline-qa/tests/{vfr_review.rs,opening_qa.rs}`, `crates/cutline-core/tests/invalid_projects.rs`, `docs/CLAUDE_GUIDE.md`, `docs/PROJECT_FORMAT.md`.
- Commit/PR: on branch `implement-approved-plans-2026-11-22` ("Implement TASK-silver-cherry-R3MZ: …").
