---
id: FEAT-magic-alpaca-JPD6
type: feature
title: Review cutline capabilities for professional video production
status: done
implementation: done
approved: yes
approved-on: 2026-11-21
priority: medium
created: 2026-11-21
updated: 2026-11-22
attachments: []
---

# Review cutline capabilities for professional video production

## Summary

Prioritize two additions for professional edits: editable gain over time on an audio layer, then a review aid for matching the exposure of adjacent shots. Document the existing split-edit workflow so an editor can lead a picture cut with incoming dialogue or carry outgoing dialogue across it.

## Current capabilities and findings

These findings come from the project model, render paths and repository tests. Runtime fixture results are pending.

| Area | Current capability and evidence | Work justified by the review |
|---|---|---|
| Editing and continuity | In `crates/cutline-core/src/model.rs`, a main clip has source in/out points and can mute its embedded audio. A separate audio layer has its own placement and source offset. `crates/cutline-render/src/audio_plan.rs` places that layer independently. | A J-cut or L-cut can be authored with existing fields. Provide a tested two-shot example covering source handles, timing and how to avoid playing dialogue twice. |
| Framing and motion | `crates/cutline-render/tests/m6_reframe.rs` covers manual keyframes, tracking a panning face and the fallback when no face is detected. `m3_motion.rs` covers animated zoom with a fixed focus point. | Use those controls for a vertical excerpt. Verify the subject stays in frame alongside captions before proposing another framing mode. |
| Color and shot matching | Per-clip `ColorGrade` supports exposure, contrast, saturation, temperature, tint and LUTs. `color_graph` in `crates/cutline-render/src/effects.rs` applies them. The editor can correct one shot independently but must choose and compare representative rendered frames manually. | Add a repeatable side-by-side comparison with luminance measurements. Let the editor select comparable frames because a deliberate lighting change can be editorially correct. |
| Dialogue and music | `Layer::Audio` exposes static gain, edge fades and placement. `audio_plan.rs` applies a constant layer gain. `crates/cutline-render/tests/m2_audio.rs` covers speech ducking, loudness, true peak and transitions. | A deliberate music swell or a dip under a particular line currently requires splitting and retiming layers or preprocessing the asset. Add an editable gain curve to a continuous layer. |
| Captions and graphics | `crates/cutline-render/tests/script_captions.rs` covers script-derived caption text and timing. `variants_locales.rs` covers a teaser and localized output. `m3_motion.rs` covers alpha compositing. | Exercise these together on a vertical excerpt. Audio audition and opening-card recognition already have separate Planner items. |
| Delivery QA | `crates/cutline-qa/tests/m7_qa.rs` exercises injected faults and review-pack limits. `m2_audio.rs` checks true peak after AAC encoding. | Keep the shot comparison informational. A difference in shot brightness alone must not turn a passing delivery into a warning or failure. |

## Ranked implementation scope

1. **Audio-layer gain automation** has the highest priority because it changes the audible mix and avoids brittle layer splits on every revision. Estimated effort is medium: schema, validation, audio graph and signal-level tests.
2. **Graded shot comparison** shortens the feedback loop for existing color controls. Estimated effort is medium: CLI selection, extraction of graded frames and report generation.
3. **Split-edit authoring example** makes an existing capability usable without changing timeline semantics. Estimated effort is small: one example and a timing regression check.

## Requirements

- [x] Add optional `gain_keys: [{at, db}]` to `Layer::Audio` in `crates/cutline-core/src/model.rs`. Use seconds relative to the resolved layer start, including on looped sources. Require finite gains and strictly increasing finite times within the layer span. Report invalid entries with their project field path.
- [x] In `crates/cutline-render/src/audio_plan.rs`, interpolate linearly in dB between keys, hold the nearest endpoint outside the keyed interval, and add the result to static `gain_db` before role ducking and final loudness processing. An absent or empty list uses the existing audio graph. A single key supplies a constant offset.
- [x] Extend `cutline review` with an optional `--compare-shots LEFT,RIGHT` argument naming two main-timeline clip IDs in the selected export. Sample each clip at the midpoint of its visible interval outside transition overlaps. Report unknown IDs and clips without a clean sample interval by ID.
- [x] Write a side-by-side PNG and JSON measurements in the review output directory. Label each frame with clip ID and sample time. Measure mean normalized BT.709 luma on the graded picture before captions and decorative overlays, reporting the two means and signed difference (right minus left). The comparison must leave the project and delivery QA result unchanged.
- [x] Add a runnable split-edit recipe to the cutline authoring guide. Show a picture cut at 2.0 s with incoming dialogue starting at 1.5 s, then a second version with outgoing dialogue continuing to 2.5 s. Include source handles, explicit audio placement and clip muting.

- [x] Review editing and continuity, framing and motion, color and shot matching, dialogue and music mixing, captions and graphics, and delivery QA. For each area, describe an editorial outcome and identify the existing command, project field or combination of features that can achieve it.
- [x] Trace each candidate gap from the public CLI or project schema to the implementation and relevant tests. Distinguish a missing control from an existing control that is hard to discover, a documented behavior that fails, and a result achievable with an existing recipe.
- [x] Exercise candidate gaps on small projects using locally generated media. Include at least a two-speaker edit with a dialogue handoff, two shots with visibly different exposure or color, and a vertical excerpt with speech, music and captions. Record the fixture recipe, commands, expected result and observed result for each.
- [x] Rank confirmed gaps by the impact on the finished video, the cost of the current workaround and implementation effort. For each recommended addition, give a minimal user-facing change, the affected code or schema, and a measurable acceptance check. Cross-reference overlapping Planner work by ID.

## Acceptance criteria

- [x] Generate a continuous four-second tone at constant amplitude. With a raw chain, ducking and final normalization disabled, set layer keys to 0 dB at 1 s and −12 dB at 3 s. Verify RMS relative to an unkeyed control at 0.5, 2.0 and 3.5 s is respectively 0, −6 and −12 dB within 0.5 dB. Repeat with the layer starting later to verify local timing.
- [x] Validate duplicate and descending key times, negative times and times beyond the layer end. Each fails with the offending field identified. Rendering a legacy project without keys produces the same decoded audio as the baseline.
- [x] Generate adjacent two-second gray shots at different brightnesses. Run the shot comparison, verify the PNG order, sample times and positive luma difference, then grade only the darker shot toward the reference. The absolute difference decreases and the reference mean stays within one 8-bit code value. Verify an unknown ID yields an actionable error.
- [x] Render the split-edit examples with distinct solid colors for the pictures and distinct frequencies for the two voices. Verify picture switches at 2.0 s within one frame in both versions and the relevant tone starts at 1.5 s or ends at 2.5 s within one frame.
- [x] Render a vertical excerpt with a generated moving subject, timed captions, voice and music. Verify the selected crop contains the subject at the sampled caption times, caption text matches the fixture and the existing loudness and true-peak checks pass. Record fixture inputs and exact commands alongside the results.

- [x] A findings section in this item covers all six review areas. Each finding cites a repository path and a specific command, field or test that supports its classification.
- [x] A reviewer can recreate each proposed gap from a clean checkout with cutline's documented dependencies, generated inputs and the recorded commands. Each result identifies what to look for or listen for at a particular timestamp, or gives a numeric QA result.
- [x] The review concludes with the additions worth pursuing in priority order, or explains why the existing tools cover the tested outcomes. Each recommendation states what the editor can achieve after the change and how to verify it.

## Design notes

Use `spec.md` §§6–7 as the intended baseline, `crates/cutline-core/src/model.rs` and `crates/cutline-cli/src/main.rs` for public controls, and the render, audio and QA crates for behavior. The baseline already describes transitions with audio crossfades, reframing, ducking, loudness normalization and post-render QA. Test specific editorial needs: for example, can incoming dialogue start before the picture cuts, can adjacent shots be matched without changing the whole timeline, and do caption and audio settings survive a vertical export? These are review questions. An existing recipe that meets the target is a valid finding.

Compare audio audition findings with FEAT-neat-osprey-DKK7 and review or QA findings with TASK-honest-cedar-A73E and TASK-silver-cherry-R3MZ before recommending additional work.

## Runtime findings (2026-11-22)

All inputs are generated by the tests named below; run them from a clean checkout with `cargo test -p <crate> --test <name>` (needs `cutline deps` ffmpeg).

| Area | Fixture and command | Look or listen for | Observed | Classification |
|---|---|---|---|---|
| Editing and continuity | Two 5 s shots (red + 400 Hz "voice", blue + 1000 Hz), picture cut at 2.0 s, clip sound muted and placed as `dialogue` audio layers with `src_in` handles (`cutline-render --test split_edit`) | J-cut: 1000 Hz from 1.5 s; L-cut: 400 Hz to 2.5 s; picture changes at frame 60 | Both within one frame; the muted clip sound never doubles the layer | Achievable with an existing recipe; now documented in `docs/CLAUDE_GUIDE.md` › Audio › Split edits |
| Framing and motion | 640×360 shot with a red square moving across in 6 s, vertical 360×640 target, `reframe: {mode: "manual", keys: [...]}` one key a second (`cutline-qa --test vertical_excerpt`) | The square inside the crop at each caption's middle (1.3 s, 3.7 s) | > 400 red pixels in both frames | Existing control covers it (also `m6_reframe.rs`) |
| Color and shot matching | Adjacent 2 s gray shots 0x505050 and 0x909090 (`cutline-qa --test shot_compare`, `cutline review p.json5 --compare-shots dark,light`) | `shots/compare_dark_light.png` dark left / light right; JSON at 1.0 s and 3.0 s | Luma 0.302 vs 0.553, difference +0.251; after `color: {exposure: 0.8}` on the dark shot 0.541 vs 0.553 (+0.012), reference unchanged | Gap (manual comparison) closed by the new `--compare-shots` |
| Dialogue and music | 6 s constant tone on an `sfx` layer (no chain, not ducked), keys 0 dB at 1 s and −12 dB at 3 s, measured in the role stem against an unkeyed control, layer starting at 0 and at 1.5 s (`cutline-render --test gain_keys`) | RMS at 0.5 / 2.0 / 3.5 s from the layer start | 0 / −6 / −12 dB within 0.5 dB at both starts; one key = constant offset | Gap (missing control) closed by `gain_keys` |
| Captions and graphics | The vertical excerpt above with a transcript (`Follow the red square` / `across the frame.`), voice and music at `music_level: 18` | SRT text; QA | Captions match the fixture; QA `loudness` ok: I = −14.1 LUFS, TP = −7.7 dBTP | Existing controls cover it; opening-card recognition is TASK-silver-cherry-R3MZ / TASK-honest-cedar-A73E, audio audition FEAT-neat-osprey-DKK7 |
| Delivery QA | The same shot comparison | QA result and suggestions with and without `--compare-shots` | The comparison writes only `shots/*`; QA findings and `qa.json` are unchanged | Informational by design |

Recommendations, in priority order (all three are implemented in this change): 1. `gain_keys` on audio layers: an editor can swell or dip music under a line on one continuous layer; verify with the stem RMS test. 2. `cutline review --compare-shots LEFT,RIGHT`: an editor sees both graded frames and their luma difference, grades the off shot, and compares again; verify with the gray-shot test. 3. The split-edit recipe: incoming dialogue can lead the cut or outgoing dialogue can carry past it with existing fields; verify with the two-tone timing test. Nothing else in the six areas needed a new control for the outcomes tested.

## Implementation log

### 2026-11-21

- Implemented: Defined the capability review, fixture scenarios and decision criteria.
- Not done: The review and its ranked findings await approval.
- Files: This item README.md.

### 2026-11-21

- Implemented: Added a source-based capability assessment, ranked additions, proposed interfaces and measurable acceptance checks.
- Not done: Runtime fixture verification and product implementation await approval.
- Files: This item README.md.

### 2026-11-22

- Implemented: every requirement and acceptance criterion.
  - `gain_keys: [{at, db}]` on `Layer::Audio` (`GainKey`, schema blessed). Validation in `timeline.rs`: `E_BAD_VALUE` at `/layers/N/gain_keys/K/at` (negative, past the layer end, not increasing) or `/db` (outside ±60 dB). JSON5 `Infinity` already fails at parse. In `audio_plan.rs` the per-source chain adds `asetnsamples=n=64` and a piecewise-linear dB `volume` expression after the layer's trim (so `t` is layer-local, loops included) and before fades, role chains, ducking and loudness. With no keys the graph is unchanged (asserted).
  - `cutline review --compare-shots LEFT,RIGHT`. IDs and clean sample intervals are checked before rendering. It samples the middle of each clip's span outside its transitions from the graded clip segment (before captions and overlays) and writes `shots/compare_L_R.png` (labelled) and `.json` (clip, timeline time and mean BT.709 luma of each side, and the right-minus-left difference). QA is unchanged. A malformed value exits 2; an unknown clip exits 4 with the list of clips.
  - The split-edit recipe, gain curves and shot matching in `docs/CLAUDE_GUIDE.md`; `gain_keys` in `docs/PROJECT_FORMAT.md`.
  - The runtime review above.
- Tests: `cutline-core --test invalid_projects` (`gain_keys_are_checked`, `schema_is_current`), `cutline-render --test gain_keys`, `--test split_edit`, `cutline-qa --test shot_compare`, `--test vertical_excerpt`; all of `cargo test -p cutline-core -p cutline-render -p cutline-qa -p cutline-cli -p cutline-transcribe` pass; clippy and fmt are clean.
- Not done: nothing.
- Files: `crates/cutline-core/src/{model.rs,timeline.rs}`, `crates/cutline-render/src/audio_plan.rs`, `crates/cutline-qa/src/review.rs`, `crates/cutline-cli/src/{main.rs,edit.rs}`, `schema/project.schema.json`, docs, and the new tests.
- Commit/PR: "Implement FEAT-magic-alpaca-JPD6: …" on branch `implement-approved-plans-2026-11-22`.
