---
id: FEAT-humble-parrot-M68S
type: feature
title: Transcription section scroll
status: partial
implementation: partial
approved: yes
approved-on: 2026-11-21
priority: medium
created: 2026-11-21
updated: 2026-11-22
attachments: []
---

# Transcription section scroll

## Summary

During a live production run, users can scroll through the Transcript while keeping the run status, stages and actions in view.

## Requirements

- [ ] Give the expanded Transcript body its own vertical scroll area within the available inspector height. Keep the Transcript disclosure control, production run details and run actions outside that area.
  - [x] Windows (`apps/windows/ui/src/project/inspector/run.ts`, `app.css`).
  - [ ] macOS (`LiveRunView` in `RunViews.swift`): implemented and the app builds; not yet checked in the running app.
- [ ] Keep the surrounding run section stationary when the user scrolls the transcript, including when scrolling past its first or last entry.
  - [x] Windows (`apps/windows/ui/src/project/inspector/run.ts`, `app.css`).
  - [ ] macOS (`LiveRunView` in `RunViews.swift`): implemented and the app builds; not yet checked in the running app.
- [ ] Preserve the user's reading position when new text arrives while they are viewing an earlier entry. Keep the transcript expanded or collapsed as selected during live updates.
  - [x] Windows (`apps/windows/ui/src/project/inspector/run.ts`, `app.css`).
  - [ ] macOS (`LiveRunView` in `RunViews.swift`): implemented and the app builds; not yet checked in the running app.

## Acceptance criteria

- [ ] On macOS and Windows, open a live production run with enough transcript entries to overflow the inspector. Scroll from the first displayed entry to the last and back. Verify the run heading, stage list, disclosure control and Stop button stay in place throughout, including at both scroll limits.
- [ ] Scroll to an earlier entry, then deliver another transcript event. Verify the same entry remains in view and the new text is reachable by scrolling down.
- [ ] Collapse Transcript while events arrive, then expand it. Verify the run actions remain reachable and the latest entry is present. Repeat with an empty transcript and a single short entry.

## Design notes

This refines the live production run inspector in `ui/spec.md` §12. On macOS, `LiveRunView` in `apps/macos/Sources/VideoStudio/Views/RunViews.swift` currently wraps the run details and Transcript disclosure in one scroll view. On Windows, `apps/windows/ui/src/project/inspector/run.ts` rebuilds the live inspector on updates, so the layout change must also preserve disclosure state and scroll position across renders.

## Implementation log

### 2026-11-21

- Implemented: Refined the feature spec and added acceptance checks.
- Not done: UI implementation and verification await approval.
- Files: This README.md.

### 2026-11-22

- Implemented: the three requirements on Windows, split into per-platform boxes. The live inspector now has three fixed parts (run details, the Transcript toggle, and the Reveal Folder/Stop row) and one scrolling transcript body with `overscroll-behavior: contain`. The run details scroll on their own only when they would take more than 55% of the height. The open/closed state and the reading position (the top visible entry, keyed by item identity, and its offset) are kept across the once-a-second rebuild. The old `<details>` element collapsed on every rebuild; that is fixed too. The macOS code is the same design: a `GeometryReader` with the details (`ViewThatFits` falls back to a scroll view when they don't fit), a custom Transcript toggle, and `TranscriptScrollView`, a `LazyVStack` with `.scrollPosition(id:anchor: .top)` so the top visible entry stays put while entries arrive or are trimmed from the start.
- Verified: Windows UI `npm run build` (gen + `tsc` + vite) passes. Checked in a throwaway Chromium page that mounts the real `runTab` with a fake live run (360×520 inspector, 60–670 entries). At both scroll limits the header, toggle and Stop row didn't move and the panel itself didn't scroll. The top visible entry stayed in place after 5 new events, after the 1 s timer rebuild and after 100 entries were trimmed from the start. Collapsing while events arrived kept it collapsed across rebuilds; expanding showed the latest entry. Empty and single-entry transcripts kept the toggle and Stop in place. macOS: `apps/macos/build.sh debug` builds (`** BUILD SUCCEEDED **`).
- Not done: the macOS sub-boxes and all three acceptance criteria. They need the macOS GUI and a real Windows (WebView2) machine with a live production run, and neither could be used in this unattended run.
- Files: `apps/macos/Sources/VideoStudio/Views/RunViews.swift`, `apps/windows/ui/src/project/inspector/run.ts`, `apps/windows/ui/src/styles/app.css`.
- Commit/PR: on branch `implement-approved-plans-2026-11-22` ("Implement FEAT-humble-parrot-M68S: Transcription section scroll").
