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
