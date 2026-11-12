# Decisions log

Non-trivial design choices not dictated by the spec. Format: date — decision — alternatives — reason.

## 2026-11-12 — Workspace at repo root
- **Decision:** the cargo workspace lives at the repository root (`crates/…`), not under a `cutline/` subfolder.
- **Alternatives:** `cutline/` subdirectory as drawn in §3.1.
- **Reason:** the repo *is* the tool; an extra nesting level adds nothing.

## 2026-11-12 — Linux is a supported dev/test platform
- **Decision:** Linux (x86_64/arm64) is supported for development and CI, using the Martin Riedl Linux static builds or system ffmpeg as a fallback.
- **Alternatives:** macOS/Windows only.
- **Reason:** development happens in Linux containers; the user approved this. macOS stays the primary target.

## 2026-11-12 — ffmpeg sources
- **Decision:** Martin Riedl builds for macOS arm64, macOS x86_64 *and* Linux, all from the release channel with `.sha256` sidecars. BtbN `ffmpeg-nX.Y-latest-win64-gpl` (newest release branch, not master) for Windows. `CUTLINE_FFMPEG_MANIFEST_URL` overrides the source with a JSON manifest (mirrors/tests).
- **Alternatives:** evermeet.cx for macOS Intel (spec default).
- **Reason:** one source with published checksums and the same build config on both Mac architectures; evermeet publishes no machine-readable sha sidecar. evermeet remains a documented manual alternative.

## 2026-11-12 — Retry with backoff on downloads
- **Decision:** every HTTP request is retried 4× with exponential backoff, except sha256 mismatches (fail immediately).
- **Reason:** the Martin Riedl CDN intermittently answers 404 to redirect requests.

## 2026-11-12 — Lockfile location
- **Decision:** `CUTLINE_LOCKFILE` → nearest `tools.lock.json` up from the cwd (the repo pin) → `<tools_dir>/tools.lock.json`. `deps install` uses the pinned URL+sha when the lockfile has a complete entry for the platform; `--latest` ignores the pin.
- **Reason:** the repo pin is authoritative in the repo; installs elsewhere still record what they installed.

## 2026-11-12 — Tool version layout without symlinks
- **Decision:** `<tools>/ffmpeg/<version>/` plus plain-text `current` and `previous` files.
- **Reason:** symlinks need admin rights or developer mode on Windows.

## 2026-11-12 — Frame-ID layout is parametric
- **Decision:** the spec layout (16×64px blocks in the top row) is the default. `FrameIdLayout` can also place an 8×2 grid centered, and maps through cover/fit/scale transforms so the decoder knows where blocks land in an output frame.
- **Reason:** a 16:9→9:16 cover crop removes the top-left blocks; the centered grid survives it, which the exact frame-mapping tests need.

## 2026-11-12 — Fonts
- **Decision:** bundle Anton, Bebas Neue, Poppins (Regular/Bold/ExtraBold, static), Inter and Montserrat (variable) from google/fonts, each with its OFL license file.
- **Reason:** libass uses only the default instance of a variable font, so heavy caption and title styles use the static Poppins ExtraBold, Anton and Bebas Neue.

## 2026-11-12 — `reframe` unifies geometry modes
- **Decision:** one `reframe: {mode}` field carries every geometry mode (`cover`/`center`, `fit_blur`, `fit_pad`, `track_face`, `manual`, `split`). The project-level `default_reframe` defaults to `cover`.
- **Alternatives:** a separate `geometry` field next to `reframe`.
- **Reason:** the spec's example uses `reframe.mode` for both, and one field avoids conflicting settings.

## 2026-11-12 — Effect times are clip-relative timeline seconds
- **Decision:** `effects[].at` counts seconds from the clip's start on the timeline (after speed changes).
- **Reason:** that is what Claude sees in `expand`/review output; source-relative times would shift under speed changes.

## 2026-11-12 — Frame-accurate segment seeking
- **Decision:** snap the in point to the source frame grid, seek a quarter frame early (`-ss` before `-i`), then `setpts=PTS-STARTPTS` and `fps`. Pad and trim to the exact frame count (`tpad`+`trim`); audio is sample-exact (`apad`+`atrim`), with sample counts derived from timeline frame boundaries so they never drift.
- **Reason:** guarantees the exact frame mapping the selftest asserts.

## 2026-11-12 — Assembly timebase
- **Decision:** every segment input is normalized with `settb=AVTB` before concat/xfade.
- **Reason:** xfade refuses inputs whose timebases differ (concat outputs AVTB, mov inputs use the stream timebase).

## 2026-11-12 — Orchestration lives in `cutline-render::pipeline`, delivery in `cutline-qa::deliver`
- **Decision:** load → probe → resolve → extras → plan → execute is a library API so the selftest and the CLI share it. QA and the side files (`.qa.json`, `.render_report.json`, `.srt`, `credits.txt`) are written by `cutline-qa::deliver`.

## 2026-11-12 — Audio is rendered separately and muxed
- **Decision:** segment mezzanines carry dialogue audio (PCM f32). An audio stage builds `mix.wav` (and from M2 a loudness-normalized `final.wav`), which the final encode muxes.
- **Reason:** two-pass loudness needs the complete mix, and the video graph stays independent of audio changes.

## 2026-11-12 — `cut` transitions
- **Decision:** `{transition: {type: "cut"}}` has zero overlap whatever its duration. `audio: "cut"` on a visual transition hard-cuts the audio at the overlap midpoint with 10 ms micro-fades.

## 2026-11-12 — Ducking envelope model
- **Decision:** the gain ramps linearly in dB to `amount_db` over `attack` seconds *ending at* speech onset (pre-roll), holds while speech lasts, and ramps back over `release` seconds after speech ends. Gaps shorter than attack+release stay ducked. The envelope is written as a 1 kHz mono WAV, upsampled by ffmpeg and applied with `amultiply`.
- **Alternatives:** `volume=eval=frame` with a giant expression; sendcmd; `sidechaincompress` (still available as `duck.mode: "sidechain"`).
- **Reason:** deterministic, visualizable in audio.png, scales to 60-minute timelines (≈14 MB envelope), and speech is never masked at onset because transcripts tell us the future.

## 2026-11-12 — Speech activity source
- **Decision:** transcript word timings mapped to the timeline when available (M5); otherwise energy VAD (20 ms frames, adaptive threshold = floor + 15 dB, falling back to a −45 dBFS gate when the floor is loud, 150 ms hangover) over the `under` stems.

## 2026-11-12 — Loudness correction pass
- **Decision:** after `loudnorm` pass 2 (linear), measure with `ebur128`. If integrated is off by > 0.2 LU or true peak exceeds the target, apply a corrective gain plus a 4× oversampled `alimiter` at TP − 0.3 dB.
- **Reason:** loudnorm silently switches to dynamic mode when linear normalization would breach the true-peak limit, so the result can miss the ±0.5 LU acceptance window.

## 2026-11-12 — Stems
- **Decision:** the audio stage renders one WAV per present role (dialogue/voice/music/sfx) with its chain applied, then a separate mix job ducks and sums them. Default chains: voice and dialogue `clean_voice`, music `highpass=30`, sfx raw.
- **Reason:** stems feed the VAD, `audio.png` and future per-role QA without re-decoding.

## 2026-11-12 — Zoom implementation
- **Decision:** zooms (punch-in, zoom, Ken Burns, shake headroom) use one per-frame exact-size `scale` (`eval=frame`, bicubic) to `W+2·round(W(s−1)/2)`, then a fixed-size `crop` whose offset is integral for centered zooms. There is no 2× intermediate.
- **Alternatives:** `zoompan` (integer crop jitter); upscale to 2× and then crop (two resamples, about 4× the pixel work).
- **Reason:** a single resample with even size deltas gives a monotone scale with no center jitter. Measured by `punch_in_is_smooth_and_monotone` (≤ 1.5 % from the ease curve, no reversals, small second differences), which satisfies the spec's "equivalent smooth method" clause.

## 2026-11-12 — Explicit color interpretation
- **Decision:** segment and overlay conversions set `in_color_matrix` and `in_range` from the probed tags. Untagged HD (≥ 720 p) is treated as BT.709 and untagged SD as BT.601; `yuvj`/`pc` means full range. Test readers convert YUV→RGB explicitly as BT.709 limited. Test fixtures are encoded with explicit, tagged BT.709.
- **Reason:** ffmpeg 6.1 and 9.0 resolve `auto` differently for untagged streams.

## 2026-11-12 — Transition goldens per ffmpeg major version
- **Decision:** `tests/golden/transitions/ffmpeg<major>/`. The pinned version (9) is what CI uses; 6 is kept for the system-ffmpeg dev path. Without goldens for the running major version, the comparison is skipped with a message.
- **Reason:** xfade's progress math differs between major versions (the midpoint frame shows a slightly different progress), so a single golden set cannot pass on both versions at SSIM 0.97.

## 2026-11-12 — Overlay and text stacking
- **Decision:** visual layers stack by `z` (default: order in `layers`). ASS text and captions are always composited last, on top.
- **Reason:** captions and titles must never be hidden by B-roll; a single libass pass is also cheaper than interleaving.

## 2026-11-12 — Speed-ramp audio
- **Decision:** video follows the exact closed-form ramp (`setpts` with log integrals). Audio uses the ramp's average tempo (`atempo`), so it can drift inside the clip but always ends on the cut.
- **Known limitation:** accepted for v1 because speed-ramped clips are usually music- or SFX-backed.

## 2026-11-12 — Bundled LUTs are generated
- **Decision:** five 17³ `.cube` LUTs are generated by `scripts/gen_luts.py` (CC0, original math).
