---
id: BUG-0003
type: bug
title: Cutline not latest
status: fixed
fix: fixed
verified: no
approved: yes
approved-on: 2026-11-19
priority: medium
created: 2026-11-19
updated: 2026-11-19
attachments: []
---

# Cutline not latest

## Summary

Warning: cutline is an older build that can't convert HEIC/HEIF photos
/Users/danieljebaraj/Documents/git/github/video-editor/bin/cutline has no `cutline still`, so Claude can't look at iPhone photos. Update it (rebuild or reinstall), or point Settings › cutline at a newer build.

## Steps to reproduce

1. Build the macOS app and a current cutline (`apps/macos/build.sh`), so `bin/cutline still --help` succeeds.
2. Open a project and run Validate.

Expected: no cutline warning. Actual: the `cutline_outdated` warning above, naming `bin/cutline`.

## Cause

The core detects `still: true` correctly, but the macOS app's `CutlineInfo` (`apps/macos/Sources/VideoStudio/Core/Models.swift`) had only `path` and `version`. When the app sent the detected environment back for `validate`, the field was gone, Rust read it as `false`, and every cutline looked like an old build. The Windows app passes the JSON through unchanged and wasn't affected.

## Fix

- [x] The Swift `CutlineInfo` keeps `still`.
- [x] The core asks the cutline again (`cutline still --help`) when an environment from the app says `still: false`, so an app build that drops the field can't raise the warning by mistake.
- [x] Test: `environment_without_still_is_rechecked` in `crates/video-studio-ffi/src/lib.rs`.

## Implementation log

### 2026-11-19
- Implemented: all three fix steps.
- Not done: checking in the running app that Validate no longer warns (verified: no).
- Files: apps/macos/Sources/VideoStudio/Core/Models.swift, crates/video-studio-ffi/src/lib.rs
- Commit/PR: not committed
