---
id: FEAT-spry-alpaca-5RQZ
type: feature
title: Add support for Codex App Server
status: proposed
implementation: none
approved: yes
approved-on: 2026-11-21
priority: medium
created: 2026-11-21
updated: 2026-11-21
attachments: []
---

# Add support for Codex App Server

## Summary

Video Studio users can select their locally authenticated Croduction run. Claude remains the default for existing installations.

## Requirements

- [ ] Expose Claude and Codex in Settings on macOS and Windows. Persist the choice in `crates/video-studio-core/src/settings.rs`. Settings without a provider field resolve to Claude. Capture the choice at session or run start so changing Settings only affects subsequent work.
- [ ] Resolve Codex from its configured executable path or, when unset, from PATH. Check the selected provider during readiness validation. For Codex, report missing executable, incompatible protocol and sign-in required as distinct findings with a remedy. A Codex workflow can start on a machine without Claude.
- [ ] Start `codex app-server` over stdio, complete `initialize` and `initialized`, and use `account/read` and `model/list` for readiness and model selection. Keep model preferences per provider and use the Codex default when no Codex model is selected.
- [ ] In `assistant.rs`, pass the current editor text, selection, project summary and authorized attachments through the existing prompt and session preparation. Stream replies, display questions and return answers with the original request ID. Apply proposed edits to the session copy and send its diff through the existing per-hunk review.
- [ ] Start a Codex thread for a new conversation and resume its ID only for the same project, conversation and provider. If resume fails, identify the lost context in the chat before starting a fresh thread with the current editor state.
- [ ] In `run/orchestrator.rs`, give Codex the prepared run workspace, manifest and production instructions. Route tool progress, diagnostics and collected outputs to the existing run inspector. Determine success through the existing output checks.
- [ ] Enforce the existing assistant and run permissions for Codex, including read-only reference media, workspace write limits, command allowlists, web access settings and ElevenLabs helper credit limits. Disable inherited tools that bypass these controls. Deny unsupported permission requests and return a protocol error for unknown server requests.
- [ ] On Stop, send `turn/interrupt` for the active thread and turn and use existing process-tree cleanup if it fails to stop. On disconnect or protocol failure, release pending questions and approvals, record a terminal error and allow a new operation to start.

## Acceptance criteria

- [ ] On each desktop platform, load legacy Claude settings, select Codex, restart and verify the choice persists. With Claude absent and a signed-in Codex available, both assistant and run readiness pass the provider check. Switching back to Claude restores its model preference.
- [ ] A fake app server records initialization before thread creation, streams a reply, requests user input with a string ID and proposes two independent edits. Verify the answer carries that ID and accepting one hunk changes only that hunk in the saved spec.
- [ ] A second turn resumes the recorded thread. Switching projects or providers starts a separate conversation. A rejected resume produces a visible notice and exactly one new thread.
- [ ] Run a fixture project with Codex and a cutline fixture. Verify the expected output appears in results and the inspector shows tool progress. Repeat with a missing required output and verify the run reports failure.
- [ ] Exercise denied command, out-of-workspace write and unsupported tool requests with a fake server. Verify the command never executes, the external file retains its contents and each request receives a terminal response.
- [ ] Exercise Stop during a pending question, unexpected server exit and failed sign-in. Verify the appropriate stopped or failed state, actionable feedback and cleanup before retry. Run the existing Claude assistant and production fixtures as regression checks.

## Design notes

This extends the Claude-only assistant and production model in `ui/spec.md` and the Claude-driven cutline workflow in `spec.md`. Put the Codex transport in the shared Rust core so the CLI, FFI and desktop shells use the same protocol handling.

The reference project at `/Users/danieljebaraj/Documents/git/gitea/Just Notes` documents its tested protocol baseline as codex-cli 0.171.0. Use `Sources/MarkdownCore/CodexProtocol.swift` for message shapes, `Sources/JustNotes/Claude/CodexRunner.swift` for process and account handling, and `Tests/MarkdownCoreTests/CodexProtocolTests.swift` for fixtures. Just Notes lets Codex apply accepted patches itself. Video Studio must target the scratch copy so the user retains per-hunk control over the editor document.

## Implementation log

### 2026-11-21

- Implemented: Specification refinement only.
- Not done: All feature requirements and acceptance checks await implementation and verification.
- Files: This README.md.
