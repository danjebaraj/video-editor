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
  - [ ] The choice in Settings on macOS and Windows (Settings › Assistant). Implemented; the app builds, not yet checked in the running app.
- [x] Resolve Codex from its configured executable path or, when unset, from PATH. Check the selected provider during readiness validation. For Codex, report missing executable, incompatible protocol and sign-in required as distinct findings with a remedy. A Codex workflow can start on a machine without Claude.
  - [x] `codex::find_codex` (configured path, else PATH). With Codex as the provider, readiness validation checks Codex instead of Claude: `codex_missing`, `codex_incompatible` and `codex_signed_out` are separate blockers, each with a remedy, and `claude_missing` isn't raised. FFI `codex.check` gives the same answer for the Settings page.
  - [x] A Codex workflow starts without Claude: `RunEnv::detect` and FFI `start_assistant` don't need Claude Code when Codex is the provider, the `codex_runs_unavailable` finding and the `CODEX_NOT_WIRED` refusal are gone, and a missing Codex is a clear environment error rather than a fallback to Claude (tested with `VIDEO_STUDIO_NO_CLAUDE=1` in `tests/codex_runs.rs`).
- [x] Start `codex app-server` over stdio, complete `initialize` and `initialized`, and use `account/read` and `model/list` for readiness and model selection. Keep model preferences per provider and use the Codex default when no Codex model is selected.
- [x] In `assistant.rs`, pass the current editor text, selection, project summary and authorized attachments through the existing prompt and session preparation. Stream replies, display questions and return answers with the original request ID. Apply proposed edits to the session copy and send its diff through the existing per-hunk review.
- [x] Start a Codex thread for a new conversation and resume its ID only for the same project, conversation and provider. If resume fails, identify the lost context in the chat before starting a fresh thread with the current editor state.
- [x] In `run/orchestrator.rs`, give Codex the prepared run workspace, manifest and production instructions. Route tool progress, diagnostics and collected outputs to the existing run inspector. Determine success through the existing output checks.
- [x] Enforce the existing assistant and run permissions for Codex, including read-only reference media, workspace write limits, command allowlists, web access settings and ElevenLabs helper credit limits. Disable inherited tools that bypass these controls. Deny unsupported permission requests and return a protocol error for unknown server requests.
  - [x] Unknown server requests get a JSON-RPC error (−32601) carrying the request's own ID (`AppServer::request`).
  - [x] Mapping the broker's permissions (read-only media, workspace writes, command allowlist, web access, credit limits) onto Codex approvals and sandbox settings, and disabling tools that bypass them.
  - [ ] Commands Codex itself classes as safe and read-only run without an approval request under `untrusted`, so the broker's command allowlist and read limits don't apply to them (the sandbox still blocks writes outside the workspace). — dropped: the owner chose to keep this behaviour (2026-11-22). Every Codex start writes the warning `codex_reads_unchecked` to its trace (`codex.jsonl` for runs, `<session>.codex-last-run.jsonl` for the assistant), and runs also show it as a warning event in `events.ndjson` and the inspector.
- [x] On Stop, send `turn/interrupt` for the active thread and turn and use existing process-tree cleanup if it fails to stop. On disconnect or protocol failure, release pending questions and approvals, record a terminal error and allow a new operation to start.

## Acceptance criteria

- [ ] On each desktop platform, load legacy Claude settings, select Codex, restart and verify the choice persists. With Claude absent and a signed-in Codex available, both assistant and run readiness pass the provider check. Switching back to Claude restores its model preference.
- [x] A fake app server records initialization before thread creation, streams a reply, requests user input with a string ID and proposes two independent edits. Verify the answer carries that ID and accepting one hunk changes only that hunk in the saved spec.
- [x] A second turn resumes the recorded thread. Switching projects or providers starts a separate conversation. A rejected resume produces a visible notice and exactly one new thread.
- [x] Run a fixture project with Codex and a cutline fixture. Verify the expected output appears in results and the inspector shows tool progress. Repeat with a missing required output and verify the run reports failure.
- [x] Exercise denied command, out-of-workspace write and unsupported tool requests with a fake server. Verify the command never executes, the external file retains its contents and each request receives a terminal response.
- [x] Exercise Stop during a pending question, unexpected server exit and failed sign-in. Verify the appropriate stopped or failed state, actionable feedback and cleanup before retry. Run the existing Claude assistant and production fixtures as regression checks.

## Design notes

Implementation notes (2026-11-22):

- `crates/video-studio-core/src/codex/agent.rs` (`CodexProcess`) drives one turn and translates the app server's traffic into the Claude client's `StreamEvent`s; `crate::agent::AgentProcess` lets `assistant.rs` and `run/orchestrator.rs` follow either provider with the same code. One app server per assistant turn or run, as with `claude -p`.
- Session tokens: a Codex conversation's token is `codex:<thread id>` (stored by the apps where they keep the Claude session ID). Claude never resumes a `codex:` token and Codex never resumes a Claude session, so switching providers starts a separate conversation. The assistant also records each session folder's thread next to the folder (`<session>.codex-thread.json`) and resumes only that thread, so a token from another project or conversation starts a new thread.
- Permissions: thread/turn settings are `approvalPolicy: untrusted`, `approvalsReviewer: user`, a `workspaceWrite` sandbox with only the run or session folder writable (no `$TMPDIR`, no `/tmp`), network access and `web_search` only with web access, `mcp_servers: {}`, and `shell_environment_policy.set` carrying the run's PATH (cutline proxy first) and the helper context (ElevenLabs credit cap). Command, network and file-change approvals go through `Policy::decide` (file changes path by path); `item/permissions/requestApproval` gets an empty grant, MCP elicitations are declined and dynamic tool calls fail, each logged and shown as blocked; unknown requests get −32601.
- Known limit: commands Codex itself classes as read-only and safe run without an approval request under `untrusted`; the sandbox still stops them writing outside the workspace, but reads aren't limited to the broker's read paths the way Claude's Read/Glob/Grep are.
- The lost-context notice reaches the chat as an existing `error` assistant event (the apps already show it; the turn continues), so no app change was needed.

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

### 2026-11-22 (Settings UI)

- Implemented: **The choice in Settings on macOS and Windows**, in a new Settings › Assistant pane on both apps:
  - A provider choice: Claude Code (the default) or Codex.
  - For Codex: its executable path (empty means PATH) and its model. "Codex default" is empty; the other entries are the models FFI `codex.check` returns.
  - Codex's status with Re-check. When Codex isn't ready, the pane shows the finding's title and remedy. FFI `codex.check` now adds `finding: {code, title, remedy}` from `CodexStatus::finding()`.
  - Claude's model preferences stay in Settings › Claude, untouched.
  - The macOS `CoreSettings` now carries `provider`, `codex_path` and `codex_model`. Before, saving any setting from the Mac app dropped them, so a Codex choice fell back to Claude. The Windows `Settings` type has them too.
- Verified:
  - `cargo test -p video-studio-ffi`: the dispatcher test checks that a missing Codex path returns `state: missing` with the `codex_missing` finding and its remedy.
  - The macOS debug build and the Windows UI build (`tsc` and vite) pass.
- Not observed: the pane wasn't opened in either running app, and `codex.check` wasn't run against a signed-in Codex from the UI.
- Not done: everything else listed in the previous entry: the assistant and runs over Codex, permissions, Stop and disconnects, and every acceptance criterion.
- Files: `apps/macos/Sources/VideoStudio/{Views/SettingsView.swift,Core/Models.swift,App/AppDelegate.swift}`, `apps/windows/ui/src/{settings/settings.ts,types.ts}`, `crates/video-studio-ffi/src/lib.rs`.
- Commit/PR: "Implement FEAT-neat-osprey-DKK7 and FEAT-spry-alpaca-5RQZ desktop UI: Audio Review player, Codex settings".

### 2026-11-22 (review of the desktop UI pass)

- The desktop UI boxes were unchecked again: they are built but nobody has used them in a running app, and a box needs "fully implemented and tested". Check them once each control has been tried on macOS and Windows.
- The Windows shell (`video-studio-win`, including the new menu entry) type-checks and is clippy-clean for `x86_64-pc-windows-msvc`, using stub C and resource compilers since a type-check never links.
- Commit/PR: desktop UI in `10d2f4c`, merged into branch `implement-approved-plans-2026-11-22-pass2`.

### 2026-11-22 (assistant, threads, runs, permissions, stop)

- Implemented:
  - "In `assistant.rs`, pass the current editor text…": `assistant::start_codex` with the same session preparation and prompt as Claude; streamed text, questions (answered by Codex question ID with the server request's own ID), edits on the scratch `spec.md` reported as `spec_changed` for the per-hunk review. FFI `start_assistant` picks the provider from Settings.
  - "Start a Codex thread for a new conversation…": `thread/start` / `thread/resume`, `codex:<thread>` tokens, the per-session thread record, and a lost-context notice followed by exactly one new thread.
  - "In `run/orchestrator.rs`, give Codex the prepared run workspace…": `Prepared.codex`, Codex developer instructions (production instructions plus a Codex note), tool calls, cutline progress and the helper's credit accounting through the existing events, success through the existing output checks.
  - "Enforce the existing assistant and run permissions for Codex…": sandbox, approvals and config as in the Design notes; every approval through the broker.
  - "On Stop, send `turn/interrupt`…": pending questions and approvals are answered first, then `turn/interrupt` for the active thread and turn, then the process tree is killed after the grace period; a disconnect or protocol failure ends the turn with an error and frees the next one.
  - "A Codex workflow starts without Claude": `RunEnv::detect` without Claude for Codex; `codex_runs_unavailable` and `CODEX_NOT_WIRED` removed.
  - Acceptance criteria 2–6 with the fake app server.
- Protocol source: the reference project isn't on this machine, so message shapes come from the installed codex-cli 0.171.0 (`codex app-server generate-ts` and `generate-json-schema`): `thread/start`, `thread/resume`, `turn/start`, `turn/interrupt`, `item/started` / `item/completed` / `item/agentMessage/delta`, `turn/completed`, `thread/tokenUsage/updated`, `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/tool/requestUserInput`, `item/permissions/requestApproval`, `mcpServer/elicitation/request`, `item/tool/call`, and the v1 `execCommandApproval` / `applyPatchApproval`.
- Verified:
  - `video-studio-cli --test codex_assistant` (7 tests): initialization before `thread/start`; streamed and whole text; usage; a question with string ID `q-7` answered with that ID; two independent edits where accepting one hunk changes only that hunk in a saved project; the session folder as the only writable root; second-turn resume; another project's conversation and a Claude session each start a new thread; a rejected resume gives one notice before the new thread and exactly one `thread/start`, and the next turn resumes the new thread; a refused command never runs, an outside file keeps its contents, and the dynamic tool, permissions, elicitation and unknown requests each get a terminal answer; Stop during a question releases it with its ID and sends `turn/interrupt` with the thread and turn IDs, and the next turn runs; an unexpected exit and a signed-out account are errors and a retry works; a failed turn is an error.
  - `video-studio-cli --test codex_runs` (2 tests, with `VIDEO_STUDIO_NO_CLAUDE=1`): a Codex run with the fake cutline produces both outputs, tool messages, cutline progress, render/validate stages and ElevenLabs credits; the run folder is Codex's working directory and only writable root and the instructions include the media manifest; a run without its outputs fails with "without producing", and a refused `rm`, an outside write and a write to `logs/run.json` are declined and counted.
  - Unit tests for tokens, shell unwrapping, effort mapping and the sandbox settings; the readiness test now expects no `codex_*` finding for a ready Codex.
  - Regression: all video-studio-core, -cli (including the Claude `e2e_runs` and `e2e_tools` fixtures) and -ffi tests pass; `cargo fmt --all --check` and clippy with `--tests` are clean.
- Not done:
  - The Settings choice on macOS and Windows and acceptance criterion 1 (desktop apps; another agent is adding the Settings UI).
  - Not checked against the real service (no real turns were run, to keep the user's account untouched): whether 0.171.0 accepts every `config` override sent with `thread/start` (`mcp_servers`, `web_search`, `sandbox_workspace_write`, `shell_environment_policy`), and the exact command text in real approval requests (shell wrappers are unwrapped heuristically).
- Files: `crates/video-studio-core/src/{agent.rs,codex.rs,codex/agent.rs,assistant.rs,claude/process.rs,run/orchestrator.rs,run/prepare.rs,validation.rs,lib.rs}`, `crates/video-studio-ffi/src/lib.rs`, `crates/video-studio-cli/src/bin/fake_codex.rs`, `crates/video-studio-cli/tests/{codex_assistant.rs,codex_runs.rs}`.
- Commit/PR: "Implement FEAT-spry-alpaca-5RQZ: Codex assistant, threads, runs, permissions and stop".

### 2026-11-22 (merge review)

- The permissions requirement was unchecked again and an open sub-box added: commands Codex classes as safe and read-only skip the broker's command allowlist and read limits.
- Commit/PR: core work in `32b97ad`, merged into branch `implement-approved-plans-2026-11-22-pass2`.

### 2026-11-22 (read gap traced)

- Implemented: as the owner asked, the read gap stays, and it's traced when Codex starts.
  - `codex::agent::READS_UNCHECKED_WARNING` is written as a `host_warning` (`codex_reads_unchecked`) line at the top of every Codex trace.
  - A Codex run also emits it as a warning event. It isn't counted in the run's warnings, so it doesn't turn every Codex run into "succeeded with warnings".
  - The permissions requirement is checked; the gap sub-box is marked dropped by the owner.
- Checked before the owner's decision: codex-cli 0.171.0 accepted `features` overrides (`shell_tool`, `unified_exec`, `view_image`, `apps`, `plugins`, `browser_use`, `computer_use`, `hooks` and others set to false) and a `run_command` dynamic tool on an ephemeral `thread/start`, with `experimentalApi`. No turn was run. That remains a way to close the gap later.
- Verified: `video-studio-cli --test codex_runs` asserts the trace line and the run event; `codex_assistant` and the core Codex tests pass.
- Files: `crates/video-studio-core/src/{codex/agent.rs,run/orchestrator.rs}`, `crates/video-studio-cli/tests/codex_runs.rs`.
- Commit/PR: "FEAT-spry-alpaca-5RQZ: trace the Codex read gap on start" on branch `implement-approved-plans-2026-11-22-pass2`.
