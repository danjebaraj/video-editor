---
id: TASK-silver-cherry-R3MZ
type: task
title: Fix cutline review failures and verify opening-frame QA
status: todo
done: no
approved: yes
approved-on: 2026-11-21
priority: medium
created: 2026-11-21
updated: 2026-11-21
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

- [ ] Create minimal local JSON5 projects for the cases below using the repository schema and generated media. Run the affected commands against the current checkout and record the actual exit status, diagnostic and output for each fixture before changing code.
- [ ] Use a local HTML file drawing an opaque amber rectangle over a blue video. Exercise the schema-supported HTML graphic declaration and a media overlay that incorrectly uses the same HTML file as ordinary video. Trace validation and review preprocessing. Correct any path that passes raw HTML to FFmpeg and document the supported declaration in the authoring guidance.
- [ ] Generate a 12-second MOV with distinguishable frame colors at 0, 4, 8 and just before 12 s, using variable frame timestamps. Verify the timestamps with ffprobe. Put the whole clip on the timeline and exercise review with OCR off, including a sample at 11.433 s through the review frame-grab path in `crates/cutline-qa/src/review.rs`. Use a 30 fps version of the same timeline as the control. Trace any failure through intermediate encoding and seeking.
- [ ] Generate two otherwise identical portrait clips with first speech at 1.2 s and first caption at 1.34 s. Open one with a visible off-white title on #141211 for 3.6 s and the other with solid black for the same interval. Inspect the rendered frames and the `black_frames` and `hook` results from the normal QA path. Trace each finding in `crates/cutline-qa/src/qa.rs` and correct any confirmed detection or reporting defect.

## Done when

- [ ] The overlay fixture either produces a review showing the declared graphic or, for invalid input, fails validation with a field-specific correction before FFmpeg starts.
- [ ] Review produces a nonempty image showing the expected held frame at an in-range sample time on the sparse capture. The same check passes for a constant-frame-rate control.
- [ ] QA regression checks distinguish a visible dark title card from a genuinely black span and describe the evidence used for the hook warning. Record the observed results for a visible opening title with delayed speech and for an empty opening.
- [ ] Each distinct failure has a documented disposition and each product fix has a passing regression check through its affected command. Record the commands, fixture inputs and results.

## Implementation log

### 2026-11-21

- Implemented: Reviewed the run evidence and specified investigation and verification steps.
- Not done: Reproduction, product fixes and regression checks await approval.
- Files: This task README.md.

### 2026-11-21

- Implemented: Made the task self-contained with observed diagnostics, synthetic fixture inputs and command-level verification steps.
- Not done: Reproduction, product fixes and regression checks await approval.
- Files: This task README.md.
