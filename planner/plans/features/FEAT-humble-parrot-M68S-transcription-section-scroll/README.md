---
id: FEAT-humble-parrot-M68S
type: feature
title: Transcription section scroll
status: proposed
implementation: none
approved: yes
approved-on: 2026-11-21
priority: medium
created: 2026-11-21
updated: 2026-11-21
attachments: []
---

# Transcription section scroll

## Summary

During a live production run, users can scroll through the Transcript while keeping the run status, stages and actions in view.

## Requirements

- [ ] Give the expanded Transcript body its own vertical scroll area within the available inspector height. Keep the Transcript disclosure control, production run details and run actions outside that area.
- [ ] Keep the surrounding run section stationary when the user scrolls the transcript, including when scrolling past its first or last entry.
- [ ] Preserve the user's reading position when new text arrives while they are viewing an earlier entry. Keep the transcript expanded or collapsed as selected during live updates.

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
