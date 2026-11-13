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

## Top level

| Field | Type | Default | Notes |
|---|---|---|---|
| `version` | `1` | required | |
| `name` | string | file stem | output file name |
| `target` | `shorts` · `long_1080p` · `long_4k` · `square` · `custom` | `shorts` | geometry, fps, encoder, safe zone |
| `fps` | number or `"30000/1001"` | target (long-form follows the first video clip) | |
| `width`, `height` | even int | — | for `custom` |
| `brand` | path or `"default"` | `brand/default.json` if present, else bundled | |
| `assets` | map id → Asset | `{}` | |
| `main` | [Clip or `{transition}`] | `[]` | the storyline |
| `layers` | [Layer] | `[]` | overlays, text, audio |
| `captions` | Captions | none | |
| `audio` | AudioSettings | defaults below | |
| `auto` | AutoDirectives | none | expanded by `cutline expand` |
| `default_reframe` | Reframe | `{mode: "cover"}` | geometry when source aspect ≠ target |
| `safe_zone` | `{top, bottom, left, right}` fractions | per target | |
| `markers` | [`{at, title}`] | `[]` | chapters |
| `exports` | [Export] | one export of `target` | multi-target from one edit |
| `cues` | map name → Cue | `{}` | phrases in the voice-over; see [Cues](#cues) |

### Targets

| Target | Size | fps | Safe zone (t/b/l/r) | Limit |
|---|---|---|---|---|
| `shorts` | 1080×1920 | 30 | 10 % / 22 % / 6 % / 16 % | ≤ 180 s |
| `long_1080p` | 1920×1080 | follows source (24/25/30/50/60) | 5 % / 8 % / 5 % / 5 % | |
| `long_4k` | 3840×2160 | follows source | same | |
| `square` | 1080×1080 | 30 | 6 % / 10 % / 6 % / 6 % | |
| `custom` | `width`×`height` | follows source | by orientation | |

## Asset

| Field | Default | Notes |
|---|---|---|
| `path` | required | image sequences use a printf pattern: `frames/%05d.png` |
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

## Clip (`main` item)

| Field | Default | Notes |
|---|---|---|
| `id` | required, unique | anchors, chapters, review labels |
| `clip` | required | asset id |
| `in`, `out` | 0, end of asset | source times |
| `duration` | — | stills; or `in` + duration·speed |
| `end_at` | — | timeline time of the cut to the next clip (the middle of the outgoing transition), usually `"cue:…"`. The clip is fitted: with `in` and `out` its speed is computed (0.25–4); without `out` the out point is (at `speed`); stills get the duration. Not with `duration` or `speed_ramp`. Jump cuts and filler removal skip fitted clips |
| `speed` | 1 | 0.25–4; audio pitch preserved (`atempo`) |
| `speed_ramp` | — | `[{t, speed}]`, source-relative `t` |
| `reverse` | false | ≤ 10 s clips |
| `reframe` | `default_reframe` | see Reframe |
| `color` | — | see Color |
| `effects` | `[]` | see Effects |
| `audio` | `keep` | `keep`, `mute`, `duck` (−12 dB) or an asset id replacing it |
| `gain_db` | 0 | |
| `chapter` | — | chapter title starting at this clip |

### Transition (`{transition: {...}}` between two clips)

| Field | Default | Notes |
|---|---|---|
| `type` | required | any xfade name (`fade`, `wipeleft`, `slideup`, `circleopen`, `zoomin`, `pixelize`, …) or `whip_left/right/up/down`, `zoom_punch`, `flash_cut`, `glitch`, `cut` |
| `duration` | required | seconds (≤ either neighbour − 1 frame); the timeline shrinks by it |
| `sfx`, `sfx_gain_db` | —, −3 | asset id or brand alias, centered on the transition |
| `audio` | `crossfade` | or `cut` (hard cut at the midpoint with micro-fades) |
| `curve` | `tri` | acrossfade curve |
| `color` | white | `flash_cut` color |

### Reframe

`mode`: `cover` (default) · `center` · `fit_blur` · `fit_pad` · `track_face` · `manual` · `split`.
`keys` (manual): `[{t, x, y, zoom}]` (clip-relative `t`, normalized center, zoom ≥ 1).
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
| `ken_burns` | `from_scale` 1, `to_scale` 1.15, `from`/`to` {x,y}, `ease` |
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
| `audio` | `asset`, `src_in`, `gain_db`, `fade_in`, `fade_out`, `loop`, `role` |
| `sfx` | `asset` (or brand alias), `at` or `anchor`+`offset`, `gain_db`, `pick_from` |

Text templates: `hook_title`, `lower_third`, `callout`, `step_counter`, `big_number`, `quote`,
`cta_subscribe`, `end_card`, `emoji_pop`, and HTML variants `hook_title_html`,
`lower_third_html`, `end_card_html`. Common params: `color`, `accent`, `font`, `size`,
`emphasis` [words], `uppercase`; plus `step`/`total`, `number`/`prefix`/`suffix`/`decimals`,
`arrow`, `emoji`, `placeholders`.

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

## Auto directives

| Directive | Fields (defaults) |
|---|---|
| `jump_cuts` | `enabled` true, `min_silence` 0.35, `keep_padding` 0.08, `applies_to` (all clip assets), `noise_db` −35 |
| `remove_fillers` | `enabled` true, `words` [um, uh, erm, uhm, hmm] |
| `punch_ins` | `every` 4.0, `scale` [1.08, 1.15], `skip_if_other_motion` true, `duration` 0.25 |
| `sfx_on_transitions` | SFX asset/alias for transitions without one |

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
  {id: "time", clip: "screen", in: 3, out: 18, end_at: "cue:cal"},  // speed computed to fit
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

The asset must be played by exactly one `audio` layer with a numeric `start` (default 0); a cue
is that start + the word time − the layer's `src_in`. Errors: `E_UNKNOWN_CUE`,
`E_CUE_NOT_FOUND` (the hint lists the closest phrases), `E_CUE_NOT_ALLOWED` (a cue in a source
time such as `in`), `E_CUE_ASSET`, `E_CUE_LAYER_TIME`, `E_CUE_TRANSCRIPT`; for `end_at`:
`E_END_AT_BEFORE_START`, `E_FIT_SPEED`, `E_FIELD_CONFLICT`.

## Export

`{name, target, fps, width, height, crf, out}`. With several exports, output files are named
`<name>-<export>.mp4`.

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

## Migration notes

- v1 is the first schema. Breaking changes will bump `version` and be listed here.
