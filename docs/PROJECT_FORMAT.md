# Project format (schema v1)

A project is a JSON5 file (comments and trailing commas allowed; plain JSON works). The JSON
Schema is `schema/project.schema.json` (`cutline schema`). Unknown fields are errors. Paths are
relative to the project file.

## Times

Anywhere a time is accepted: seconds (`12.4`), timecode (`"01:02.500"`, `"00:01:02.500"`),
frames (`"f:372"`, at the source fps for source times, the timeline fps otherwise), or `"end"`.
Cuts snap to the nearest output frame.

Timeline times (layer `start`/`end`, sfx `at`, clip `end_at`, marker `at`) can also be **cue
references**: `"cue:clocks"`, `"cue:clocks+0.4"`, `"cue:clocks-0.2"` (seconds after/before the
cue). See [Cues](#cues).

The same fields take **beat references** from the project's music layer: `"bar:9"` (first beat of
bar 9), `"bar:9.3"` (its third beat), `"beat:33"`, each with an optional `+0.1`/`-0.1` seconds.
Bars and beats count from 1 at the first one on or after timeline 0; `cutline beats` lists them.
They need exactly one audio layer with role `music` and a numeric `start` (no `align`/`anchor`).
Bars assume 4 beats; `W_BARS_UNCERTAIN` says when the bar starts could not be told apart.
Errors: `E_BEAT_REF`.

## Top level

| Field | Type | Default | Notes |
|---|---|---|---|
| `version` | `1` | required | |
| `name` | string | file stem | output file name |
| `target` | `shorts` · `long_1080p` · `long_4k` · `square` · `portrait_4x5` · `custom` | `shorts` | geometry, fps, encoder, safe zone |
| `fps` | number or `"30000/1001"` | target (long-form follows the first video clip) | |
| `width`, `height` | even int | — | for `custom` |
| `brand` | path or `"default"` | `brand/default.json` if present, else bundled | |
| `assets` | map id → Asset | `{}` | |
| `main` | [Clip or `{transition}`] | `[]` | the storyline |
| `layers` | [Layer] | `[]` | overlays, text, audio |
| `captions` | Captions | none | |
| `audio` | AudioSettings | defaults below | |
| `auto` | AutoDirectives | none | expanded by `cutline expand` |
| `default_frame` | ScreenFrame | none | screen frame for every video clip; see [Screen frame](#screen-frame-frame-on-a-clip-default_frame-on-the-project) |
| `default_reframe` | Reframe | `{mode: "cover"}` | geometry when source aspect ≠ target |
| `safe_zone` | `{top, bottom, left, right}` fractions | per target | |
| `markers` | [`{at, title}`] | `[]` | chapters |
| `exports` | [Export] | one export of `target` | multi-target from one edit |
| `cues` | map name → Cue | `{}` | phrases in the voice-over; see [Cues](#cues) |
| `variants`, `locales`, `strings` | maps | `{}` | several cuts and languages from one project; see [Variants and locales](#variants-and-locales) |
| `brand_bumpers` | bool | `true` | `false`: do not add the brand's `intro` / `outro` around `main` |
| `watermark` | bool | `true` | `false`: do not draw the brand `logo` as a watermark |
| `profile` | `standard` | `screencast`: tuned for screen/terminal recordings — x264 `-tune animation` and CRF − 2 for sharp text and UI edges; QA's freeze check passes (pauses are normal) |
| `chroma` | `"420"` | `"444"`: full color resolution (no fringes on small colored text; ProRes 4444 segments, H.264 High 4:4:4). Not every player supports 4:4:4 and platforms re-encode to 4:2:0, so use it for downloads and docs. Drafts and `--hw` stay 4:2:0. Exports can override |

### Targets

| Target | Size | fps | Safe zone (t/b/l/r) | Limit |
|---|---|---|---|---|
| `shorts` | 1080×1920 | 30 | 10 % / 22 % / 6 % / 16 % | ≤ 180 s |
| `long_1080p` | 1920×1080 | follows source (24/25/30/50/60) | 5 % / 8 % / 5 % / 5 % | |
| `long_4k` | 3840×2160 | follows source | same | |
| `square` | 1080×1080 | 30 | 6 % / 10 % / 6 % / 6 % | |
| `portrait_4x5` | 1080×1350 | 30 | 6 % / 10 % / 6 % / 6 % | LinkedIn / Instagram feed |
| `custom` | `width`×`height` | follows source | by orientation | |

## Asset

| Field | Default | Notes |
|---|---|---|
| `path` | required | `"color:#RRGGBB"` or `"color:brand"`: a plain still of that color. Image sequences use a printf pattern: `frames/%05d.png`. A `.demo.json5` script is recorded in headless Chrome (see [Browser recordings](#browser-recordings-demojson5-assets)). A VHS `.tape` script is recorded with `vhs` when the project loads (cached by the tape's content; `Output` lines are replaced; relative paths in the tape resolve next to it) and used as a video. Editing the tape re-records it; `cutline record project.json5 [--asset id]` records again on demand (fresh clocks, process lists) |
| `kind` | auto (`video`, `image`, `audio`, `image_sequence`, `html`, `lottie`, `svg`) | from ffprobe + extension |
| `role` | — | `dialogue`, `voice`, `music`, `sfx` (audio layers default to `music`) |
| `transcript` | — | Scribe JSON or cutline transcript JSON for this asset |
| `script` | — | the text that was spoken (a `.txt`, e.g. the TTS script). Captions, ducking and cues use the script's exact words, timed from the transcript; `[audio tags]` are ignored. `W_SCRIPT_MISMATCH` when under 70 % of the script is heard |
| `duration` | 3 s | stills / HTML / Lottie / SVG |
| `alpha` | — | assert alpha exists (`E_ALPHA_NOT_PRESENT` if not) |
| `fps` | 30 | image sequences / HTML |
| `params` | — | injected as `window.__cutline_params` for HTML |
| `width`, `height` | target size | HTML/SVG/Lottie render size |
| `synthetic` | false | marks AI-generated media in `credits.txt` |

WebM VP9 alpha is decoded with `libvpx-vp9` automatically.

### Browser recordings (`.demo.json5` assets)

An asset whose path ends in `.demo.json5` is a script that drives a page in headless Chrome
while the screen is recorded. Like a `.tape`, it is recorded when the project loads, cached by
the script (and, for a local page, the files beside it), and recorded again by
`cutline record project.json5 [--asset id]`.

```json5
// rec/export.demo.json5
{
  viewport: {w: 1600, h: 1000, scale: 2},       // CSS px; scale 2 records 3200×2000 for zooms
  url: "http://localhost:3000",                 // or a local page: "app/index.html"
  theme: "light",                               // prefers-color-scheme
  locale: "en",                                 // browser language
  allow: ["cdn.example.com"],                   // other origins the page may load from
  steps: [
    {goto: "/dashboard"},
    {wait: "text=Revenue"},                     // a selector, or seconds: {wait: 1.2}
    {click: "button:has-text('Export')", name: "export"},
    {type: {into: "#email", text: "ada@example.com", delay: 60}, name: "email"},
    {press: "Enter"},                           // or a chord: "Meta+K"
    {scroll: {to: "#invoices", duration: 0.8}}, // or {by: 600}
    {hover: ".tooltip-target"},
  ],
}
```

| Script field | Default | Notes |
|---|---|---|
| `viewport` | required | `w`, `h` in CSS pixels, `scale` 2 (device pixel ratio) |
| `url` | — | base address for relative `goto` paths. The page is opened before recording starts, so there are no blank frames |
| `allow` | `[]` | origins or hosts besides the base address's. **Every other request is blocked** (`W_DEMO_BLOCKED` lists them) |
| `settle` | 0.6 | seconds to stay after each action and at the end; a step's `after` overrides it |
| `fps` | 30 | frame rate of the recording |
| step `name` | — | makes the step usable from the edit |
| step `timeout` | 10 | seconds to wait for the element; then `E_DEMO_STEP_FAILED` names the step and selector |

Selectors are CSS, plus `text=Revenue` (the innermost visible element containing the text) and
`button:has-text('Export')`. Elements off screen are scrolled into view first. Text starting
with `$` is read from that environment variable (`text: "$DEMO_PASSWORD"`), so credentials are
never in the script, the cache or the sidecar.

Capture is in real time (animations and loading play as they happen), resampled to a constant
frame rate. Headless Chrome draws no mouse pointer: the recording comes with an **events
sidecar** (pointer path, clicks, key presses and the rectangle of every element a step
touched), and the edit draws the pointer (`auto: {cursor: {}}`).

**Editing by step.** Named steps replace hand-measured times and rects:

| Where | Form | Becomes |
|---|---|---|
| clip `in` / `out` | `"step:export"`, `"step:export-0.5"` | the moment the step acted (± seconds), in the recording |
| `zoom_to` effect | `step: "export"` | `rect` of the element; `start` when the pointer sets off |
| `punch_in` effect | `step: "export"` | `focus` on the element; `at` the click |
| `highlight` / `spotlight` layer | `step: "export"` | `rect`, anchored to the clip, from when the pointer sets off until a second after the step |
| `mask` layer | `step: "email"` | `rect`, anchored to the clip (its timing is yours) |

`cutline expand` shows the resulting numbers. Errors: `E_UNKNOWN_STEP` (the hint lists the
names), `E_STEP_NO_RECT` (the step touched no element), `E_STEP_NOT_A_RECORDING`,
`E_STEP_NOT_ALLOWED`. Rects are fractions of the viewport, so give the clip a `frame` (or a
target with the viewport's aspect ratio) rather than a cropping `reframe`.

## Clip (`main` item)

| Field | Default | Notes |
|---|---|---|
| `id` | required, unique | anchors, chapters, review labels |
| `clip` | required | asset id |
| `in`, `out` | 0, end of asset | source times |
| `duration` | — | stills; or `in` + duration·speed |
| `end_at` | — | timeline time of the cut to the next clip (the middle of the outgoing transition): `"cue:…"`, `"bar:…"` or seconds. The clip plays at its `speed` (default 1) and is fitted by trimming: footage past the cut is dropped (`out` is then only a limit); footage that ends early holds its last frame and warns `W_FIT_HOLD`. Stills get the duration. Not with `duration` or `speed_ramp`. Jump cuts and filler removal skip fitted clips |
| `fit` | `trim` | with `end_at` + `in` + `out`: `speed` plays exactly `in`..`out` at whatever speed (0.25–4) fills the slot |
| `speed` | 1 | 0.25–8; audio pitch preserved (`atempo`). Any speed other than 1 (also `speed_ramp`, `fit: "speed"`) blends neighbouring frames so motion stays even; text and cursors soften a little while they move, so keep screen recordings at 1 where you can |
| `speed_ramp` | — | `[{t, speed}]`, source-relative `t` |
| `reverse` | false | ≤ 10 s clips |
| `reframe` | `default_reframe` | see Reframe |
| `color` | — | see Color |
| `effects` | `[]` | see Effects |
| `audio` | `keep` | `keep`, `mute`, `duck` (−12 dB) or an asset id replacing it |
| `gain_db` | 0 | |
| `chapter` | — | chapter title starting at this clip |

### Screen frame (`frame` on a clip, `default_frame` on the project)

Draws the clip inside generated window chrome on a background instead of full-bleed:

```json5
default_frame: {style: "browser", url: "app.example.com"},        // every video clip
main: [
  {id: "dash", clip: "screen"},
  {id: "ios", clip: "phone_rec", frame: {style: "phone", background: {gradient: ["#3b1f5b", "#0b0d1a"]}}},
  {id: "raw", clip: "screen2", frame: {style: "off"}},             // full-bleed after all
]
```

| Field | Default | Notes |
|---|---|---|
| `style` | `browser` | `browser` (title bar with `url`), `window` (title bar with `title`), `phone`, `tablet` (rounded bezel), `none` (rounded corners and shadow only), `off` (no frame; opts a clip out of `default_frame`) |
| `url`, `title` | — | text in the bar |
| `background` | `"brand"` | `"brand"` (soft gradient from the brand background), a `"#color"`, `{gradient: [from, to]}` (top-left to bottom-right) or an image asset id (cover-fitted) |
| `padding` | 0.06 | space around the window, as a fraction of the frame's shorter side |
| `radius` | 14 px | corner radius (phones and tablets have their own) |
| `shadow` | 0.5 | drop-shadow strength, 0–1 |
| `theme` | `dark` | chrome colors: `dark` or `light` |

- The recording keeps its aspect ratio and is never cropped; the window is as large as the
  padding allows. On vertical targets it sits in the upper part, leaving room for captions.
- `zoom_to` rects, annotation rects and points stay **relative to the recording**, so adding or
  changing a frame never invalidates them. Zooms use the recording's full resolution.
- `default_frame` applies to video clips only (not stills, HTML or brand bumpers).
- A framed clip cannot also have `reframe` (`E_FIELD_CONFLICT`).

### Transition (`{transition: {...}}` between two clips)

| Field | Default | Notes |
|---|---|---|
| `type` | required | any xfade name (`fade`, `wipeleft`, `slideup`, `circleopen`, `zoomin`, `pixelize`, …) or `whip_left/right/up/down`, `zoom_punch`, `flash_cut`, `glitch`, `cut` |
| `duration` | required | seconds (≤ either neighbour − 1 frame); the timeline shrinks by it |
| `sfx`, `sfx_gain_db` | —, −3 | asset id or brand alias, centered on the transition |
| `audio` | `crossfade` | or `cut` (hard cut at the midpoint with micro-fades) |
| `curve` | `tri` | acrossfade curve |
| `color` | white | `flash_cut` color |
| `ease` | by type | progress curve: `in_out_cubic` for moving transitions (slides, covers, reveals, whips, zooms), `in_out_quad` for fades and wipes; `linear` for a constant rate. Not `back`/`spring` |

With `profile: "screencast"`, two or more `fade`/`dissolve` transitions between video clips warn
`W_SCREEN_DISSOLVES`: text from both screens shows at once mid-dissolve. Cut between app shots.

### Reframe

`mode`: `cover` (default) · `center` · `fit_blur` · `fit_pad` · `track_face` · `manual` · `split`.
`keys` (manual): `[{t, x, y, zoom}]` (clip-relative `t`; `x, y` = crop center in 0..1 of the
source, clamped so the view stays inside the frame; zoom ≥ 1; eased in-out-cubic between keys;
`max_pan_speed` does not apply).
`bias` `{x, y}` (default 0.5, 0.38): where the face sits in the crop. `subject`: `most_persistent`
(default), `largest`, `left`, `right`, `index` + `subject_index`. `max_pan_speed` (frame widths
per second, default 0.6). `fallback` (default `fit_blur`) when no faces. `regions` (split): two
`{x, y, w, h}` rects. `pad_color` for `fit_pad` (default brand background).

### Color

`preset` (`natural`, `punchy`, `warm_film`, `cool_clean`, `bw_contrast`), `lut` (bundled name
or `.cube` path) + `intensity` (0–1), `exposure` (stops), `contrast`, `saturation`,
`temperature` (Kelvin), `tint` (−1..1), `sharpen` (0–2).

### Effects (`at` = seconds from the clip's start on the timeline)

| Type | Fields (defaults) |
|---|---|
| `punch_in` | `at`, `scale` 1.15, `duration` 0.25, `ease` out_cubic, `hold` (release after), `focus` {x,y} |
| `zoom` | `from` 1, `to`, `start` 0, `end` clip end, `ease`, `focus` |
| `zoom_to` | `rect` `{x, y, w, h}` (fractions of the frame), `start` 0, `duration` 0.6, `ease` in_out_cubic, `padding` 0.03, `end` (zoom back out from here). Scale and focus are computed so the rect fills the view (capped at 3×, `W_ZOOM_CAPPED`) |
| `ken_burns` | `from_scale` 1, `to_scale` 1.15, `from`/`to` {x,y}, `ease` |

A clip can zoom to several places in turn: each `zoom_to` (and each `punch_in` / `zoom` with a
`focus`) takes over the view from its start, panning from the previous one. Give a `zoom_to` an
`end` to zoom out before the next, or chain `{type: "zoom", from: 1, to: 1, start, end, focus}`
to pan while staying zoomed.

Zooms (`punch_in`, `zoom`, `zoom_to`, `ken_burns`, `shake`) and reframe moves (`track_face`,
`manual` keys) move in fractions of a pixel, so slow moves glide. When the source has at least 1.4× the target's pixels and the zoom reaches 1.15×,
the zoom is taken from the source's detail (2× supersampling) rather than from the downscaled frame.

| `shake` | `at`, `duration` 0.3, `intensity` 0.02 |
| `flash` | `at`, `duration` 0.15, `color` (white) |
| `blur_in` | `duration` 0.4 |
| `glitch` | `at`, `duration` 0.15, `amount` 12 |
| `vignette` | `strength` 0.4 |
| `motion_blur` | `at`, `duration` 0.15 |
| `freeze` | `at`, `duration` (extends the clip; audio holds silent) |

Easings: `linear`, `in_quad`, `out_quad`, `in_out_quad`, `in_cubic`, `out_cubic`, `in_out_cubic`,
`in_expo`, `out_expo`, `in_out_expo`, `back`, `spring`.

## Layers

All layers take timing: `start` / `end` / `duration` **or** `anchor` (clip id) + `offset`
(seconds after the clip starts — follows ripple edits). Defaults: text 3 s, media its length,
others until the end. Visual layers take: `position` (preset `center`, `top`, `bottom`, `left`,
`right`, `top_left`, `top_right`, `bottom_left`, `bottom_right`, `upper_mid`, `lower_mid`, or
`{x, y}` normalized center), `scale` (fraction of frame width), `rotation` (deg), `opacity`, `z`,
`enter`/`exit` (`fade`, `slide_left/right/up/down`, `pop`, `scale_in`), `anim_duration` (0.3),
`fit` (`cover`/`contain`/`stretch`), `radius`, `border`, `border_color`, `shadow`.

| `type` | Specific fields |
|---|---|
| `text` | `template` (see `templates list`), `text`, `subtitle`, `params` |
| `video` | `asset`, `src_in`, `src_out`, `audio` (false), `gain_db` — alpha video overlays |
| `broll` | same as video + `ken_burns`; full-frame cover unless `scale` is set (PiP) |
| `image` | `asset`, `ken_burns` |
| `html` | `asset` (HTML/SVG/Lottie) |
| `progress_bar` | `style` (`brand` or color), `edge` (top/bottom), `height` |
| `audio` | `asset`, `src_in`, `gain_db`, `fade_in`, `fade_out`, `loop`, `role`, `align` (`end`: backtime so the asset's own ending lands on the layer end — default the timeline end; the head is trimmed, or a shorter asset starts later; not with `src_in`/`loop`) |
| `sfx` | `asset` (or brand alias), `at` or `anchor`+`offset`, `gain_db`, `pick_from` |
| `mask` | `rect` `{x, y, w, h}` (fractions of the frame), `mode` (`blur` default, `pixelate`, `fill`), `color` (fill, default `#000000`), `strength` (default 1), `text` (replacement text, Inter), `text_color` (`#9AA0A6`), `text_size` (px at full size; default 60 % of the rect height). Hides part of the frame over the footage and overlays, under text and captions; until the end by default. Use `fill` for secrets: blurred or pixelated text can sometimes be recovered |

### Annotations

Layers that show where to look. They are drawn over the footage, overlays, masks and the
watermark, and under text and captions. Coordinates are fractions of the frame (the same as
`mask` and `zoom_to`, so one rect serves a `zoom_to` and its `highlight`). Default length 3 s.

| `type` | Fields (defaults) |
|---|---|
| `highlight` | `rect`, `style` (`outline` · `fill` · `underline`), `color` (`brand` accent), `width` 4 px, `radius` 8 px, `enter` (`draw`: the stroke draws itself · `fade` · `none`) |
| `spotlight` | `rect`, `dim` 0.6 (how dark the rest gets), `feather` 12 px, `radius` 8 px. Spotlights on screen together share one dim layer |
| `arrow` | `from` `{x, y}`, `to` `{x, y}`, `label` (text behind the tail), `color`, `width` 6 px, `size` 34 px (label) |
| `keys` | `keys` e.g. `["cmd", "K"]` (cmd, shift, alt, ctrl, enter, tab, esc, space, arrows become keycap symbols), `position` (`bottom`), `size` 64 px |
| `badge` | `text` (a few characters, e.g. `"4×"`), `position` (`top_right`), `size` 30 px: a small dark pill |
| `cursor` | `path` `[{t, x, y, click}]` (`t` = seconds after the layer starts; the pointer eases between keys and ripples on `click: true`), `size` 1, `click_ripple` true, `color` (ripple). Ends 0.6 s after the last key by default |

Pixel sizes are for a 1080p-class output and scale with drafts. `highlight`, `spotlight`,
`arrow` and `cursor` **follow the zoom** of the clip underneath (`zoom_to`, `punch_in`, `zoom`,
`ken_burns`): the rect is written for the unzoomed frame and stays on its target while the
picture zooms. With `anchor` the clip is the anchor clip; otherwise the clip on screen. Set
`follow: false` for screen coordinates. A rect or point outside the frame is
`E_RECT_OUT_OF_FRAME`. `cutline review` writes `annotations/*.jpg`, a frame of each.

Text templates: `hook_title`, `lower_third`, `callout`, `step_counter`, `big_number`, `quote`,
`cta_subscribe`, `end_card`, `emoji_pop`, the [business cards](#business-cards) below, and HTML variants `hook_title_html`,
`lower_third_html`, `end_card_html`. Common params: `color`, `accent`, `font`, `size`,
`emphasis` [words], `uppercase`; plus `step`/`total`, `number`/`prefix`/`suffix`/`decimals`,
`arrow`, `emoji`, `placeholders`.

### Business cards

Text templates with their own background, images and motion, for company videos and demos.
They are drawn above other layers and annotations, under captions. Put a plain clip under a
full-frame card: an asset with `path: "color:brand"` (the brand background) or
`"color:#101014"` is a still of that color.

```json5
assets: {plain: {path: "color:brand"}, acme: {path: "logos/acme.png"}},
main: [{id: "title", clip: "plain", duration: 3}, …],
layers: [
  {type: "text", template: "title_card", text: "Glance", subtitle: "Every metric on one screen", anchor: "title"},
  {type: "text", template: "feature_list", text: "New in 4.2", start: "cue:new", end: "cue:next",
   params: {items: ["Shared dashboards", "One-click export"], reveal: ["cue:dash", "cue:export"]}},
]
```

| Template | `text` / `subtitle` | `params` |
|---|---|---|
| `title_card` | product name / one-line promise | `logo: false` hides the brand logo |
| `section_card` | section title / detail | `number` (shown as `02`). Each section card starts a chapter in `cutline chapters` |
| `feature_list` | heading | `items` (1–5 strings), `reveal` (one time per item: seconds on the edit or `"cue:…"`; default every 0.5 s) |
| `stat` | — / label | `number` (counts up), `prefix`, `suffix`, `decimals`, `source` (small line); takes `position` |
| `testimonial` | the quote | `name`, `title`, `company`, `photo` and `logo` (image asset ids; the photo is cropped to a circle) |
| `compare` | heading | `left: {title, items}`, `right: {title, items}` (1–6 items each; the right column is highlighted, with ticks) |
| `logo_wall` | heading | `logos` (1–12 image asset ids), `monochrome` (white silhouettes for logos with transparency) |
| `cta_link` | headline / detail | `url` (shown in a pill), `qr: false` hides the QR code of the URL (`https://` is added when the URL has no scheme) |

All take `background`: `"brand"` (default; a soft gradient of the brand background), `"scrim"`
(dark, translucent, over footage; the default for `feature_list` and `stat`), `"none"` or a
`"#color"`; and `color` / `accent` to override the brand text and accent colors. Titles use the
brand `title` font (bold when the font has a weight axis, as Inter does), the rest the `body` font.
Text is wrapped and shrunk to stay inside the safe zone on every target.

## Captions

| Field | Default |
|---|---|
| `source` | `auto` (transcripts mapped through the edit) · `file` (SRT/transcript in timeline time) · `none` |
| `file` | — |
| `style` | brand default (`bold_pop`); also `karaoke`, `word_by_word`, `clean_sentence`, `boxed` |
| `position` | `lower_mid` (vertical) / `bottom` |
| `max_words_per_line`, `max_chars_per_line`, `max_lines` | per style (bold_pop 3 words / 22 chars) |
| `corrections` | `{}` (plus the brand `dictionary`) |
| `emphasis` | `[]` — accent color |
| `emoji` | `{}` — word → emoji name |
| `font`, `size`, `color`, `accent_color`, `uppercase` | brand / style defaults |
| `assets` | clip assets + voice layers |
| `lang` | auto |

## Audio

`loudness` `{integrated: -14, true_peak: -1.0, lra}`, `voice_chain` (`clean_voice`),
`dialogue_chain` (= voice_chain), `duck` `{under: [voice, dialogue], target: [music],
amount_db: -12, attack: 0.12, release: 0.45, mode: "envelope"|"sidechain"}`, `micro_fade` 0.01.
`music_level` (LU, 3–40): the music stem's gain is measured and set so the ducked music sits
this far below the voice (`render_report.json` → `loudness.music_gain_db`); 18 is a typical
narration bed.

## Auto directives

| Directive | Fields (defaults) |
|---|---|
| `jump_cuts` | `enabled` true, `min_silence` 0.35, `keep_padding` 0.08, `applies_to` (all clip assets), `noise_db` −35 |
| `remove_fillers` | `enabled` true, `words` [um, uh, erm, uhm, hmm] |
| `punch_ins` | `every` 4.0, `scale` [1.08, 1.15], `skip_if_other_motion` true, `duration` 0.25 |
| `sfx_on_transitions` | SFX asset/alias for transitions without one |
| `snap_to_beats` | `enabled` true, `asset` (default: the only music-role audio layer), `max_shift` 0.15 s, `downbeat_reach` 0.35 s. Each cut moves onto the music (in order, so later cuts see the ripple) by fitting the outgoing clip with `end_at`: onto the first beat of a bar when one is within `downbeat_reach` and the bars are clear, otherwise onto the nearest beat within `max_shift`. Cuts farther away, clips with a `speed_ramp`, and moves that would run a clip out of footage stay put. `cutline expand` lists every move; `cutline beats` shows tempo, bars and beats on the timeline |

| `fast_forward_idle` | `enabled` true, `min_idle` 2.5 s, `speed` 4 (1.5–8), `keep` 0.4 s, `activity` 0.006, `badge` true, `applies_to` (all video clip assets). Stretches of a recording where almost nothing changes on screen (loading, long typing, a spinner) and nobody speaks play at `speed`, with a small "4×" `badge` layer; `keep` seconds at each end stay at normal speed. The clip is split into pieces (`rec`, `rec~2`, …); layers anchored to it stay on the same moment of the footage. Clips with `end_at`, a `speed` or a `speed_ramp` are left alone |
| `zoom_on_clicks` | `enabled` true, `max_scale` 2, `merge_within` 2.5 s, `hold` 1.2 s, `duration` 0.6 s, `steps` (all). Browser recordings: zooms in on each clicked or typed-in element as the pointer sets off, pans to the next one when it follows closely (one scale for the run, so it is a pan and not a pump), and zooms out `hold` seconds after the last. Clicks close in time and place share one framing. Clips that already have a zoom are left alone |
| `cursor` | `enabled` true, `size` 1.4, `click_ripple` true, `keys` true. Browser recordings: adds a `cursor` layer per clip with the recorded pointer path and clicks, and a `keys` badge for shortcuts and Enter / Escape / Tab |

Generated items carry `generated_by`; `cutline expand` shows them. Jump-cut pieces keep the first
piece's id (`talk`, `talk~2`, …).

## Cues

A cue names a phrase in a voice track; cutline finds it in the track's transcript when it loads
the project and replaces every `"cue:name"` reference with timeline seconds (so `validate`,
`expand` and `render` see plain numbers). `cutline cues project.json5` lists where each landed.

```json5
cues: {
  clocks: "World clocks",                                   // short form: the phrase
  cal: {phrase: "calendar jumps", edge: "end"},              // when the last word ends
  again: {phrase: "world clocks", occurrence: 2, asset: "vo"},
},
main: [
  {id: "intro", clip: "title", end_at: "cue:clocks"},       // cut to the next clip on "World"
  {transition: {type: "fade", duration: 0.4}},
  {id: "time", clip: "screen", in: 3, end_at: "cue:cal"},   // plays from 3 s at 1×, cut on "cal"
],
layers: [
  {type: "audio", asset: "vo", start: 1.2},
  {type: "text", template: "lower_third", text: "World clocks", start: "cue:clocks+0.3", end: "cue:cal-0.5"},
],
```

| Field | Default | Notes |
|---|---|---|
| `phrase` | required | case and punctuation are ignored; close transcription slips match ("pains" for "panes") with a `W_CUE_FUZZY` warning |
| `asset` | the only voice-role audio layer's asset | its transcript: the asset `transcript` file, the transcript cache (`cutline transcribe`), or whisper |
| `occurrence` | 1 | which occurrence of the phrase |
| `edge` | `start` | `start` (first word begins) or `end` (last word ends) |

The asset must be played by exactly one `audio` layer, with a numeric `start` (default 0) or
pinned to a clip with `anchor` + `offset`; a cue is the layer's timeline start + the word time −
the layer's `src_in`. With an anchored voice the cues are resolved, the timeline is built, and the
anchor clip's real start is fed back until it settles; anchoring the voice to a clip whose own
position depends on those cues is `E_CUE_CYCLE`. Errors: `E_UNKNOWN_CUE`,
`E_CUE_NOT_FOUND` (the hint lists the closest phrases), `E_CUE_NOT_ALLOWED` (a cue in a source
time such as `in`), `E_CUE_ASSET`, `E_CUE_LAYER_TIME`, `E_CUE_TRANSCRIPT`, `E_CUE_CYCLE`; for `end_at`:
`E_END_AT_BEFORE_START`, `E_FIELD_CONFLICT`, `W_FIT_HOLD` (footage ends before the cut), and with
`fit: "speed"` `E_FIT_SPEED`.

## Export

One edit, several deliverables. With several exports, output files are named
`<name>-<export>.<ext>`.

```json5
exports: [
  {name: "youtube"},                                                   // the project target
  {name: "linkedin", target: "portrait_4x5"},
  {name: "site", audio: false, loop: true, max_bytes: "8MB", range: ["cue:export", "cue:done"]},
  {name: "web", format: "webm", captions: "sidecar"},
]
```

| Field | Default | Notes |
|---|---|---|
| `name` | target name | used in the file name |
| `target`, `fps`, `width`, `height`, `chroma` | the project's | per-export geometry; the edit re-fits (frames, reframe, text) |
| `format` | `mp4` | `mp4` (H.264 + AAC) or `webm` (VP9 + Opus) |
| `crf` | per target | quality (x264 scale for MP4, VP9 scale for WebM); ignored with `max_bytes` |
| `audio` | `true` | `false`: no audio track, for silent autoplay. QA's `muted` check then warns where narration has no caption or text on screen |
| `loop` | `false` | seamless loop: the end dissolves into the start and the file is 0.5 s shorter. Needs `audio: false`; QA's `loop_seam` compares the last frame with the first |
| `max_bytes` | — | size cap: `"8MB"`, `"900KB"` or bytes. The video bitrate is solved for it (and lowered and encoded again if the first pass lands over). `W_MAX_BYTES_TIGHT` when the cap leaves too little; QA's `file_size` fails if it is missed |
| `range` | whole edit | `[from, to]` on the edit (seconds or cues): export only that part |
| `captions` | `burn` | `burn` (in the picture), `sidecar` (files only) or `none` |
| `poster` | 0.5 s | time on the edit of the poster frame |
| `out` | `out/<name>.<ext>` | output path |

Every export also writes `<name>.srt`, `<name>.vtt` (WebVTT for web players), `<name>.poster.jpg`,
`<name>.qa.json` and `<name>.render_report.json`. A part of the edit (`range`, `loop`) is not
re-normalized, so its loudness is reported but not checked.

## Variants and locales

Several videos from one project. Both are applied before anything else reads the project, so
cues, steps and timing see an ordinary project; because cuts are timed by cues and `end_at`, a
shorter script or another language re-times the edit by itself.

```json5
variants: {
  full:   {},
  teaser: {exclude: ["deep_dive"], max_duration: 30, exports: ["linkedin"],
           assets: {vo: {path: "vo.teaser.mp3", script: "vo.teaser.txt"}}},
},
locales: {
  de: {assets: {vo: {path: "vo.de.mp3", script: "vo.de.txt"}},
       cues: {export: "die Export-Schaltfläche"}, strings: "strings.de.json5", lang: "de"},
},
strings: {title: "World clocks", cta: "Try it free"},
main: [
  {id: "title", clip: "plain", end_at: "cue:export"},
  {id: "deep_dive", clip: "screen", only: ["full"]},
],
layers: [{type: "text", template: "title_card", text: "@title", anchor: "title"}],
```

```
cutline render project.json5                      # every variant, base language
cutline render project.json5 --variant teaser --locale de
cutline validate project.json5 --locale de        # the flags work on every command
```

**Variants** (a cut of the edit):

| Field | Notes |
|---|---|
| `include` / `exclude` | clip ids to keep / leave out (default: all clips) |
| `only` / `except` on clips and layers | the variants an item belongs to; a layer anchored to a dropped clip goes with it, and a transition survives only between two kept neighbours |
| `assets` | asset fields to replace, e.g. a shorter voice-over and its script |
| `exports` | export names to render for this variant |
| `max_duration` | `W_VARIANT_TOO_LONG` when the edit runs longer |

Without `--variant`, `render` renders every variant and the other commands use the first by
name. Outputs are named `<name>-<variant>…`.

**Locales** (a language):

| Field | Notes |
|---|---|
| `assets` | asset fields to replace: the voice-over, its `script` / `transcript`, images |
| `cues` | the cue phrases in this language, by cue name |
| `strings` | a JSON5 file (or an object) of `key: text`. Text written as `"@key"` (layer `text`, `subtitle`, `label`, `keys`, anything inside `params`, clip `chapter`, frame `url` / `title`, marker titles) takes the locale's string, else the project's `strings`. `"@@x"` is a literal `@x`; a missing key is `E_UNKNOWN_STRING` |
| `lang` | language code for `captions.lang` |

With `--locale`, browser recordings are made in that language too (the script's `locale` is
replaced and each language is cached separately), so the interface on screen matches the
narration. Outputs are named `<name>…-<locale>`. `W_FIT_HOLD` shows where a longer narration
runs out of footage.

## Brand file

```json
{
  "fonts": {"title": "Anton", "body": "Inter", "caption": "Poppins ExtraBold"},
  "colors": {"primary": "#FFFFFF", "accent": "#FFC400", "text": "#FFFFFF", "stroke": "#000000", "background": "#101014"},
  "logo": null, "caption_style": "bold_pop", "lut": null, "watermark_position": "top_right",
  "sfx": {"whoosh": "../sfx/whoosh.wav", "pop": null, "ding": null, "riser": null},
  "dictionary": {"chefs knife": "chef's knife"}
}
```

| Field | Default | Notes |
|---|---|---|
| `logo` | — | PNG (with transparency), JPEG or WebP, relative to the brand file. Drawn as a watermark over the edit at `watermark_position` inside the safe zone; also used by `title_card` |
| `watermark` | `{opacity: 0.7, scale: 0.08}` | `scale` is the logo width as a fraction of the frame width; `from` / `to` (seconds on the edit) limit when it shows |
| `watermark_position` | `top_right` | any position preset |
| `intro`, `outro` | — | video, image or HTML file played before / after `main`. They appear in `cutline expand` as clips `brand_intro` / `brand_outro` |
| `bumper_fade` | 0.4 | seconds of fade between a bumper and the edit; 0 cuts |

**Times with an intro.** Every timeline time written in the project (layer `start`/`end`, sfx
`at`, clip `end_at`, markers, cues, `bar:`/`beat:`) counts from the start of the *edit*, so adding
or removing an intro never shifts your edit. `cutline expand`, `review` and `preview --at` show
output time (intro included). The watermark, captions, auto jump cuts and beat snapping leave
the bumpers alone. Layers cannot extend over the intro or outro.

`cutline init --style business` scaffolds `brand/default.json` from the bundled `business`
brand (Inter, blue accent, `clean_sentence` captions).

## Migration notes

- v1 is the first schema.
- An export without `target` now uses the project's target (it used to be `shorts`). Breaking changes will bump `version` and be listed here.
- `end_at` with both `in` and `out` used to change the clip's speed to fit. It now trims (or
  holds the last frame); add `fit: "speed"` for the old behaviour.
- Transitions ease by default; add `ease: "linear"` for the old constant rate.
