---
id: BUG-stellar-lily-K8CN
type: bug
title: Codex browser automation crashes
status: open
fix: none
verified: no
approved: no
priority: medium
created: 2026-11-23
updated: 2026-11-23
attachments: []
---

# Codex browser automation crashes

## Summary

During a macOS Codex production run, `cutline review project.json5` failed to render a local HTML graphic. The bundled headless browser, installed Google Chrome and installed Microsoft Edge all exited before cutline obtained a DevTools WebSocket endpoint. No frames were produced. The run later delivered three 45-second exports using built-in title cards.

The reporter also observed that the video contained no browser recording. The saved input spec explicitly says “No stock footage, browser recordings or logos.” The missing recording therefore matches this run's instructions. The failing operation was HTML graphic rendering. A separate capture failure would need a project that actually requests a recording.

## Environment

- macOS Apple Silicon, Video Studio production run, Codex default resolved to `gpt-6.1-sol`, web access off. The input was a local title-card graphic. No external article or account is needed to exercise the failure.
- Bundled executable: `chrome-headless-shell-mac-arm64` version `158.0.8127.36`. Fallbacks attempted in order: Google Chrome and Microsoft Edge under `/Applications`.

## Steps to reproduce

1. In a temporary project, create a two-second local HTML title card using the repository's supported HTML graphic format. Draw a centered solid amber square on a dark background for the first second, then change the square to white. Use no external fonts, scripts, URLs, or media.
2. Reference the card as the sole picture in a version-1 project with one vertical export. Run `cutline review project.json5` first directly, then through the Codex production-run cutline launcher with web access off and normal sandbox settings. Use fresh render caches for each invocation so both must launch a browser.
3. Record the selected executable, process exit status or signal if available, profile and temporary-directory locations, elapsed time, startup stderr, and whether the DevTools endpoint became available. Compare both launch contexts before assigning a cause.

## Expected result

Review and final render can rasterize the authored local HTML graphic in the production-run environment. A browser startup failure identifies the attempted executables and the failure phase. A successful fallback preserves the graphic and its timing.

## Actual result

The first review exited 4 after 0.19 s. All three candidates reported “Input/Output error while resolving websocket URL from browser process” followed by “unexpected end of stream”. The bundled shell also printed thermal-notification and DNS-watch errors. Chrome and Edge supplied empty startup stderr. A separate beats-cache write reported “Operation not permitted”. These messages do not establish which restriction or process failure caused startup to end. The successful native-card render did not exercise this HTML path.

## Fix checklist

- [ ] Trace launch and fallback in `crates/cutline-html/src/lib.rs` and the production command environment in `crates/video-studio-core/src/run/`. Compare direct and sandboxed launch with the same local input. Identify the first failing operation and record evidence for the cause, including child-process diagnostics when available.
- [ ] Correct the verified launch prerequisite for local HTML rendering while preserving workspace permissions, offline operation and browser-process cleanup. If the production execution context cannot host the renderer, use a narrowly scoped app-managed rendering path with the same authorized inputs and outputs.
- [ ] Preserve per-candidate startup diagnostics when every browser exits before handshake, including executable, phase, elapsed time, exit status if known and whether stderr was empty. Distinguish immediate process exit from an actual startup timeout.
- [ ] Add a deterministic launcher regression for early exit with empty stderr, fallback success, and exhaustion of all candidates. Verify a failed attempt leaves no stale frames that a later attempt could report as success.
- [ ] On macOS run the local fixture through the production entry point. Sample the exported frames at 0.5 and 1.5 seconds and verify the square changes from amber to white. Verify an offline project that prohibits browser recording does not launch desktop capture. Record the command outcomes and browser used in the implementation log.

## Implementation log

### 2026-11-23

- Implemented: Documented the failing browser handshake, the run's explicit capture instructions and self-contained regression criteria.
- Not done: Controlled reproduction, root-cause confirmation, fix and verification await approval.
- Files: This bug README.md.
