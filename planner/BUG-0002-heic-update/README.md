---
id: BUG-0002
type: bug
title: HEIC update
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

# HEIC update

## Summary

Bash is available now, so I'll convert the HEIC photos to JPEG in a temporary folder and look at each one.

PreToolUse:Bash hook error: The command uses a variable or command substitution, which isn't allowed. Run cutline (or `video-studio eleven` / `video-studio capture`) as a plain command, one per call, with no pipes, redirections or substitutions. This is blocked by Syncfusion Video Studio.

PreToolUse:Bash hook error: `cutline still` must write its copy into photo-copies/ in your working folder, with --out. This is blocked by Syncfusion Video Studio.

The photos are still out of reach, and the spec is back at its original nine lines, so I'm pausing until the photo problem is sorted.

**What blocked the photos this time.** The shell is now allowed, but Syncfusion Video Studio restricts it to a single command, `cutline still`, which makes a JPEG copy of a HEIC photo. The installed cutline is version 0.1.0 and does not have that subcommand. Every other command, including the macOS image converter, is blocked by the Studio's hook. The exact error:

```
error: unrecognized subcommand 'still'
cutline 0.1.0
```

**Two ways to unblock it.** Either one works, and I'll finish the spec as soon as the photos are visible.

1. Update cutline to a version that includes the `still` subcommand, then tell me to try again.
2. Export the seven photos as JPEG or PNG into Downloads. In Photos on the Mac, select them, then File, Export, Export 7 Photos, and choose JPEG. Re-attach them to the project so they get media IDs.

**Alternatively, describe them.** If you'd rather not convert, tell me in one line each what the seven photos show, for example "7206 is the Roku remote, 7210 is the file list". I'll place them from your description.

**About the spec.** My full rewrite from earlier was not kept, so the file holds only your original notes. Once I can see the photos I'll write the complete tutorial again with the images placed correctly and the zooms aimed at the right buttons and menu lines.

## Steps to reproduce

1. Have an older cutline (built before FEAT-0004, so without `cutline still`) where Video Studio finds it first: the path in Settings › cutline, on PATH, or in `~/.local/bin`, `/opt/homebrew/bin` or `/usr/local/bin`. It still reports `cutline 0.1.0`.
2. Follow the BUG-0001 steps: HEIC photos (`IMG_7206.HEIC` … `IMG_7212.HEIC`) in a media or reference folder, not in the media list.
3. In the Claude tab, ask the assistant to use the photos in the spec.

## Expected result

Claude makes JPEG copies with `cutline still` and looks at them. If that can't work, Claude is told why up front and asks the user once to update cutline, with no failed attempts.

## Actual result

Claude tried a temporary folder with a variable (denied: substitution), then wrote the copy outside `photo-copies/` (denied), then ran `cutline still` and got `error: unrecognized subcommand 'still'` from the old cutline.

## Cause

- `find_cutline` returned the first cutline that answered `--version`. Every build reports `0.1.0`, so an old copy earlier in the search order hid the current build, and nothing checked for `still`.
- The assistant was given Bash and told to use `cutline still` whenever any cutline was found, even one without that command.
- The denial messages didn't give the exact command: the substitution error even suggested `video-studio eleven` / `video-studio capture`, which the assistant can't run. The prompt used a relative `photo-copies/` path, the folder didn't exist yet, and the prompt didn't rule out variables or temporary folders.

## Fix

- [x] `CutlineInfo` has `still`. `find_cutline` checks `cutline still --help` on each candidate, returns the first one that has it, and falls back to the first working one with `still: false` (`crates/video-studio-core/src/claude/locate.rs`).
- [x] The assistant checks the cutline it's given. With `still` it gets Bash, an existing `photo-copies/` folder, and a prompt with the exact command, the absolute copies path, and "no variables, `$(…)`, `~`, pipes, `&&`, `mkdir` or temporary folders". Without `still` it gets no shell, and the prompt names the old cutline and says to ask the user once to update it (`assistant.rs`, `run/prompt.rs`, `viewable.rs`).
- [x] In the assistant, broker denials (substitution, wrong output folder, any other command) end with the exact command to run, filled in with the cutline and the absolute `photo-copies/` path. The `still` check runs before the general output-folder check (`claude/broker.rs`).
- [x] Runs offer `cutline still` only when the cutline has it (`PromptInput::cutline_still`). A media-list copy made with an old cutline fails with "this cutline is an older build … update cutline" instead of clap's error.
- [x] Readiness: a new `cutline_outdated` warning names the old cutline and how to update it (`validation.rs`).
- [x] Tests: `find_cutline_prefers_one_with_still`, `assistant_still_denials_spell_out_the_command` (replays the two denied commands from this report), `assistant_prompt_outdated_cutline`, `run_prompt_heic_depends_on_cutline_still`, and the `cutline_outdated` case in the environment checks.
- [ ] Verified in the macOS app with real iPhone photos and an updated cutline (steps above).

## Implementation log

### 2026-11-19
- Implemented: every fix step above except checking it in the app.
- Not done: checking it in the macOS app. Also, the cutline the app found on that Mac needs updating (rebuild, or `scripts/install-video-studio-app.sh`) before HEIC photos work. The app now warns about it instead of failing.
- Files: crates/video-studio-core/src/claude/locate.rs, crates/video-studio-core/src/claude/broker.rs, crates/video-studio-core/src/assistant.rs, crates/video-studio-core/src/run/prompt.rs, crates/video-studio-core/src/run/prepare.rs, crates/video-studio-core/src/viewable.rs, crates/video-studio-core/src/validation.rs
- Commit/PR: see the commit "Fix BUG-0002" on main (it also makes app builds build and use the latest cutline)

