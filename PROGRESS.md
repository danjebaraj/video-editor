# PROGRESS

Platform for this implementation run: Linux x86_64 container (no macOS host). macOS-only acceptance items are marked **not run here** and are covered by the macOS CI job.

| Milestone | Status |
|---|---|
| M0 Foundation | ✅ done |
| M1 Project model & basic render | ⏳ |
| M2 Audio | ⏳ |
| M3 Motion/transitions/overlays/color | ⏳ |
| M4 Text & graphics | ⏳ |
| M5 Transcription/captions/edit assist | ⏳ |
| M6 Auto-reframe | ⏳ |
| M7 QA/review/selftest | ⏳ |
| M8 HTML/SVG/Lottie | ⏳ |
| M9 Windows/packaging/docs | ⏳ |

## M0 — Foundation
Built:
- Cargo workspace with all 12 crates from §3.1, `xtask` (`cargo xtask ci`), GitHub Actions (macOS-14, windows-latest allowed to fail, ubuntu), `deny.toml`.
- `cutline-deps`: platform dirs (`CUTLINE_HOME`), tool discovery (env → tools dir → PATH), version parsing (`-/filter_complex` vs `-filter_complex_script`), capability probing, downloads with sha256 + retry, zip/tar extraction, lockfile, stage/promote/rollback.
- CLI: `doctor` (+ `--json`), `deps install ffmpeg|whisper-model|face-model|--all`, `deps update [--check]`, `deps rollback`, `deps status`, `probe [--loudness]`, `selftest --quick` (M0 checks). JSON output conventions and exit codes 0–4.
- `cutline-media`: ffprobe normalization (fps, VFR, rotation, color, alpha incl. VP9 `alpha_mode`, faststart detection), fingerprints, cache helpers.
- `cutline-testkit`: frame-ID video generator/decoder, tone/noise/burst audio, alpha overlays (ProRes 4444, VP9 alpha, PNG sequence), grid and beep/flash sources, FFT, RMS, SSIM, golden compare (`CUTLINE_BLESS=1`).
- Bundled OFL fonts in `assets/fonts/`.

Acceptance:
- `deps install ffmpeg` installs a working ffmpeg and records it in the lockfile. Verified for real on Linux with Martin Riedl 9.0.2, plus a mock-server integration test (`crates/cutline-cli/tests/m0_cli.rs`). The macOS run is covered by CI (**not run here**).
- `doctor --json` lists capabilities (test).
- `probe` on ProRes 4444 / VP9 alpha / PNG sequence fixtures (test).
- Frame-ID round trip through x264 CRF 23 + scale to 1080×1920, 300/300 frames (test).

Deviations: see `docs/decisions.md` (ffmpeg sources, Linux support).

## Blockers
- github.com release downloads are blocked from this container (BtbN Windows builds). The code path exists; Windows CI exercises it.

## Known issues
- (none yet)
