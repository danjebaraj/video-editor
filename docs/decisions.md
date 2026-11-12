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
