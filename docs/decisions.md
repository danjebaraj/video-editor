# Decisions log

Non-trivial design choices not dictated by the spec. Format: date — decision — alternatives — reason.

## 2026-11-12 — Workspace at repo root
- **Decision:** the cargo workspace lives at the repository root (`crates/…`), not under a `cutline/` subfolder.
- **Alternatives:** `cutline/` subdirectory as drawn in §3.1.
- **Reason:** the repo *is* the tool; an extra nesting level adds nothing.

## 2026-11-12 — Linux is a supported dev/test platform
- **Decision:** Linux (x86_64/arm64) is supported for development and CI, using the Martin Riedl Linux static builds or system ffmpeg as a fallback.
- **Alternatives:** macOS/Windows only.
- **Reason:** development happens in Linux containers; the user approved this. macOS stays the primary target.

## 2026-11-12 — ffmpeg sources
- **Decision:** Martin Riedl builds for macOS arm64, macOS x86_64 *and* Linux, all from the release channel with `.sha256` sidecars. BtbN `ffmpeg-nX.Y-latest-win64-gpl` (newest release branch, not master) for Windows. `CUTLINE_FFMPEG_MANIFEST_URL` overrides the source with a JSON manifest (mirrors/tests).
- **Alternatives:** evermeet.cx for macOS Intel (spec default).
- **Reason:** one source with published checksums and the same build config on both Mac architectures; evermeet publishes no machine-readable sha sidecar. evermeet remains a documented manual alternative.

## 2026-11-12 — Retry with backoff on downloads
- **Decision:** every HTTP request is retried 4× with exponential backoff, except sha256 mismatches (fail immediately).
- **Reason:** the Martin Riedl CDN intermittently answers 404 to redirect requests.

## 2026-11-12 — Lockfile location
- **Decision:** `CUTLINE_LOCKFILE` → nearest `tools.lock.json` up from the cwd (the repo pin) → `<tools_dir>/tools.lock.json`. `deps install` uses the pinned URL+sha when the lockfile has a complete entry for the platform; `--latest` ignores the pin.
- **Reason:** the repo pin is authoritative in the repo; installs elsewhere still record what they installed.

## 2026-11-12 — Tool version layout without symlinks
- **Decision:** `<tools>/ffmpeg/<version>/` plus plain-text `current` and `previous` files.
- **Reason:** symlinks need admin rights or developer mode on Windows.

## 2026-11-12 — Frame-ID layout is parametric
- **Decision:** the spec layout (16×64px blocks in the top row) is the default. `FrameIdLayout` can also place an 8×2 grid centered, and maps through cover/fit/scale transforms so the decoder knows where blocks land in an output frame.
- **Reason:** a 16:9→9:16 cover crop removes the top-left blocks; the centered grid survives it, which the exact frame-mapping tests need.

## 2026-11-12 — Fonts
- **Decision:** bundle Anton, Bebas Neue, Poppins (Regular/Bold/ExtraBold, static), Inter and Montserrat (variable) from google/fonts, each with its OFL license file.
- **Reason:** libass uses only the default instance of a variable font, so heavy caption and title styles use the static Poppins ExtraBold, Anton and Bebas Neue.

## 2026-11-12 — `reframe` unifies geometry modes
- **Decision:** one `reframe: {mode}` field carries every geometry mode (`cover`/`center`, `fit_blur`, `fit_pad`, `track_face`, `manual`, `split`). The project-level `default_reframe` defaults to `cover`.
- **Alternatives:** a separate `geometry` field next to `reframe`.
- **Reason:** the spec's example uses `reframe.mode` for both, and one field avoids conflicting settings.

## 2026-11-12 — Effect times are clip-relative timeline seconds
- **Decision:** `effects[].at` counts seconds from the clip's start on the timeline (after speed changes).
- **Reason:** that is what Claude sees in `expand`/review output; source-relative times would shift under speed changes.

## 2026-11-12 — Frame-accurate segment seeking
- **Decision:** snap the in point to the source frame grid, seek a quarter frame early (`-ss` before `-i`), then `setpts=PTS-STARTPTS` and `fps`. Pad and trim to the exact frame count (`tpad`+`trim`); audio is sample-exact (`apad`+`atrim`), with sample counts derived from timeline frame boundaries so they never drift.
- **Reason:** guarantees the exact frame mapping the selftest asserts.

## 2026-11-12 — Assembly timebase
- **Decision:** every segment input is normalized with `settb=AVTB` before concat/xfade.
- **Reason:** xfade refuses inputs whose timebases differ (concat outputs AVTB, mov inputs use the stream timebase).

## 2026-11-12 — Orchestration lives in `cutline-render::pipeline`, delivery in `cutline-qa::deliver`
- **Decision:** load → probe → resolve → extras → plan → execute is a library API so the selftest and the CLI share it. QA and the side files (`.qa.json`, `.render_report.json`, `.srt`, `credits.txt`) are written by `cutline-qa::deliver`.

## 2026-11-12 — Audio is rendered separately and muxed
- **Decision:** segment mezzanines carry dialogue audio (PCM f32). An audio stage builds `mix.wav` (and from M2 a loudness-normalized `final.wav`), which the final encode muxes.
- **Reason:** two-pass loudness needs the complete mix, and the video graph stays independent of audio changes.

## 2026-11-12 — `cut` transitions
- **Decision:** `{transition: {type: "cut"}}` has zero overlap whatever its duration. `audio: "cut"` on a visual transition hard-cuts the audio at the overlap midpoint with 10 ms micro-fades.

## 2026-11-12 — Ducking envelope model
- **Decision:** the gain ramps linearly in dB to `amount_db` over `attack` seconds *ending at* speech onset (pre-roll), holds while speech lasts, and ramps back over `release` seconds after speech ends. Gaps shorter than attack+release stay ducked. The envelope is written as a 1 kHz mono WAV, upsampled by ffmpeg and applied with `amultiply`.
- **Alternatives:** `volume=eval=frame` with a giant expression; sendcmd; `sidechaincompress` (still available as `duck.mode: "sidechain"`).
- **Reason:** deterministic, visualizable in audio.png, scales to 60-minute timelines (≈14 MB envelope), and speech is never masked at onset because transcripts tell us the future.

## 2026-11-12 — Speech activity source
- **Decision:** transcript word timings mapped to the timeline when available (M5); otherwise energy VAD (20 ms frames, adaptive threshold = floor + 15 dB, falling back to a −45 dBFS gate when the floor is loud, 150 ms hangover) over the `under` stems.

## 2026-11-12 — Loudness correction pass
- **Decision:** after `loudnorm` pass 2 (linear), measure with `ebur128`. If integrated is off by > 0.2 LU or true peak exceeds the target, apply a corrective gain plus a 4× oversampled `alimiter` at TP − 0.3 dB.
- **Reason:** loudnorm silently switches to dynamic mode when linear normalization would breach the true-peak limit, so the result can miss the ±0.5 LU acceptance window.

## 2026-11-12 — Stems
- **Decision:** the audio stage renders one WAV per present role (dialogue/voice/music/sfx) with its chain applied, then a separate mix job ducks and sums them. Default chains: voice and dialogue `clean_voice`, music `highpass=30`, sfx raw.
- **Reason:** stems feed the VAD, `audio.png` and future per-role QA without re-decoding.

## 2026-11-12 — Zoom implementation
- **Decision:** zooms (punch-in, zoom, Ken Burns, shake headroom) use one per-frame exact-size `scale` (`eval=frame`, bicubic) to `W+2·round(W(s−1)/2)`, then a fixed-size `crop` whose offset is integral for centered zooms. There is no 2× intermediate.
- **Alternatives:** `zoompan` (integer crop jitter); upscale to 2× and then crop (two resamples, about 4× the pixel work).
- **Reason:** a single resample with even size deltas gives a monotone scale with no center jitter. Measured by `punch_in_is_smooth_and_monotone` (≤ 1.5 % from the ease curve, no reversals, small second differences), which satisfies the spec's "equivalent smooth method" clause.

## 2026-11-12 — Explicit color interpretation
- **Decision:** segment and overlay conversions set `in_color_matrix` and `in_range` from the probed tags. Untagged HD (≥ 720 p) is treated as BT.709 and untagged SD as BT.601; `yuvj`/`pc` means full range. Test readers convert YUV→RGB explicitly as BT.709 limited. Test fixtures are encoded with explicit, tagged BT.709.
- **Reason:** ffmpeg 6.1 and 9.0 resolve `auto` differently for untagged streams.

## 2026-11-12 — Transition goldens per ffmpeg major version
- **Decision:** `tests/golden/transitions/ffmpeg<major>/`. The pinned version (9) is what CI uses; 6 is kept for the system-ffmpeg dev path. Without goldens for the running major version, the comparison is skipped with a message.
- **Reason:** xfade's progress math differs between major versions (the midpoint frame shows a slightly different progress), so a single golden set cannot pass on both versions at SSIM 0.97.

## 2026-11-12 — Overlay and text stacking
- **Decision:** visual layers stack by `z` (default: order in `layers`). ASS text and captions are always composited last, on top.
- **Reason:** captions and titles must never be hidden by B-roll; a single libass pass is also cheaper than interleaving.

## 2026-11-12 — Speed-ramp audio
- **Decision:** video follows the exact closed-form ramp (`setpts` with log integrals). Audio uses the ramp's average tempo (`atempo`), so it can drift inside the clip but always ends on the cut.
- **Known limitation:** accepted for v1 because speed-ramped clips are usually music- or SFX-backed.

## 2026-11-12 — Bundled LUTs are generated
- **Decision:** five 17³ `.cube` LUTs are generated by `scripts/gen_luts.py` (CC0, original math).

## 2026-11-12 — Text measurement matches libass
- **Decision:** layout measures with `ab_glyph`. The ASS font size is converted to an `ab_glyph` pixel scale using each font's ratio of hhea height to OS/2 (winAscent + winDescent). Line height equals the font size.
- **Reason:** libass sizes fonts by the OS/2 win metrics (VSFilter compatibility). Using hhea over-predicted width by about 18 %. Measured agreement with rendered pixels is now within 8 % (`autofit_*` test).

## 2026-11-12 — Fontconfig isolation for text rendering
- **Decision:** every final encode that burns in ASS gets `FONTCONFIG_FILE` pointing to a generated config that lists only the bundled fonts directory.
- **Reason:** identical output whether or not system fonts exist, and never a silent fallback to a system font (tested).

## 2026-11-12 — Fonts and emoji
- **Decision:** Montserrat (variable) was removed: libass renders its default (Thin) instance. Bundled: Anton, Bebas Neue, Inter (variable, default Regular), Poppins Regular/Bold/ExtraBold. A 32-emoji Noto subset (Apache-2.0) is resized to 160 px and fetched from fonts.gstatic.com; the GitHub png tree no longer serves them.

## 2026-11-12 — Template images (emoji_pop) are overlays, not ASS
- **Decision:** templates can emit image overlays (positioned in full-target pixels) in addition to ASS events. `emoji_pop` uses that path with a pop animation.
- **Reason:** libass cannot render color emoji bitmaps.

## 2026-11-12 — Text boxes in QA
- **Decision:** templates report their layout boxes. QA checks every text/caption box against the safe zone; reading speed (cps) applies only to captions.

## 2026-11-12 — whisper.cpp via sidecar only (whisper-rs not compiled)
- **Decision:** transcription runs the whisper.cpp CLI (`whisper-cli`, discovered via `CUTLINE_WHISPER_CLI`, `<tools>/whisper/`, or PATH), with `-ml 1 -sow -ojf` word segments and `-dtw <preset>` alignment for known models. The `whisper-rs` crate is not a dependency.
- **Alternatives:** an in-process `whisper-rs` binding behind a cargo feature (the spec's first choice).
- **Reason:** building whisper.cpp (needed by both paths) was not permitted in the implementation environment, so a feature-gated binding could not be compiled or tested. The spec names the sidecar as the fallback. macOS CI installs `whisper-cpp` via Homebrew and runs the WER test.

## 2026-11-12 — Transcript cache is global
- **Decision:** transcripts are cached under the global cache dir by (asset fingerprint, model, language). `cutline transcribe --import-scribe` stores the import under the default model key, so renders find it without a `transcript:` field.

## 2026-11-12 — Captions are not split at cuts
- **Decision:** caption chunks break at word/char limits, sentence ends and pauses only. Display spans snap to a cut when they would cross it by less than two frames.
- **Reason:** jump cuts fall between words constantly; splitting there shredded sentences into one- or two-word captions.

## 2026-11-12 — Auto-directive expansion
- **Decision:** `jump_cuts` intersects silencedetect spans with transcript gaps (silencedetect alone when there is no transcript), keeps `keep_padding` inside each silence, and splits the clip. The first piece keeps the original id (so anchors survive); later pieces are `<id>~N`. Every generated item carries `generated_by`. Fillers are cut between neighbouring words with 30 ms margins. Punch-ins are placed every `every` seconds of timeline, one per clip, alternating `scale[0]`/`scale[1]`, skipping clips with other motion. `sfx_on_transitions` fills transitions that have no `sfx`. The expanded project has `auto` removed and is what `expand` prints and what renders use.

## 2026-11-12 — Face detection: YuNet via `ort`, behind the `onnx` feature
- **Decision:** `cutline-reframe/onnx` (forwarded as `onnx` by the render, qa and cli crates) compiles the YuNet 2023mar detector on ONNX Runtime (`ort` 2.0.0-rc.15; prebuilt runtime downloaded at build time). There is one process-wide ORT environment. Default builds have no detector: `track_face` then falls back to `fit_blur` with `W_REFRAME_NO_FACES`. CI and release builds enable `onnx`.
- **Reason:** the user chose optional features for heavy native dependencies. The feature is built and tested in this environment, including on the NASA public-domain face fixture.

## 2026-11-12 — Crop path via sendcmd
- **Decision:** the camera path is computed in Rust from 6 fps / 640 px analysis: greedy IoU-free nearest-center tracks, subject selection, dead zone with hysteresis, critically damped spring (ω = 6), max pan speed, hold then drift to center after 1.5 s lost, snap at `scdet` scene cuts. It is emitted as a `sendcmd` script driving `crop@rf` (x/y, and w/h for manual zoom keys), then scaled to the target. Crops use even pixel positions.
- **Reason:** deterministic, debuggable (the `.cmd` file is a readable cache artifact) and testable without ffmpeg.

## 2026-11-12 — Dev profile without debug info
- **Decision:** `[profile.dev] debug = 0`.
- **Reason:** test binaries linking ffmpeg-heavy crates pushed `target/` to 29 GB and filled the disk.

## 2026-11-12 — Explicit frame color properties (`setparams`)
- **Decision:** segment and final graphs end with `setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv`.
- **Reason:** with ffmpeg 9, frames leaving overlay/subtitle filters carry "unspecified" primaries/transfer, which override the `-color_*` output options. Pinned-ffmpeg renders then failed the color-tag QA check (caught by selftest under 9.0.2).

## 2026-11-12 — Review pack composition in Rust
- **Decision:** review images are composed with the `image` crate. Labels are drawn with the bundled Inter font via `ab_glyph`; frames are grabbed through ffmpeg PNG pipes with explicit BT.709 conversion. The pack uses an up-to-date final render when one exists (newer than the project file), otherwise the draft preview. It always produces `preview_540p.mp4` and `audio.png` from the draft's stems.
- **Music-vs-voice suggestion:** compares integrated loudness of the music stem (plus the envelope duck amount) with the voice/dialogue stem, and flags it when music is less than 18 LU below the voice.

## 2026-11-12 — Selftest goldens
- **Decision:** `selftest --full` compares against the golden sets in the source tree (dev builds) or `CUTLINE_GOLDEN_DIR`. When neither exists it checks durations, safe zones and rendering only, and says so in the report.

## 2026-11-13 — HTML renderer
- **Decision:** `chromiumoxide` 0.9 (tokio) drives Chrome. For each frame: pause and seek all Web Animations, seek SVG SMIL, `goToAndStop(frame, true)` for Lottie, await an optional `window.__cutline_seek(t)`, wait two rAFs, then capture a PNG screenshot. A warm-up seek to 0.05 s runs before frame 0 (otherwise the first capture can show the compositor's running animation). Params are injected via `Page.addScriptToEvaluateOnNewDocument`. Transparency uses `Emulation.setDefaultBackgroundColorOverride` with alpha 0.
- **Sandbox:** `Fetch` interception of every request. Only `data:`, `blob:` and `file://` paths under the source dir, the project dir, the bundled assets and the work dir load; everything else fails with `BlockedByClient` and is recorded.
- **Determinism flags:** `--font-render-hinting=none`, `--disable-lcd-text`, `--disable-font-subpixel-positioning`, `--force-color-profile=srgb`, `--run-all-compositor-stages-before-draw`, `--disable-threaded-animation`. Two renders produced byte-identical PNGs (tested).
- **Chrome:** `CUTLINE_CHROME` → `deps install chrome` (Chrome for Testing headless shell, sha recorded) → system Chrome/Chromium/Edge.
- **Caching:** the mezzanine is keyed by source content + sibling file fingerprints + params + size/fps/duration/alpha + Chrome path. HTML text templates (`*_html`) render full-frame with brand colors and the layer's params.
- **Lottie:** `lottie-web` 5.13.0 is vendored in `assets/html/` (MIT); the tarball sha256 is recorded in `assets/html/SOURCES.md`.

## 2026-11-13 — Manual reframe zoom after a fixed crop
- **Decision:** manual reframe keys move a fixed-size crop window (zoom 1) via sendcmd x/y only. Zoom is applied afterwards as a per-frame `scale` plus a centered `crop`, from an eased in-out-cubic expression built from the keys.
- **Reason:** changing `crop` w/h through sendcmd at runtime stalled ffmpeg's filter graph indefinitely; the paths-with-unicode test caught it.

## 2026-11-13 — Chapters and thumbnails
- **Decision:** chapters come from `markers`, else clip `chapter` fields, else clip ids (jump-cut pieces merged, ≥ 10 s apart). The first chapter is forced to 00:00; there are warnings for fewer than 3 chapters or any chapter under 10 s. Thumbnails take the frame at timeline time t, use the face crop when a crop path exists, cover-crop to 1280×720 (1080×1920 Shorts), add a small saturation/contrast pop, and render the template text statically (entrance animations stripped) at 1.5× title size.

## 2026-11-13 — Release packaging
- **Decision:** `scripts/release.sh` builds with `--features onnx` and packs the binary with the `assets/` directory beside it (found via `<exe_dir>/assets`): a universal macOS binary via `lipo`, and a zip on Windows. `.github/workflows/release.yml` runs it on `v*` tags.

## 2026-11-12 — macOS: ONNX Runtime link fixes
- **Decision:** with `onnx` on macOS, `cutline-reframe/build.rs` links clang's `libclang_rt.osx.a` (found via `cc --print-resource-dir`), and `.cargo/config.toml` sets `MACOSX_DEPLOYMENT_TARGET=13.4` unless the environment overrides it.
- **Reason:** the prebuilt ONNX Runtime (CoreML provider) calls `__isPlatformVersionAtLeast`. Rust's std defines it, but the release profile's thin LTO internalizes it, so every `--release --features onnx` link on macOS failed (debug builds and tests linked fine, which is why CI never saw it). The runtime is built for macOS 13.4; without the deployment target the binary claimed 11.0 and would crash on older systems.

## 2026-11-12 — Chrome fallback when a browser produces no frames
- **Decision:** the HTML renderer tries `chrome_candidates()` in order (installed headless shell, then system Chrome/Chromium/Edge; `CUTLINE_CHROME` alone when set). After launch it runs a content-independent health check (one `requestAnimationFrame` on `about:blank`, 15 s). A failed launch or health check moves on to the next browser; any other error is final.
- **Reason:** on macOS the freshly downloaded `chrome-headless-shell` never fired `requestAnimationFrame` for its first few minutes after `deps install chrome`, so every HTML layer failed with "warm-up seek timeout", while system Chrome worked. Most likely macOS first-launch checks of the new binary. It worked on its own afterwards and could not be reproduced on demand (the result is cached per binary).

## 2026-11-12 — Still images in face analysis, review grabs and thumbnails
- **Decision:** face analysis loops a still image (`-loop 1 -framerate <rate> -t <dur>`). Review frame grabs and thumbnails pass no `-ss` for still sources. The analysis cache key moved to `faces-v2`.
- **Reason:** with ffmpeg 9 any `-ss` (even 0) on an image input yields no frames, so `track_face` on a photo silently fell back to `fit_blur`, and `thumbnail` / `review` failed on photo clips.

## 2026-11-12 — Whisper model resolution and caption warnings
- **Decision:** `whisper::resolve_model()` is used by both `transcribe` and render: `CUTLINE_WHISPER_MODEL`, else `large-v3-turbo-q5_0` if installed, else `base.en` if installed. Render emits `W_CAPTIONS_NO_TRANSCRIPT` when captions are `auto`, speech assets exist and no words were found. Plan diagnostics now appear in render's human output and in `--json` (`exports[].diagnostics`), not only in `render_report.json`.
- **Reason:** `deps install --all` installs `base.en`, but transcribe and render asked for `large-v3-turbo-q5_0`. After the documented setup, `transcribe` failed and renders silently produced an empty `.srt` with no warning.

## 2026-11-13 — Cues resolve to seconds before the project is typed
- **Decision:** `"cue:name[±s]"` references are replaced by timeline seconds on the raw JSON in `pipeline::parse_file_with_cues`, using the voice track's transcript (asset `transcript` file, the transcript cache, or whisper). Allowed only in timeline times (layer `start`/`end`, sfx `at`, clip `end_at`, marker `at`). The voice asset must be played by exactly one audio layer with a numeric `start`.
- **Alternatives:** a `TimeValue::Cue` variant resolved inside `timeline::resolve`.
- **Reason:** `TimeValue` is `Copy` and used everywhere, and core has no transcript access. Resolving first means validation, `expand`, QA and rendering only ever see explicit numbers, and errors still carry JSON pointers to the reference or the cue.

## 2026-11-13 — `end_at` is the cut point, not the clip end
- **Decision:** `end_at` is where the cut to the next clip lands: the midpoint of the outgoing transition (or the clip end without one). With `in` and `out` the speed is computed; otherwise `out` is. Jump cuts and filler removal leave fitted clips whole.
- **Reason:** a viewer perceives the cut at the transition midpoint, so that is what should sit on the spoken cue. Splitting a fitted clip would change its length and break the fit.

## 2026-11-13 — Script alignment
- **Decision:** an asset `script` is aligned to its transcript with a banded Needleman–Wunsch over normalised tokens (exact 3, close 2, substitution −1, gap −2). Paired script words take the transcript word's times; unpaired ones are interpolated between neighbours (or share the previous word's span when there is no gap); transcript words absent from the script are dropped. `[bracketed]` audio tags and dash-only tokens are not words. Alignment happens wherever transcripts are loaded (captions, ducking, cues, jump cuts), after the transcript cache, so caches stay script-independent.
- **Alternatives:** forced alignment on the audio (needs an acoustic model); replacing caption text per segment.
- **Reason:** the TTS script is known exactly, and the recogniser's word timings are good; only its spelling is unreliable ("pains" for "panes"). Word-level pairing keeps per-word timing for karaoke and bold_pop styles.

## 2026-11-13 — Mask layers in the overlay pass
- **Decision:** `mask` is a layer type composited in z order with the other overlays (so a mask can cover a PiP or an image), before the ASS pass (so text and captions stay readable on top). Blur and pixelate crop the region from the current composite and overlay it back with `enable`; fill is a `drawbox`. Default blur sigma is half the region's short side, which removes text that fills the region.
- **Alternatives:** a per-clip effect (would not cover overlays or span clips); pre-processing the footage.
- **Reason:** redaction targets what the viewer sees, wherever it comes from, and must follow the same timing rules (anchor/offset, start/end, cues) as everything else.

## 2026-11-13 — Screencast profile and 4:4:4 delivery
- **Decision:** `profile: screencast` adds x264 `-tune animation` and lowers CRF by 2, and QA's freeze check passes (reporting the spans as expected). `chroma: "444"` switches segments to ProRes 4444 (yuv444p10le) and the deliverable to H.264 High 4:4:4 yuv444p; drafts and hardware encodes stay 4:2:0, and QA accepts either layout when 4:4:4 was requested.
- **Alternatives:** `-tune stillimage` (worse on scrolling and zooms); 4:4:4 by default for screencasts.
- **Reason:** terminal and UI footage is flat colour with hard edges and long still stretches. 4:4:4 stays opt-in because platforms re-encode to 4:2:0 and some players cannot decode High 4:4:4.

## 2026-11-13 — zoom_to computes the zoom from a rectangle
- **Decision:** `zoom_to` derives scale `s = min(1/w', 1/h')` (rect plus padding, capped at 3×) and the fixed point `f = x0·s/(s−1)` that puts the view window `x0` (centred on the rect, clamped inside the frame) on screen, and feeds them to the existing zoom chain as a ramp (+ release for `end`).
- **Reason:** pointing at a UI element is the common case in screencasts, and the focus-point maths is easy to get wrong by hand.

## 2026-11-13 — Music level and backtimed music
- **Decision:** `audio.music_level` measures the voice (or dialogue) and music stems after they are rendered and before the mix, and applies `voice − level − (music + duck)` dB to the music stem: the same measure `review` reports, which now uses the chosen level as its aim. `align: end` on an audio layer sets `src_in = duration − span` (or starts a shorter asset later) during timeline resolution, so audio planning, captions and QA see an ordinary layer.
- **Not done:** snapping cuts to musical beats (needs beat tracking; left for later).
- **Reason:** both replace trial-and-error renders: the Glance demo needed two renders to find `gain_db: -9` (17.1 LU); `music_level: 18` lands at 18 in one.

## 2026-11-13 — Cheaper HTML capture for drafts and stills
- **Decision:** drafts and previews capture HTML/SVG/Lottie at device scale 0.5 (same CSS layout) and at most 15 fps; `preview --at` (stills only) captures HTML clips and layers that are on screen at none of the requested times at 1 fps. Final renders are unchanged. Both settings are part of the HTML cache key.
- **Alternatives:** compositing only the requested frames (a second render path through the whole pipeline).
- **Reason:** profiling a cold preview of the Glance demo showed headless Chrome frame capture took ~76 of 86 s; segments, audio and the 540p proxy took ~6 s. Now ~11 s (stills away from HTML) to ~22 s.

## 2026-11-13 — Text-over-detail review check
- **Decision:** for each text-template box, review grabs a frame while the text shows and one 0.15 s before it appears (or after it leaves) in the same clip, both scaled to 960 px wide. Columns in the text's rows where new content appeared on a previously flat spot are the glyphs; the footprint grows sideways from the layout box while those stay contiguous. The mean luma gradient of the "before" frame under that footprint (+3 px) above 6 becomes a suggestion. HTML text templates are tagged `text_html` and skipped (their boxes are estimates).
- **Alternatives:** a ring around the layout box (missed text wider than its box and was fooled by neighbouring content); rendering a second pass without text layers.
- **Reason:** layout boxes under-measure some rendered text, and motion (zoom drift, ticking clocks) produces frame differences that are not text; requiring a flat spot before the change separates appearing glyphs from moving edges. Calibrated on the Glance demo: 0.4–2.3 for labels over empty UI, 48 for a label over scripture text.

## 2026-11-13 — Discovering a repo-local `.cutline-home`
- **Decision:** without `CUTLINE_HOME`, cutline uses the nearest `.cutline-home/` that contains `tools/`, searching up from the working directory and then from the binary's folder, before the platform data dir; the cache follows the home as it does with `CUTLINE_HOME`. `doctor` prints the home and how it was found.
- **Reason:** a checkout's `target/release/cutline` run from another folder (e.g. a video project) reported ffmpeg missing although the repo's `.cutline-home` had everything installed. Requiring `tools/` keeps stray empty folders from hijacking the home.

## 2026-11-13 — VHS tapes as assets
- **Decision:** an asset whose `path` ends in `.tape` is recorded with `vhs` (`CUTLINE_VHS` or `PATH`) when the project loads, before probing, into `.cutline/cache/terminal/<hash of tape + vhs version>.mp4`, and the asset then points at that file. The tape's `Output` lines are replaced; vhs runs in the tape's folder with cutline's ffmpeg first on `PATH`. A missing vhs (`E_TOOL_MISSING`) or a failed recording (`E_TAPE_FAILED`) is reported on the asset's `path`.
- **Alternatives:** a separate `cutline record` step; an `AssetKind::Terminal` handled throughout the pipeline.
- **Reason:** the edit stays reproducible from the project folder, and everything after loading (probe, segments, QA) treats the recording as ordinary footage. Recording is keyed on the tape text only, so a tape that shows live data (clocks, process lists) is re-recorded only when edited.

## 2026-11-13 — Correction: text template boxes are accurate
- **Finding:** the earlier note that layout boxes under-measure rendered text was wrong. Measured against libass, `Measure::width` is within 1 % for every bundled font (Inter regular/bold, Poppins regular/bold, Anton, Bebas Neue), and a `lower_third` box matches its ink within a few pixels at 1080p and at the 540p draft. The apparent overflow in the Glance demo came from a hand-drawn debug outline crossing the last glyph.
- **Change:** `text_boxes.rs` now pins box-versus-ink at both sizes. The review check keeps its frame-difference design for its other reason (motion and neighbouring content), and the comments no longer cite under-measured boxes.

## 2026-11-13 — Cues from a voice layer anchored to a clip
- **Decision:** `pipeline::load` resolves cues with the anchored voice layer's start provisionally 0, finds the anchor clip's start from the project truncated after that clip (with its own `end_at` cleared), and repeats with that start until it stops moving (at most 6 rounds; usually 2). No convergence means the anchor clip's position depends on the layer's own cues: `E_CUE_CYCLE`. Tapes are recorded and media probed once.
- **Reason:** "the narration starts half a second into the first screen clip" is a natural way to place a voice-over, and it should not require hand-computing the clip's start. The truncated resolve keeps the start measurable even while later clips are fitted to provisional cues.

## 2026-11-13 — Beat tracking and beat-snapped cuts
- **Decision:** `cutline-audio::beats` estimates tempo by autocorrelating a smoothed spectral-flux onset envelope (22.05 kHz, 512 hop) with a log-normal prior around 120 BPM, then places beats with Ellis' dynamic-programming tracker. `auto.snap_to_beats` runs last in `auto` expansion: cut by cut, in order, it re-resolves the timeline and sets `end_at` on the outgoing clip when a beat of the music layer lies within `max_shift`; a move the clip cannot make (validation errors) is reverted. Results are cached per asset fingerprint. This supersedes the earlier "not done" note.
- **Alternatives:** a bundled beat-tracking library or model; snapping to a fixed grid from a user-given BPM.
- **Reason:** no new dependency (rustfft was already in the workspace), deterministic results, and accuracy that suits cutting: within 35 ms of the true grid at 95–140 BPM with off-beat hats and noise in tests, and 110.1 BPM on the Glance demo's music, which was generated at "around 110 BPM". Smoothing the envelope before autocorrelation avoids the octave error that a non-integer beat period (in frames) otherwise causes.
