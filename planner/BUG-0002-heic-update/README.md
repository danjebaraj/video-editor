---
id: BUG-0002
type: bug
title: HEIC update
status: open
fix: none
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
