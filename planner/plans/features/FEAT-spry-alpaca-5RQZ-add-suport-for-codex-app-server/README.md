---
id: FEAT-spry-alpaca-5RQZ
type: feature
title: Add support for Codex App Server
status: partial
implementation: partial
approved: yes
approved-on: 2026-11-21
priority: medium
created: 2026-11-21
updated: 2026-11-22
attachments: []
---

# Add support for Codex App Server

## Summary

Video Studio users can select their locally authenticated Croduction run. Claude remains the default for existing installations.

## Requirements

- [ ] Expose Claude and Codex in Settings on macOS and Windows. Persist the choice in `crates/video-studio-core/src/settings.rs`. Settings without a provider field resolve to Claude. Capture the choice at session or run start so changing Settings only affects subsequent work.
  - [x] `Settings.provider` (`claude` | `codex`, default Claude for settings without the field), `codex_path`, `codex_model`, persisted in `settings.rs` (unit tested, Claude's model preferences untouched).
  - [x] Captured at start: a run (`run::prepare::prepare`) and an assistant turn (FFI `start_assistant`) read the provider when they start.
  - [ ] The choice in Settings on macOS and Windows.
- [ ] Resolve Codex from its configured executable path or, when unset, from PATH. Check the selected provider during readiness validation. For Codex, report missing executable, incompatible protocol and sign-in required as distinct findings with a remedy. A Codex workflow can start on a machine without Claude.
  - [x] `codex::find_codex` (configured path, else PATH). With Codex as the provider, readiness validation checks Codex instead of Claude: `codex_missing`, `codex_incompatible` and `codex_signed_out` are separate blockers, each with a remedy, and `claude_missing` isn't raised. FFI `codex.check` gives the same answer for the Settings page.
  - [ ] A Codex workflow actually starting without Claude: until the assistant and runs use Codex, readiness reports `codex_runs_unavailable` and runs and assistant turns refuse with a clear message rather than falling back to Claude.
- [x] Start `codex app-server` over stdio, complete `initialize` and `initialized`, and use `account/read` and `model/list` for readiness and model selection. Keep model preferences per provider and use the Codex default when no Codex model is selected.
- [ ] In `assistant.rs`, pass the current editor text, selection, project summary and authorized attachments through the existing prompt and session preparation. Stream replies, display questions and return answers with the original request ID. Apply proposed edits to the session copy and send its diff through the existing per-hunk review.
- [ ] Start a Codex thread for a new conversation and resume its ID only for the same project, conversation and provider. If resume fails, identify the lost context in the chat before starting a fresh thread with the current editor state.
- [ ] In `run/orchestrator.rs`, give Codex the prepared run workspace, manifest and production instructions. Route tool progress, diagnostics and collected outputs to the existing run inspector. Determine success through the existing output checks.
- [ ] Enforce the existing assistant and run permissions for Codex, including read-only reference media, workspace write limits, command allowlists, web access settings and ElevenLabs helper credit limits. Disable inherited tools that bypass these controls. Deny unsupported permission requests and return a protocol error for unknown server requests.
  - [x] Unknown server requests get a JSON-RPC error (−32601) carrying the request's own ID (`AppServer::request`).
  - [ ] Mapping the broker's permissions (read-only media, workspace writes, command allowlist, web access, credit limits) onto Codex approvals and sandbox settings, and disabling tools that bypass them.
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

The reference project at `/Users/danieljebaraj/Documents/git/Just Notes` documents its tested protocol baseline as codex-cli 0.171.0. Use `Sources/MarkdownCore/CodexProtocol.swift` for message shapes, `Sources/JustNotes/Claude/CodexRunner.swift` for process and account handling, and `Tests/MarkdownCoreTests/CodexProtocolTests.swift` for fixtures. Just Notes lets Codex apply accepted patches itself. Video Studio must target the scratch copy so the user retains per-hunk control over the editor document.

## Implementation log

### 2026-11-21

- Implemented: Specification refinement only.
- Not done: All feature requirements and acceptance checks await implementation and verification.
- Files: This README.md.

### 2026-11-22

- Implemented:
  - The shared-core transport `crates/video-studio-core/src/codex.rs`:
    - `Provider`, and `find_codex`.
    - `AppServer`: starts `<codex> app-server`, sends one message per line, routes responses by ID, keeps notifications, and answers server requests with "method not found" using the request's ID.
    - `initialize` + `initialized`, `account/read` and `model/list` (lenient parsing).
    - `check` → `CodexStatus` (`missing` / `incompatible` / `sign_in_required` / `ready` with plan and models), and `finding()` remedies.
  - Settings fields (`provider`, `codex_path`, `codex_model`) and the provider-aware readiness check in `validation.rs` (split into `check_claude`, `check_codex` and `check_cutline_env`).
  - FFI `codex.check`.
  - A clear refusal (`run::prepare::CODEX_NOT_WIRED`) when Codex is chosen for a run or an assistant turn, so nothing silently falls back to Claude.
  - The fake app server `vs-fake-codex` (codex-cli 0.171.0 message shapes).
- Verified:
  - The handshake shape was checked against the installed codex-cli 0.171.0 (`initialize` result, the `account/read` result with `account` and `planType`, and notifications arriving between responses). `model/list` was not called against the real service in this unattended run.
  - `video-studio-cli --test codex`: initialization is sent before anything else; a notification is kept; a server request with the string ID `srv-1` is answered with −32601 and that ID; plan and models are read; signed out, incompatible and missing are distinct.
  - `validation` unit tests: Codex as the provider checks Codex, not Claude, with one blocker per problem.
  - `settings::provider_defaults_to_claude_and_persists`.
  - All video-studio-core, -cli and -ffi tests pass; clippy and fmt are clean.
- Not done:
  - Requirements 4–6 and 8: the assistant over Codex (editor context, streaming, questions answered with the request ID, edits on the session copy with per-hunk review), thread start and resume with lost-context notices, runs driven by Codex in `run/orchestrator.rs`, and Stop with `turn/interrupt`, disconnect handling and cleanup.
  - Most of requirement 7 (permission mapping).
  - The Settings UI on macOS and Windows.
  - All acceptance criteria.

  These need a protocol design for turns, items and approvals, which the reference project (`/Users/danieljebaraj/Documents/git/gitea/Just Notes`, not on this machine) was meant to supply, plus the desktop GUIs, so they were left for a follow-up.
- Files: `crates/video-studio-core/src/{codex.rs,lib.rs,settings.rs,validation.rs,run/prepare.rs}`, `crates/video-studio-ffi/src/lib.rs`, `crates/video-studio-cli/{Cargo.toml,src/bin/fake_codex.rs,tests/codex.rs}`.
- Commit/PR: "Implement FEAT-spry-alpaca-5RQZ (partial): Codex provider setting, transport and readiness" on branch `implement-approved-plans-2026-11-22`.
