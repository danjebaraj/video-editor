---
id: BUG-amber-walrus-72VP
type: bug
title: Codex Audio Review stalls after patch approval and issue report mislabels the failure
status: in-progress
fix: in-progress
verified: no
approved: yes
approved-on: 2026-11-23
priority: medium
created: 2026-11-23
updated: 2026-11-23
attachments:
  - file: attachments/codex-replay.py
    kind: text
    size: 4767
    added: 2026-11-23
    description: Replays the app's Codex handshake against the real codex app-server and reports which commands and file changes were never reported finished
---

# Codex Audio Review stalls after patch approval and issue report mislabels the failure

## Summary

Codex received approval to write four narration files. Video Studio stopped the run after 120 seconds without confirmation that the change had been applied. The newer timeout message correctly distinguishes a second pending change. The underlying stall persists. The exported issue report also attributes Codex output to Claude, promotes schema descriptions to warnings, and presents an in-progress summary alongside a terminal failure.

## Environment

- Run `run_01m84n2c6e9z9p3yamvm`, Audio Review, macOS aarch64, app core and cutline 0.1.0, Codex model `gpt-6.1-sol`. The report does not establish the Codex CLI version.
- Selected output: YouTube Shorts, 1080×1920. Requested mix: 45 seconds with restrained 90 BPM music and no narration from 40–45 seconds. The previously confirmed ElevenLabs key remains configured. This report supplies no credential error.

## Steps to reproduce

1. In a temporary project, select Codex and start Audio Review for one Shorts output. Supply four short narration passages and a local audio fixture. Use the normal run preparation, sandbox and approval broker. The reference run folder is unnecessary.
2. In the app-server fixture, emit one file-change item for additions to `audio/scene1.txt` through `audio/scene4.txt`, then request approval for its item ID. After acceptance, leave completion pending. At 111 seconds request a separate change to `summary.md` with its own item ID. Advance a test clock to 120 seconds. Also exercise a normal server that applies and confirms the authorized changes.
3. Export an issue report from a local failed-run fixture with provider Codex, the model above, three exit-zero cutline logs, and no output WAV. In a schema log include JSON description strings containing `W_FIT_HOLD` and `W_SCRIPT_MISMATCH`. Put “Audio generation is in progress” in the draft summary. These strings reproduce the reporting defects independently of the live stall.

## Expected result

A valid workspace patch completes through the normal Codex protocol and the review can proceed to a playable WAV. If completion genuinely remains unknown, the app stops safely and reports each item's state. The exported report identifies the actual provider and terminal outcome, and lists only diagnostics emitted for the run.

## Actual result

The first patch timed out. The change to `summary.md` was abandoned nine seconds after acceptance. The report correctly says file state is unknown and no soundtrack was mixed. All three cutline invocations exited successfully. Nevertheless, the report displays “Claude Code: unknown”, “Claude's summary”, and two schema definitions under “cutline warnings”. The draft says audio generation is in progress. Its presence does not establish that the specific pending patch completed.

## Fix checklist

- [x] Trace one normal multi-file patch from `item/started` through `item/fileChange/requestApproval`, broker decisions, response delivery and server completion in `crates/video-studio-core/src/codex/agent.rs`. Compare with a supported local Codex app server using a disposable workspace; record the CLI version, item/request IDs, terminal notifications and observed file contents.
- [x] Find the first divergence in the failing run's `logs/codex.jsonl`: on the MacBook Pro, Codex sent no `item/completed` for any accepted command or file change in the turn (see the log entry "The failing run's protocol log").
- [ ] Find why Codex never reports items finished on that Mac (see "Next steps on the failing Mac" below). A fixture that deliberately omits completion tests the deadline only and cannot establish the cause.
- [ ] Correct the cause on the app's side, if it is in how the app starts Codex (waits on the step above), and verify a real Audio Review on that Mac reaches a playable `outputs/audio-review.wav`.
- [x] Verify a clean run creates all four expected script contents, accepts only authorized workspace changes, and produces a decodable `outputs/audio-review.wav` using local audio. Retain separate deadlines, one response per request and unknown file state for genuinely unconfirmed writes (BUG-green-cricket-P6HQ and BUG-deft-lily-344X).
- [x] Safeguard while the cause is unknown: an accepted change past its deadline, or still unreported when the turn ends, is checked on disk. A change whose files all match counts as written and the run goes on; otherwise the stop names each file's state instead of "unknown".
- [x] Update report generation in `crates/video-studio-core/src/logs.rs` to use recorded provider and effective model. Use a neutral label for legacy records without reliable attribution. On failed runs identify assistant-authored summary text as a draft and give the host's terminal result precedence while retaining the draft as evidence.
- [x] Replace the warning collector's substring match on “warning” or `W_` with recognition of emitted diagnostics. Schema/help prose must contribute zero warnings. Preserve genuine structured validation findings and supported plain-text warning lines, including warnings on commands that exited zero.
- [x] Extend `crates/video-studio-cli/tests/codex_runs.rs` and the report tests in `logs.rs` or `tests/e2e_runs.rs`. Verify the successful patch path and the 120 s/9 s timeout distinction. For the supplied report fixture, assert Codex attribution, failed status, missing WAV, three successful commands and no schema warnings. Add a real diagnostic as a positive control and exercise Claude and legacy report attribution.

## Next steps on the failing Mac

For a Claude Code session on the MacBook Pro where the stall happens (every run on the Mac mini succeeds with the same codex-cli 0.171.0). Work in a branch from `fix-BUG-amber-walrus-72VP`, or from main once it is merged.

1. **Record the setup.** Run `codex --version`. Read `~/.codex/config.toml` and note the `[features]`, plugins, `[mcp_servers.*]`, model, `model_reasoning_effort`, `service_tier` and any shell or sandbox settings. Never copy tokens or keys into the bug.
2. **Read what the model saw.** Open the session file of the failing thread, `~/.codex/sessions/2026/11/23/rollout-2026-11-23T11-50-45-01a20951-3260-75b0-b6cc-cbd693f92e5b.jsonl`.
   - For each tool call (`exec_command`, `write_stdin`, `apply_patch`, `shell` or whatever it names them), note what it returned: an exit code and output, a "still running" session ID, or an error. Note too whether the four scene files were ever written.
   - Check the run folder `~/Library/Application Support/Syncfusion Video Studio/audio-reviews/prj_01m8054am8gw085257jg/2026-11-23-1150-default/audio/` for `scene1.txt` to `scene4.txt`.
3. **Reproduce outside the app** with `attachments/codex-replay.py`, in an empty folder. The prompt asks for one command and one patch:

   ```bash
   mkdir -p /tmp/replay1 && python3 -I attachments/codex-replay.py /tmp/replay1 "Run \`cutline --version\`. Then, in one apply_patch call, add audio/scene1.txt and audio/scene2.txt with one short sentence each. Then stop." /tmp/replay1.jsonl 180
   ```

   - Add `REPLAY_MCP_OFF=computer-use,node_repl` to match the app, which turns off those MCP servers because this Mac's config defines them.
   - Add `REPLAY_MODEL=gpt-6.1-sol` to match the failing run's model.
   - The script prints each item with "completed NEVER" when Codex never reported it finished. On the Mac mini both items complete within 0.1 s.
4. **If it reproduces, find the setting.** Run the same replay with a clean Codex home holding only the sign-in: `mkdir /tmp/codexhome && cp ~/.codex/auth.json /tmp/codexhome/`, then `CODEX_HOME=/tmp/codexhome python3 -I …`. If that completes, copy the settings from `~/.codex/config.toml` back one group at a time (features, plugins such as computer use, MCP servers, model and service tier) until it stalls. Watch for the `cua_repl` MCP server, which the failing thread started and the Mac mini never does.
5. **If it doesn't reproduce outside the app,** run a real Audio Review there and compare its `logs/codex.jsonl` with the replay's log. The difference is in what the app sends: the `thread/start` config, `shell_environment_policy` with the `.bin` PATH and `VIDEO_STUDIO_RUN_CONTEXT`, `developerInstructions`, or the size of the first message.
6. **Fix it on the app's side** where the cause is something the app controls. For example, override the offending setting in `CodexOptions::thread_params` (`crates/video-studio-core/src/codex/agent.rs`), as the app already does for MCP servers. If not, report it with what Codex needs. Add a test that the `thread/start` request carries the override (`tests/codex_runs.rs` logs what the fake Codex receives).
7. **Verify** with a real Audio Review on that Mac: every command and both file changes complete, and `outputs/audio-review.wav` plays. Then check the open boxes above, set `status`/`fix`/`verified` to match, and add an Implementation log entry.

## Implementation log

### 2026-11-23

- Implemented: Documented the recurring stall and issue-report defects with independent reproduction and verification fixtures.
- Not done: Runtime diagnosis, product fixes and verification await approval.
- Files: This bug README.md.

### 2026-11-23 — Fix

- Traced with codex-cli 0.171.0 (this Mac), by replaying the app's handshake against `codex app-server` in disposable workspaces. The replay used `approvalPolicy: untrusted`, the workspace-write sandbox with only the folder writable, and `/tmp` and `$TMPDIR` excluded, and it accepted every approval:
  - A four-file `audio/scene1–4.txt` patch, then `summary.md`: item `exec-1c4f…6289` started at 6.64 s, its approval (request 0) was accepted at once, and it completed at 6.71 s with the files on disk. The second item completed 0.03 s after it started.
  - The same patch applied with `apply_patch` through the shell: Codex routes it through a `fileChange` item (`exec-6883…5b08`), which completed 0.05 s after approval.
  - The same patch in a folder under `~/Library/Application Support`: it completed 0.07 s after approval.
  - Four real Audio Reviews today sent `item/completed` for every approved change.

  No divergence on this Mac.
- The failing run's `events.ndjson` (from the owner) shows the stall is inside Codex, not in delivery:
  - "Writing `audio/scene1.txt`…" through `scene4.txt` at 16:51:09 (the approval was answered).
  - Several model calls without new tool calls.
  - Codex saying "File creation is still pending" at 16:52:18.

  Codex itself saw its patch unfinished. The run was on another Mac (library under `/Users/danieljebaraj/…`), with a Codex CLI version the report doesn't record. That run's `logs/codex.jsonl` holds its version and the patch's messages.
- Implemented (`codex/agent.rs`):
  - The driver keeps each file change's entries.
  - When an accepted change passes its deadline, or is still unreported when the turn ends, each entry is checked on disk: an added file must hold exactly the added text, a deleted one must be gone, and an updated one must hold the new side of every hunk, in order.
  - A change whose files all match is closed as written, with a `patch_applied_unreported` host note in `codex.jsonl`, and the run goes on. Codex's late completion of it isn't reported again.
  - Otherwise the stop says, per file, "is missing", "matches the change", "doesn't match the change" or "is still there".
  - The driver records the Codex CLI version (`thread/start`'s `cliVersion`) in `run.json` (`codex.version`).
- Implemented (`logs.rs`):
  - The issue report and the export README name the assistant from the recorded provider: "Codex 0.171.0", "Claude Code …", or "assistant not recorded" for an older record that doesn't say.
  - A failed run's `summary.md` is shown as the assistant's draft summary, quoted, under the run's result. On a successful run it stays the summary.
  - The warning collector takes only what cutline emitted: `⚠ CODE path: message` lines, `warning:` lines, `WARN` log lines and the `warnings` of a `--json` result. The schema's descriptions, `--help` text and an empty `"warnings": []` add none.
- Tests:
  - `a_codex_patch_written_but_never_reported_counts_once_its_files_match`: the fake's new `silent` mode writes the files without reporting; the run goes on to a WAV, the host note names the files, and each write is reported once.
  - `a_codex_patch_never_reported_applied_stops_the_run_with_the_files_named` now checks "is missing" for each file.
  - `a_file_change_is_checked_on_disk`.
  - `a_codex_issue_report_names_codex_and_only_emitted_warnings`: the supplied fixture, plus a positive control and the Claude and legacy cases.
  - The Codex run test checks `codex.version`.
  - The fake's file changes now carry the added text as `diff`, as real Codex does.
  - Workspace: 556 tests pass.
- Not done: identifying and correcting the divergence. It needs the failing run's `logs/codex.jsonl`. Until then the safeguard lets a run go on when Codex applied a change without reporting it, and stops with each file's state when it didn't.
- Files: `crates/video-studio-core/src/codex/agent.rs`, `src/logs.rs`, `src/run/orchestrator.rs`, `crates/video-studio-cli/src/bin/fake_codex.rs`, `tests/codex_runs.rs`.

### 2026-11-23 — The failing run's protocol log

- The owner supplied the failing run's `logs/codex.jsonl`. It came from the MacBook Pro (`daniels.macbook.pro.lan`), with codex-cli 0.171.0, model `gpt-6.1-sol` at reasoning effort `low`, and `approvalPolicy: untrusted`. Every successful run here was on a Mac mini with the same CLI.
- First divergence: Codex sent no `item/completed` for anything it ran in that turn, not just the patches.
  - The five accepted commands (`cutline schema`, `cutline doctor`, `cutline init --help`, two quotes, `eleven music`) all started, were approved (requests 0, 2 and 4–8) and ran: `doctor` streamed its output, and the music file and its credits reached the host. None was reported finished.
  - Neither file change completed: `exec-690a…cf34` (the four scene files, request 1) and `exec-1ab4…822e` (`summary.md`, request 9).
  - Only the declined `ls audio` completed, as `declined`.
  - Between them Codex called the model every ~3 s, with about 40k tokens in and 30–100 out each time, plus one `terminalInteraction` with empty stdin on the music command. Codex was polling sessions it considered still running.
  - At 16:52:18 Codex said "File creation is still pending". After the host's `turn/interrupt`, the turn ended `interrupted` with `items: []`, and stderr shows "failed to record rollout items: thread … not found".
- What differs from this Mac:
  - The user's Codex config lists the MCP servers `computer-use` and `node_repl` (the app turns them off).
  - The thread still started `cua_repl`, part of Codex's computer- and browser-use features, which never appears here.
  - The thread reports `serviceTier: default` where this Mac has `null`.
- Ruled out by replays on this Mac:
  - A login profile (`zsh -lc`) that leaves a background process holding the terminal: commands still completed.
  - `gpt-6.1-sol` instead of `gpt-6.2-sol`: completed.
  - A run folder under `~/Library/Application Support`: completed.
- Next: Codex's session file for the thread (`~/.codex/sessions/2026/11/23/rollout-2026-11-23T11-50-45-01a20951-….jsonl` on the MacBook Pro) holds what each tool returned to the model. `~/.codex/config.toml` (features, plugins, MCP servers) holds what is set up differently there. Both are needed to name the cause.

