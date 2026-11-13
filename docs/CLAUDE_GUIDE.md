# cutline — guide for Claude

`cutline` is a declarative video editor: you write a **project file** (JSON5) that describes the
edit, and cutline validates, previews, reviews and renders it. Every command accepts `--json`
(machine output on stdout, logs on stderr). Exit codes: 0 ok · 1 validation/QA error · 2 usage ·
3 tool/dependency · 4 internal.

## Default workflow

1. **Tools.** `cutline doctor`. If anything is red: `cutline deps install --all` (ffmpeg, whisper
   model, face model, Chrome headless shell). `cutline deps status` shows versions vs the lockfile.
2. **Understand the footage.**
   - `cutline probe <file>...` — duration, fps (VFR flag), rotation, color, alpha, loudness (`--loudness`).
   - `cutline transcribe <file>` for spoken footage (or `--import-scribe vo.scribe.json` for an
     ElevenLabs Scribe export; no API calls are ever made). Read the transcript to find the story:
     the **hook** (strongest line, first 1–3 s), the key beats, and the **CTA**.
3. **Write the project.** `cutline init --target shorts --assets <dir>` scaffolds `project.json5`,
   a brand file and folders. Fill in `main` (story order), `layers`, `captions`. Prefer `auto`
   directives for jump cuts and fillers (`auto: {jump_cuts: {}, remove_fillers: {}}`).
   `cutline templates list` shows templates, transitions, effects, easings, caption styles,
   color presets, LUTs, emoji and fonts. `cutline schema` prints the JSON Schema.
4. **Validate and inspect.** `cutline validate project.json5` — fix every error (each has a
   `code`, a JSON-pointer `path` and usually a `hint`). Then `cutline expand project.json5` to see
   the explicit timeline (jump cuts, removed fillers, generated punch-ins) as JSON + an EDL table.
5. **Look at it.** `cutline review project.json5` writes `review/`:
   - `contact_sheet.jpg` (timecode + clip id per tile), `cuts/*.jpg` (before | after each cut),
     `captions/*.jpg` (safe zone shaded, caption box outlined), `reframe/*.jpg` (crop rectangle on
     the source), `audio.png` (per-role waveforms, final mix, duck envelope, momentary LUFS vs
     target), `timeline.txt`, `qa.json`, `summary.md` (QA + suggested improvements),
     `preview_540p.mp4`.
   - Read `summary.md` first, then look at the images. Iterate until there are **no QA errors** and
     the suggestions are addressed. Use `cutline preview project.json5 --at 3.2 7.5 --safe-zones`
     for quick stills while iterating (drafts capture HTML graphics at half density and 15 fps;
     stills skip frame capture for graphics not on screen at the requested times).
6. **Render.** `cutline render project.json5` (all exports; `--export name` for one). QA runs
   automatically; the command fails on QA errors unless `--allow-qa-errors`. Outputs next to the
   video: `<name>.qa.json`, `<name>.render_report.json`, `<name>.srt`, `credits.txt`. Then:
   `cutline chapters project.json5` (long form) and `cutline thumbnail project.json5 --at <t>
   --text "..."`. Report the output paths and the **publishing checklist** (printed by `render`).

`render --dry-run` prints every ffmpeg command and filter script (reproducible renders).
Segments are cached: changing captions, text or music does not re-render clips.

## Shorts style defaults (target `shorts`, 1080×1920, ≤ 180 s)

- Hook text in the **first second** (`hook_title` layer) — QA warns otherwise.
- Captions `bold_pop` (1–3 words, current word in accent with a pop); emphasis words in
  `captions.emphasis`, optional emoji via `captions.emoji: {word: "fire"}`.
- A **visual change every 3–5 s**: cut, `punch_in`, B-roll, a graphic. `auto.punch_ins` helps.
- Music ducked under the voice (automatic when a music layer exists; tune `audio.duck`).
- SFX on transitions sparingly (`auto.sfx_on_transitions: "whoosh"` + brand SFX aliases).
- `progress_bar` layer on. End with a loopable last line or a CTA (`cta_subscribe`).
- Keep everything inside the safe zone (top 10 %, bottom 22 %, right 16 % reserved by the UI).
- Reframe 16:9 footage with `reframe: {mode: "track_face"}` (needs the face model; falls back to
  `fit_blur` with a warning), or `fit_blur` / `cover` / `manual` keys / `split` for two people.

## Long-form defaults (`long_1080p`, `long_4k`)

- `clean_sentence` captions (or none), `lower_third` for names, `chapter` on clips (or `markers`)
  so `cutline chapters` yields ≥ 3 chapters of ≥ 10 s starting at 00:00.
- B-roll every 20–40 s (`broll` layers, PiP with `radius`/`border`/`shadow` or full-frame).
- Leave the last 20 s free for the YouTube end screen (`end_card` template draws placeholders).

## Screen recordings

- Set `profile: "screencast"` (sharper text encoding; no QA warnings for paused screens). Add
  `chroma: "444"` for downloads/docs where colored text must not fringe.
- Point at UI with `{type: "zoom_to", rect: {x, y, w, h}, start, end}` instead of working out
  zoom scales and focus points.

### Hide private details

- Look for usernames, home paths, emails, tokens and hostnames in every frame (`review`'s
  contact sheet) and cover them with `mask` layers: `{type: "mask", rect: {x, y, w, h},
  anchor: "clip", duration: 2}`; `rect` is in fractions of the frame.
- `blur` (default) suits incidental text; use `mode: "fill"` with the background `color` for
  secrets, optionally with replacement `text` (e.g. `~/Library/…`) so the UI still reads well.

## Narrated videos: time the edit to the voice-over

- Put the VO on an `audio` layer with a numeric `start`, and give its asset a `transcript` (or
  run `cutline transcribe` once so it is cached).
- Name the beats under `cues` (`{clocks: "World clocks"}`) and use `"cue:clocks"` in timeline
  times: clip `end_at` (the cut lands on the cue), layer `start`/`end`, sfx `at`, marker `at`.
  Offsets: `"cue:clocks+0.3"`.
- `end_at` fits each clip: give `in`+`out` to let cutline pick the speed, or only `in` to trim.
  No hand arithmetic for clip lengths, speeds or transition overlaps.
- Set the VO asset's `script` to the text you generated it from: captions then show your exact
  wording (names, capitals, punctuation) instead of recogniser slips, and cue phrases copied
  from the script match exactly. Audio tags like `[warm]` are ignored.
- `cutline cues project.json5` shows what each cue matched and where it landed; fix
  `W_CUE_FUZZY` / `E_CUE_NOT_FOUND` from the hint.

## Audio

- Music under narration: set `audio.music_level: 18` instead of guessing `gain_db`, and put
  `align: "end"` (with a `fade_in`) on the music layer so the track's real ending closes the
  video rather than a fade-out mid-phrase.

- Roles: `dialogue` (clip audio), `voice` (VO assets), `music`, `sfx`. Voice/dialogue chain
  `clean_voice` by default (`podcast_warm`, `raw` available).
- Loudness: two-pass normalization to −14 LUFS / −1 dBTP by default (`audio.loudness`).
- Ducking uses transcript word timings when available, otherwise a VAD; see `audio.png`.
- ElevenLabs VO/music/SFX files are just assets: give VO `role: "voice"` and its Scribe JSON as
  `transcript`.

## Generated graphics (Claude Motion, HTML)

- Any ffmpeg-decodable video works as an asset; alpha overlays: ProRes 4444, WebM VP9 alpha,
  PNG sequences (`frames/%05d.png`).
- HTML / SVG / Lottie assets render through headless Chrome, frame by frame, with transparent
  background. Seek hooks: CSS/Web Animations are paused and seeked automatically; implement
  `window.__cutline_seek = async (t) => {...}` for JS-driven animation. Asset `params` arrive as
  `window.__cutline_params`. Network is blocked (only local files next to the page / in the project).
- HTML variants of templates: `hook_title_html`, `lower_third_html`, `end_card_html`.

## Fixing common errors

| Code | Fix |
|---|---|
| `E_TIME_OUT_OF_RANGE` | in/out beyond the media; check `cutline probe` duration or use `"end"` |
| `E_TRANSITION_TOO_LONG` | shorten the transition or lengthen a neighbour |
| `E_UNKNOWN_ASSET` / `E_UNKNOWN_ANCHOR` | typo in an asset or clip id |
| `E_UNKNOWN_FIELD` | the hint lists allowed fields |
| `E_EFFECT_OUT_OF_RANGE` | effect `at` is clip-relative (seconds from the clip's start on the timeline) |
| `W_SHORTS_OVER_LIMIT` | trim to ≤ 180 s or use a long-form target |
| `W_REFRAME_NO_FACES` | install the face model, choose `manual` keys or another mode |

## Publishing checklist (printed after render)

- YouTube **altered/synthetic content** disclosure when realistic AI-generated people, voices or
  scenes are present (an AI voice narrating real footage may qualify).
- Music and SFX rights confirmed (ElevenLabs plan / licenses).
- Chapters in the description (long form). Thumbnail exported. Upload the `.srt`.
