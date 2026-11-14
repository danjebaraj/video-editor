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
   - Read `summary.md` first, then look at the images. It flags text templates that sit over
     detailed content (measured from frames with and without the text). Iterate until there are **no QA errors** and
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

## Business videos and product demos

- Start with `cutline init --style business` (target `long_1080p`, `profile: "screencast"`,
  `clean_sentence` captions, business brand, no progress bar).
- Put the company look in the brand file once: `logo` (watermark, top right by default),
  `intro` / `outro` bumpers, colors and fonts. Every project using the brand gets them;
  `brand_bumpers: false` or `watermark: false` opt a project out.
- Times in the project count from the start of the edit, after the intro. Do not add the intro
  length to anything.
- Structure with the business cards: `title_card` to open, `section_card` between parts (they
  become the chapters), `feature_list` with `reveal` on cues while the narration lists
  features, `stat` for one number that matters, `testimonial` / `logo_wall` for proof,
  `compare` for before/after, and `cta_link` to close (headline, URL, QR code). Put full-frame
  cards over a `path: "color:brand"` clip; `feature_list` and `stat` default to a scrim so
  they can sit over the product.
- Deliver per channel from the one edit with `exports`: the full video, a `portrait_4x5` cut
  for LinkedIn, a silent looping hero for the website (`audio: false, loop: true, max_bytes`,
  usually a `range` of the best 10 seconds), `format: "webm"` where the site wants it. Silent
  exports must read without sound: keep captions burned in and fix every `muted` QA warning.
  Each export gets a `.vtt` and a `.poster.jpg` for the embed.
- One project, several videos. `variants` for cut-downs (a teaser: `exclude` clips, give it a
  shorter voice-over, set `max_duration`); `locales` for languages (a voice-over and script per
  language, cue phrases in that language, and `"@key"` strings for on-screen text). Time
  everything with cues and `end_at` so each version fits its own narration, then check every
  one: `cutline review project.json5 --variant teaser --locale de`.
- Tone: no punch-in cadence, no emoji, no `bold_pop`. Plain cuts between screens, one fade from
  the title and into the end card. Keep captions on: most business video is watched muted.

## Screen recordings

- Record web apps from a script, not by hand: write a `.demo.json5` (viewport, `url`, steps:
  `goto`, `wait`, `click`, `type`, `press`, `scroll`, `hover`), use it as the asset path, and
  name the steps that matter. Then edit by name: `in: "step:export-1"`,
  `{type: "zoom_to", step: "export"}`, `{type: "highlight", step: "export"}`,
  `{type: "mask", step: "email", mode: "fill"}`. When the UI changes, the recording and every
  rect follow; nothing is measured by hand. Start each script with a `wait` for the page's key
  element, keep `scale: 2`, give credentials as `$ENV_VAR`, and list third-party origins the
  page needs in `allow`. `cutline record project.json5` records again on demand.
- With a browser recording, start from `auto: {cursor: {}, zoom_on_clicks: {}}`: the pointer
  is drawn from the recording (with click ripples and shortcut badges) and the view follows
  the clicks. Add `fast_forward_idle: {}` for loading and long typing. Read `cutline expand` to
  see what they made, then override by hand only where needed (a clip with its own zoom is
  left alone).
- Record terminal footage from VHS tapes: use the `.tape` file itself as the asset path and cutline
  records it (and re-records only when the tape changes; run `cutline record project.json5` when
  the screen shows live data you want fresh). End each tape by quitting the program
  it started (e.g. `Hide`, `Type "q"`): VHS kills the terminal, and some programs then linger.
- Set `profile: "screencast"` (sharper text encoding; no QA warnings for paused screens). Add
  `chroma: "444"` for downloads/docs where colored text must not fringe.
- Point at UI with `{type: "zoom_to", rect: {x, y, w, h}, start, end}` instead of working out
  zoom scales and focus points. Zooms are sub-pixel smooth and use the recording's full
  resolution, so do not pre-crop or pre-scale footage outside cutline.
- Frame the recording: `default_frame: {style: "browser", url: "app.example.com"}` puts every
  recording in a window on the brand background (`window` for desktop apps, `phone` / `tablet`
  for mobile recordings). It reads as a product shot instead of a raw capture, and it is the
  right way to show a 16:9 recording in a vertical video (prefer it to `fit_blur`). Rects for
  `zoom_to` and annotations stay relative to the recording.
- Show where to look. Pair a `zoom_to` with a `highlight` on the same `rect` (it follows the
  zoom), use `spotlight` when the rest of the screen distracts, an `arrow` with a short `label`
  for "new here", `keys` when the narration names a shortcut, and a `cursor` layer over
  recordings made without a pointer. One device at a time: a highlight or a spotlight, not both.
  Time them with cues (`start: "cue:export"`), and check `review/annotations/`.
- **Keep recordings at 1×.** `end_at` trims a clip to its slot; it never changes the speed. If
  `W_FIT_HOLD` says a clip runs out of footage, give it more footage (earlier `in`, later
  `out`), cut sooner, or accept the held last frame when the screen is still anyway. Use
  `speed`/`fit: "speed"` only for a deliberate fast-forward or slow-down.
- **Cut between app shots; do not dissolve.** A dissolve between two screens shows both
  screens' text at once (`W_SCREEN_DISSOLVES`). Put plain cuts between recordings (no
  `transition` item), and keep a `fade` for the title card and the end card. When a change of
  place needs marking, use one moving transition (`slideleft`, `zoomin`, `whip_left`), not many.
- Let each shot settle: the action a line of narration describes should start a beat after the
  cut, and the result should stay on screen about a second before the next cut.

### Hide private details

- Look for usernames, home paths, emails, tokens and hostnames in every frame (`review`'s
  contact sheet) and cover them with `mask` layers: `{type: "mask", rect: {x, y, w, h},
  anchor: "clip", duration: 2}`; `rect` is in fractions of the frame.
- `blur` (default) suits incidental text; use `mode: "fill"` with the background `color` for
  secrets, optionally with replacement `text` (e.g. `~/Library/…`) so the UI still reads well.

## Narrated videos: time the edit to the voice-over

- Put the VO on an `audio` layer with a numeric `start` (or `anchor` it to a clip, e.g. the one after
  the title), and give its asset a `transcript` (or
  run `cutline transcribe` once so it is cached).
- Name the beats under `cues` (`{clocks: "World clocks"}`) and use `"cue:clocks"` in timeline
  times: clip `end_at` (the cut lands on the cue), layer `start`/`end`, sfx `at`, marker `at`.
  Offsets: `"cue:clocks+0.3"`.
- `end_at` fits each clip by trimming at normal speed: give `in` (and `out` as a limit). No
  hand arithmetic for clip lengths or transition overlaps. `W_FIT_HOLD` means the footage ended
  before the cut and the last frame is held.
- Set the VO asset's `script` to the text you generated it from: captions then show your exact
  wording (names, capitals, punctuation) instead of recogniser slips, and cue phrases copied
  from the script match exactly. Audio tags like `[warm]` are ignored.
- `cutline cues project.json5` shows what each cue matched and where it landed; fix
  `W_CUE_FUZZY` / `E_CUE_NOT_FOUND` from the hint.

## Audio

- **Cut on the music.** `cutline beats project.json5` lists the bars and beats on the timeline.
  Never work out beat times by hand:
  - Narration-led edit: time cuts with `end_at: "cue:…"` and add `auto: {snap_to_beats: {}}`.
    Each cut moves to the first beat of a bar when one is within 0.35 s (`downbeat_reach`),
    otherwise to the nearest beat within 0.15 s (`max_shift`). `cutline expand` lists the moves.
  - Music-led edit (montage, title sequence): write `end_at: "bar:5"` (or `"bar:5.3"`,
    `"beat:17"`).
  - Bar starts carry the weight: put section changes and the title/end card on a bar; use
    in-between beats for quick cuts inside a section.
  - A beat reads as a cut only when the picture changes at once: use plain cuts on beats. A
    dissolve centred on a beat feels late and soft.
  - The music layer needs a numeric `start` for `bar:`/`beat:` times (use `src_in` to choose
    where the track begins); `align: "end"` cannot be combined with them.
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
