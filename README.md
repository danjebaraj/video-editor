# cutline

A declarative command-line video editor built for Claude to cut YouTube Shorts, short-form and
long-form videos from provided footage, ElevenLabs audio and generated graphics.

Claude writes a JSON5 **project file**; `cutline` validates it (machine-readable errors with
JSON pointers), shows it (`review`: contact sheets, cut pairs, caption frames, audio graph, QA,
suggestions), and renders it with ffmpeg (segment cache, exact frame mapping, two-pass loudness,
post-render QA).

```
cutline doctor                       # tools, filters, fonts, models, Chrome
cutline deps install --all           # ffmpeg (pinned static build), whisper model, face model, Chrome
cutline init my-short --target shorts --assets footage/
cutline validate my-short/project.json5
cutline expand   my-short/project.json5     # explicit timeline (auto jump cuts, fillers, punch-ins)
cutline cues     my-short/project.json5     # where each voice-over cue landed
cutline review   my-short/project.json5     # review/ pack for vision
cutline render   my-short/project.json5     # final render + QA + .srt + credits
cutline guide                               # the workflow Claude should follow
```

## Features

- **Editing:** trims, concat, ripple via anchors, speed and speed ramps, reverse, freeze,
  VFR→CFR, rotation, BT.709 normalization, `cover` / `fit_blur` / `fit_pad` / `track_face` /
  `manual` / `split` geometry.
- **Attention:** punch-ins and zooms with easing (smoothness-tested), Ken Burns, shake, flash,
  blur-in, glitch, vignette, motion blur; every xfade transition plus whips, zoom punch, flash
  cut and glitch; overlays (alpha video, images, PNG sequences, B-roll/PiP with rounded corners,
  border, shadow) with enter/exit animations; progress bar; LUTs and color presets.
- **Screencasts:** `profile: "screencast"` (text-tuned encoding, freeze-tolerant QA), optional
  4:4:4 chroma, and `zoom_to` a rectangle of the UI.
- **Masks:** blur, pixelate or fill a region (with optional replacement text) to hide private
  details in screen recordings.
- **Text:** libass-rendered templates (hook title, lower third, callout, step counter, big
  number, quote, CTA, end card, emoji pop) with bundled fonts, safe zones and auto-fit; HTML
  variants via headless Chrome.
- **Captions:** whisper.cpp or ElevenLabs Scribe import, words mapped through the edit; a
  `script` replaces recognised words with the exact text, keeping the audio's timing;
  `bold_pop`, `karaoke`, `word_by_word`, `clean_sentence`, `boxed`; emphasis and emoji; SRT.
- **Script timing:** name phrases in the voice-over (`cues`) and cut, place text and SFX on
  them (`"cue:clocks"`); clips fit themselves to the cut with `end_at`.
- **Edit assist:** automatic jump cuts, filler removal, punch-in cadence, SFX on transitions,
  all expanded to inspectable timeline items.
- **Audio:** role stems, voice chains, transcript/VAD-driven ducking, music level set relative to
  the voice (`music_level`), backtimed music endings (`align: end`), two-pass loudness
  (−14 LUFS / −1 dBTP default), micro-fades, `audio.png`.
- **Reframe:** YuNet face detection (ONNX Runtime, `onnx` feature), tracking with a dead zone,
  spring smoothing, max pan speed, scene-cut resets; manual keys and split layouts.
- **Generated graphics:** HTML / SVG / Lottie rendered frame-accurately with alpha via headless
  Chrome, sandboxed (no network).
- **Quality:** post-render QA (codec, geometry, color tags, duration, A/V, loudness, clipping,
  silence, black, freeze, hook, caption safe zone and reading speed), `selftest --quick/--full`
  on synthetic media, review pack.

## Building

```bash
cargo build --release                    # core (no face detector)
cargo build --release --features onnx    # with YuNet face tracking (downloads ONNX Runtime)
```

Requires Rust ≥ 1.80. External tools are managed by `cutline deps` (pinned in
`tools.lock.json`); a system ffmpeg ≥ 6 works as a fallback. `scripts/release.sh` builds a
universal macOS binary and a Windows zip with the `assets/` directory alongside.

Tools and caches live in the cutline home: `CUTLINE_HOME` if set, else the nearest
`.cutline-home/` with a `tools/` folder above the working directory or the `cutline` binary (so a
repo checkout's `target/release/cutline` finds the repo's tools from anywhere), else the platform
data directory; `cutline doctor` shows which. Environment overrides: `CUTLINE_HOME` (tools + cache), `CUTLINE_ASSETS`, `CUTLINE_FFMPEG` +
`CUTLINE_FFPROBE`, `CUTLINE_CHROME`, `CUTLINE_WHISPER_CLI`, `CUTLINE_WHISPER_MODEL`,
`CUTLINE_FACE_MODEL`, `CUTLINE_LOCKFILE`.

## Development

```bash
cargo xtask ci          # fmt --check, clippy -D warnings, tests, selftest --quick
cargo test --workspace --all-features
CUTLINE_BLESS=1 cargo test ...           # re-bless golden images / schema
cargo run -p cutline-cli -- selftest --full
```

Docs: [`docs/CLAUDE_GUIDE.md`](docs/CLAUDE_GUIDE.md) (workflow),
[`docs/PROJECT_FORMAT.md`](docs/PROJECT_FORMAT.md) (every field),
[`docs/decisions.md`](docs/decisions.md), [`PROGRESS.md`](PROGRESS.md),
[`spec.md`](spec.md) (the original specification, including the licensing review in §12).

## Licensing

cutline itself is MIT OR Apache-2.0 (internal use). Bundled assets: OFL fonts (`assets/fonts`),
Noto Emoji (Apache-2.0), generated LUTs (CC0), lottie-web (MIT). ffmpeg static builds are GPL and
are downloaded, never bundled, and run as separate processes. See spec §12 for the full review,
including the content-rights checklist (ElevenLabs plan, YouTube synthetic-content disclosure).
