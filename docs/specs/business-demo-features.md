# Spec (tentative): business and product-demo features

Status: **approved 2026-11-14**, being built on branch `business-demo-features`.

## 1. Why

cutline is strong at creator-style video (Shorts hooks, `bold_pop` captions, punch-ins, beat
cuts) and already has a good base for demos: VHS terminal recording, `profile: "screencast"`,
`zoom_to`, masks, voice-over cues and `end_at`.

A business or product-demo video is judged on different things:

| A demo viewer needs | cutline today |
|---|---|
| To see where to look (pointer, click, the one field that matters) | `zoom_to` and `callout` text only. No highlight, spotlight, arrow, cursor or key badge |
| A polished frame around the screen (window chrome, background, padding) | Raw full-bleed recording |
| Web-app footage that can be re-recorded when the UI changes | Terminal only (VHS). Browser footage is recorded by hand outside cutline |
| Consistent company branding | The brand file has `logo`, `intro`, `outro` and `watermark_position`, but no render code reads them |
| Business graphics (title card, feature list, link CTA) | Templates lean to YouTube: `cta_subscribe`, `emoji_pop`, `hook_title` |
| Files for a website, LinkedIn, email, docs | MP4 only; `shorts`, `long_*`, `square`, `custom` |
| A 90 s demo, a 30 s teaser and a German version from one edit | One edit per project |

## 2. Features

Sizes: **S** about a day, **M** a few days, **L** a week or more. All new fields are additive to
schema v1 (no version bump).

### Phase 1 — polish with no new dependencies

#### F1. Annotation layers (M)

New layer types, all taking the standard timing (`start`/`end`/`anchor`/`cue:`) and a `rect` in
frame fractions like `mask` and `zoom_to`:

```json5
{type: "highlight", rect: {x, y, w, h}, style: "outline",   // outline | fill | underline
 color: "brand", width: 4, radius: 8, enter: "draw", start: "cue:export"},
{type: "spotlight", rect: {x, y, w, h}, dim: 0.6, feather: 12, radius: 8},
{type: "arrow", from: {x, y}, to: {x, y}, label: "New in 4.2", color: "brand"},
{type: "keys", keys: ["cmd", "K"], position: "bottom"},     // keycap badges
{type: "cursor", path: [{t, x, y, click: true}], size: 1.5, click_ripple: true},
```

- `spotlight` dims everything outside the rect; several at once share one dim layer.
- `cursor` draws a synthetic pointer along eased keys with a ripple on clicks. F6 fills `path`
  from a recording; it can also be written by hand over footage whose own cursor is hidden.
- Annotations follow a clip's zoom: when the clip under an anchored annotation has a `zoom_to`
  or `punch_in`, the rect is transformed with it, so one rect serves both.
- Review: a new `review/annotations/*.jpg` set with each rect outlined on its frame.
- Acceptance: golden frames per type on both targets; a transformed rect stays within 2 px of
  the zoomed content; `E_RECT_OUT_OF_FRAME` for bad rects.

#### F2. Screen frames and backgrounds (M)

A clip-level `frame` puts the recording inside a window on a background:

```json5
{id: "dash", clip: "screen", frame: {
  style: "browser",            // browser | window | phone | tablet | none
  url: "app.example.com",      // shown in the browser bar
  background: "brand",         // brand | "#101014" | {gradient: ["#1b1f3b", "#0b0d1a"]} | asset id
  padding: 0.06, radius: 14, shadow: 0.5, theme: "dark",
}}
```

- Chrome is drawn by cutline (generated, like PiP borders and shadows today), so no third-party
  device artwork to license. `phone`/`tablet` are plain rounded bezels, not named devices.
- `zoom_to` rects stay relative to the recording, not the framed canvas.
- A project-level `default_frame` applies to every clip of `profile: "screencast"` projects.
- Vertical targets: a 16:9 recording in a frame is placed in the upper part with captions
  below, replacing `fit_blur` as the better default for demos in `shorts`.
- Acceptance: golden frames per style and theme; text sharpness after framing is no worse than
  a plain `fit_pad` at the same scale (edge-contrast measure from the existing zoom tests).

#### F3. Brand kit applied, plus a business brand preset (S–M)

- Render what the brand file already declares: `logo` as a watermark at `watermark_position`
  (`watermark: {opacity, scale, from, to}`), `intro` and `outro` assets inserted around `main`
  with a default fade (`brand_bumpers: false` on a project opts out).
- `cutline init --style business` scaffolds a brand with `clean_sentence` captions, no emoji,
  Inter throughout, restrained transitions, and `long_1080p` + `screencast`.
- A `business` section in the guide: no punch-in cadence, cuts not dissolves, captions on by
  default (most business video is watched muted).
- Acceptance: logo golden frames in each corner inside the safe zone; bumpers shift cues and
  anchors correctly (extends the existing cue tests).

#### F4. Business templates (M)

New text templates, each with an HTML variant where motion matters:

| Template | Use |
|---|---|
| `title_card` | Product name, one-line promise, logo |
| `section_card` | Chapter divider ("2 · Invite your team"); also sets a chapter marker |
| `feature_list` | Up to 5 bullets that build one by one; `reveal: ["cue:a", "cue:b"]` |
| `stat` | Number with label and source line (extends `big_number`) |
| `testimonial` | Quote, name, title, company logo, optional photo |
| `compare` | Before / after or us / them, two columns |
| `logo_wall` | Customer logos in a grid, monochrome option |
| `cta_link` | Headline, URL, optional QR code generated from the URL |

- Acceptance: golden frames per template per target (as for existing templates), auto-fit and
  safe-zone tests, review's "text over detailed content" check covers them.

#### F5. Delivery for business channels (M)

```json5
exports: [
  {name: "site",     target: "long_1080p", format: "mp4", audio: false, loop: true, max_bytes: "8MB"},
  {name: "linkedin", target: "portrait_4x5", captions: "burn"},
  {name: "docs",     target: "custom", width: 960, height: 540, range: ["cue:export", "cue:done"]},
  {name: "web",      format: "webm"},
]
```

- New target `portrait_4x5` (1080×1350). New export fields: `format` (`mp4` · `webm`), `audio`, `loop` (checks first and last frame match, or adds a crossfade), `max_bytes`
  (bitrate solved for the size, QA error if missed), `range` (a cut-out of the timeline),
  `captions` (`burn` · `sidecar` · `none`), plus a `.vtt` next to the `.srt` and a poster JPEG.
- QA: a `muted` check for exports marked for autoplay — captions present and no section
  longer than N s where only the voice carries meaning (no caption on screen).
- Acceptance: each format probes correctly; `max_bytes` met within 3 %.

### Phase 2 — recording web apps

#### F6. Browser recordings as assets (L)

The web equivalent of `.tape`: an asset whose path is a `.demo.json5` script, recorded with the
Chrome that cutline already installs, cached by script content, re-recorded by `cutline record`.

```json5
{viewport: {w: 1600, h: 1000, scale: 2}, url: "http://localhost:3000", theme: "light",
 steps: [
   {goto: "/dashboard"}, {wait: "text=Revenue"},
   {click: "button:has-text('Export')", name: "export"},
   {type: {into: "#email", text: "ada@example.com", delay: 60}},
   {press: "Enter"}, {wait: 1.2}, {scroll: {to: "#invoices", duration: 0.8}},
 ]}
```

- Output: the video plus an **events sidecar** (cursor path, clicks, key presses, and the
  on-screen rect of every element a step touched, each with a time and the step's `name`).
- The real cursor is not captured; F1's `cursor` layer draws it from the sidecar, so pointer
  motion is smooth and its size is a setting.
- Named steps become usable in the edit: `zoom_to: {step: "export"}`,
  `{type: "highlight", step: "export"}`, `in: "step:export-0.5"`. No hand-measured rects.
- Unlike HTML graphics, the page needs the network. Recording is restricted to the origins
  listed in the script (`allow: [...]`); a login step reads credentials from environment
  variables and never writes them to the cache or sidecar.
- Acceptance: a fixture web app records identically twice (frame hashes at settled points);
  step rects within 2 px of the DOM rect; `E_DEMO_STEP_FAILED` names the step and selector.

Open design question: real-time capture (CDP screencast; simple, but frame timing varies) or
stepped capture as the HTML renderer does today (exact, but real apps with timers and network
do not always tolerate a virtual clock). I would start with real-time capture resampled to CFR
and keep stepped capture for local, static pages.

#### F7. Auto directives for demos (M, needs F6 for the first two)

| Directive | Effect |
|---|---|
| `zoom_on_clicks` | A `zoom_to` on each named click's element, held until the next action, merged when clicks are close together |
| `cursor` | Adds the `cursor` layer with click ripples and optional `keys` badges for shortcuts |
| `fast_forward_idle` | Stretches with no screen change and no narration (loading, long typing) play at 4–8× with a small "4×" badge. Works on any recording via frame differences |

All expand to explicit items shown by `cutline expand`, like today's auto directives.

### Phase 3 — scale one edit to many videos

#### F8. Variants (M)

```json5
variants: {
  teaser: {include: ["title", "export", "cta"], max_duration: 30, exports: ["linkedin"]},
  full:   {},
}
```

Clips and layers take `only: ["full"]` / `except: ["teaser"]`. `cutline render --variant teaser`.
Each variant can have its own VO asset, so cues re-time the cut to the shorter script.

#### F9. Locales (M)

```json5
locales: {
  de: {assets: {vo: {path: "vo.de.mp3", script: "vo.de.txt"}}, strings: "strings.de.json5"},
}
```

Text layers reference `"@key"` strings; cue phrases are given per locale. Because the edit is
timed by cues and `end_at`, the German cut follows the German narration without re-editing.
`W_FIT_HOLD` already reports where a longer narration runs out of footage. F6 scripts take a
`locale` so the UI itself can be re-recorded in that language.

#### F10. Presenter bubble (S–M)

A `presenter` layer: webcam asset in a circle or rounded rect, face-centered with the existing
YuNet tracker, with named layouts switchable over time (`bubble`, `side_by_side`,
`presenter_full`, `screen_full`) and audio sync offset to the screen recording.

#### F11. Demo QA (M)

- **Legibility:** estimate UI text height in the delivered frame; warn when it falls below a
  readable size for the target (the usual failure of 16:9 recordings in vertical video).
- **Private details:** optional OCR pass over the contact sheet frames flags emails, tokens,
  home paths and hostnames that no `mask` covers. Needs an OCR dependency (to be chosen and
  licence-checked), so it is off unless installed via `cutline deps`.
- **Pacing:** warn on stretches over N s with no narration, no screen change and no annotation.

## 3. Suggested order

1. F3, F1, F2 — the most visible lift per day of work; every later feature builds on F1.
2. F4, F5.
3. F6, then F7.
4. F8, F9, then F10, F11.

Each feature lands with its docs (`PROJECT_FORMAT.md`, `CLAUDE_GUIDE.md`), schema, golden tests
and a `selftest --full` case, as the existing milestones did.

## 4. Out of scope

Recording native desktop or mobile apps, generating voice or media (stays with other tools, per
the original non-goals), a GUI, uploading to any platform, interactive (clickable) demos.

## 5. Decisions (2026-11-14)

1. Build all eleven features, in the order of section 3.
2. F6 captures in real time, resampled to a constant frame rate.
3. F2 frames are generated by cutline only (no user-supplied frame images).
4. F11 OCR is an optional dependency with a setting to choose the engine: macOS Vision by
   default on macOS, Tesseract elsewhere.
5. F5 delivers MP4 and WebM (no GIF or WebP).
