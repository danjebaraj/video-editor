---
id: FEAT-0004
type: feature
title: add support for heic files
status: partial
implementation: partial
approved: yes
approved-on: 2026-11-19
priority: medium
created: 2026-11-19
updated: 2026-11-19
attachments: []
---

# add support for heic files

## Summary

* The assistant is not able to read heic files.
* Please see message below.
* Please ensure that the Video Studio is also able to work with these files.

I can't open HEIC photos directly, so I need you to tell me what each one shows before I can place them in the steps. A few other choices are yours too.

### What this item delivers

iPhone photos arrive as HEIC (`.heic`, sometimes `.heif`). Today they half-work:

- Video Studio files them as images (`kind_for_path` in `crates/video-studio-core/src/media.rs` lists `heic`), but Claude can't see them. Claude Code's Read tool only shows PNG, JPEG, GIF and WebP, so both the spec assistant and dry/production runs have to ask the user what each photo shows (the message above).
- cutline doesn't know them at all. `default_kind_from_ext` (`crates/cutline-core/src/timeline.rs`) and `IMAGE_EXT` (`crates/cutline-media/src/probe.rs`) leave `heic`/`heif` out, so a HEIC asset is treated as video. iPhone HEICs are also tiled (a grid of 512×512 HEVC tiles) and carry a rotation, which a plain `-loop 1 -i photo.heic` input doesn't handle.

After this item:

1. Claude can look at every HEIC/HEIF in a project. Video Studio makes a small JPEG "viewable copy" of each one in the run folder or the assistant session folder and tells Claude where it is.
2. cutline accepts HEIC/HEIF anywhere it accepts a PNG or JPEG: main-storyline stills, `image` layers, logos and B-roll stills. It decodes each photo once, upright and at full resolution, into a cached PNG and renders from that.
3. The original file is never modified, moved or copied next to the user's media.

## Requirements

### R1. cutline: recognise HEIC/HEIF as images

- [x] `default_kind_from_ext` in `crates/cutline-core/src/timeline.rs` returns `AssetKind::Image` for `heic` and `heif`, in any letter case.
- [x] `IMAGE_EXT` in `crates/cutline-media/src/probe.rs` includes `heic` and `heif`, so `cutline probe` reports `kind: "image"` for them.
- [x] `cutline probe --json photo.heic` reports the size of the whole picture as displayed: the composed tile grid (for example 4032×3024), not one tile (512×512), with rotation applied (a portrait iPhone photo reports `display_width` 3024 and `display_height` 4032). `duration` is absent or 0, as for a PNG.
- [x] `kind_for_path` in `crates/video-studio-core/src/media.rs` also returns `MediaKind::Image` for `heif` (it already does for `heic`).

### R2. cutline: decode once into a cached still

- [x] New function `cutline_media::still::decode_still(tc, path, cache) -> Result<PathBuf>` in a new file `crates/cutline-media/src/still.rs`. Input: a HEIC/HEIF path. Output: the path of an 8-bit PNG in the cutline cache with:
  - the primary image only (bursts and image sequences inside the file are ignored);
  - every tile composed into one picture at full resolution;
  - the stored rotation and mirroring (`irot`/`imir`, EXIF orientation) applied, so the picture is upright;
  - BT.709/sRGB colour; 10-bit sources are reduced to 8-bit; HDR gain maps are ignored (the SDR base image is used);
  - an alpha channel only when the source has one.
- [x] The cache key is the asset fingerprint plus the string `heic-still-v1`. A second call for an unchanged file runs no ffmpeg process (assert in a test by counting spawned processes or by cache hit).
- [x] Asset loading in `crates/cutline-media/src/assets.rs`: when an asset's extension is `heic` or `heif`, `AssetMedia.path` points at the decoded PNG; `AssetMedia.fingerprint` stays the original file's fingerprint. Every renderer path that uses `-loop 1 -i <path>` for images (`segment.rs`, `overlay.rs`, `extras.rs`, review pack) therefore works unchanged.
- [x] `extras.rs` (the `["png", "jpg", "jpeg", "webp"]` check near line 116) accepts `heic` and `heif` and uses the decoded still.
- [x] `credits.txt` and the render report list the original `.heic` path, not the cached PNG.
- [x] Decoding uses the pinned ffmpeg from `cutline deps`; no new external tool. The decode command appears in `render --dry-run` output like other preparatory commands.

### R3. cutline: errors and doctor

- [x] When a HEIC/HEIF can't be decoded (corrupt file, or the ffmpeg build can't read it), `cutline validate` and `cutline render` fail with error code `E_IMAGE_DECODE`, the asset's JSON pointer as `path`, the ffmpeg reason in `message`, and the hint `Export the photo as JPEG (Photos › File › Export) and use that file instead.`
- [x] `cutline doctor` checks for the `heif` demuxer and `hevc` decoder and that ffmpeg is at least 7.1 (the first release that composes HEIF tile grids). They are optional capabilities: when missing, doctor prints a warning `HEIC photos can't be used: <reason>`, and every other check is unaffected.
- [x] `cutline still <in> --out <file> [--max-side <px>] [--quality <1-100>]`: new subcommand. Writes a JPEG (`.jpg`/`.jpeg`) or PNG (`.png`, chosen by the `--out` extension) of the primary image, upright, scaled so the longest side is at most `--max-side` (default: no scaling; never upscales). JPEG quality defaults to 85. Accepts any image cutline can read (PNG, JPEG, WebP, BMP, TIFF, GIF first frame, HEIC, HEIF). `--json` prints `{"out": "<path>", "width": n, "height": n}`. Exit codes follow spec §5 (1 for an undecodable input, with the `E_IMAGE_DECODE` message).

### R4. Video Studio: viewable copies for Claude

- [x] New module `crates/video-studio-core/src/viewable.rs` with `ensure_viewable_copies(entries: &mut [ManifestEntry], out_dir: &Utf8Path, cutline: &Utf8Path) -> bool`. For every entry with `kind == "image"`, a resolved `path` and an extension of `heic` or `heif`, it makes `<out_dir>/media-previews/<media-id>.jpg` by running `cutline still <path> --out <that file> --max-side 1568 --quality 85`. 1568 px matches the vision-input limit the review pack uses (spec §7.3).
- [x] Copies are reused: `<out_dir>/media-previews/index.json` maps media ID → the fingerprint `quick_hash` the copy was made from. A copy is made again only when the hash differs or the file is missing. Copies for media IDs no longer in the manifest are deleted.
- [x] Each conversion has a 60-second timeout. Conversions run one after another (photos are few; this keeps the prompt deterministic).
- [x] `ManifestEntry` in `crates/video-studio-core/src/run/prepare.rs` gains two optional fields, omitted from JSON when absent:
  - `viewable_path`: absolute path of the JPEG copy;
  - `viewable_error`: why no copy could be made (the last non-empty stderr line of `cutline still`, or `timed out`).
- [x] `prepare()` calls `ensure_viewable_copies` with the run folder after `manifest()` and before `media-manifest.json` is written, so the file includes the new fields. A failed conversion never fails the run.
- [x] The spec assistant (`crates/video-studio-core/src/assistant.rs`) calls it with the session folder before Claude starts. The copies sit inside the session folder, which Claude can already read, so no new read path is added.
- [x] `manifest_summary` in `crates/video-studio-core/src/run/prompt.rs` appends ` — view: media-previews/<id>.jpg` to the line of an item with a viewable copy, and ` — can't be viewed: <viewable_error>` to one whose copy failed.
- [x] The run prompt's description of `media-manifest.json` (around line 410 of `prompt.rs`) and `assistant_system_prompt` each gain this sentence: `HEIC and HEIF photos can't be opened with Read; open the JPEG at viewable_path (listed as "view:") to see the picture, and keep referring to the original file (its path in project.json5, media:<id> in spec.md).`
- [x] The run's write policy keeps `media-previews/` writable only by the app, not by Claude (it is created before Claude starts; Claude only reads it).

### R5. Video Studio apps: show HEIC media

- [x] macOS (`apps/macos`): the media list and the spec preview show a thumbnail for a HEIC/HEIF item, upright, the same as for a JPEG. (ImageIO decodes HEIC natively; verify and fix only what doesn't show.)
- [ ] Windows (`apps/windows`): `media_server.rs` serves a `.heic`/`.heif` request as `image/jpeg` from a viewable copy made with `cutline still --max-side 1568` and cached in the app's cache folder (keyed by fingerprint `quick_hash`). WebView2 can't display HEIC itself. When the copy fails, the item shows the generic image placeholder with the tooltip `Can't preview this HEIC photo: <reason>`.
- [x] Adding a folder (`add_folder`) picks up `.heic` and `.heif` files as images, as it does `.jpg`.

## Acceptance criteria

- [x] Fixtures: `tests/fixtures/heic/portrait-tiled.heic` (an iPhone photo: tiled grid, rotation 90°, about 4032×3024 stored) and `tests/fixtures/heic/single.heif` (one untiled image, no rotation, under 1000 px). Each is under 1.5 MB, owned by the maintainer and released as CC0, recorded in `tests/fixtures/SOURCES.md`.
- [x] `cutline probe --json tests/fixtures/heic/portrait-tiled.heic` reports `kind` `image`, `display_width` 3024 and `display_height` 4032.
- [x] `decode_still` on the tiled fixture gives a 3024×4032 PNG whose top-left 64×64 block matches a blessed reference PNG (SSIM ≥ 0.97, bless with `CUTLINE_BLESS=1`), proving the tiles are composed in order and the picture is upright.
- [x] A Shorts fixture project with the tiled HEIC as a 3-second main clip (`fit_blur`) and the untiled HEIF as an `image` layer validates, renders and passes QA with no errors. The contact sheet shows both photos upright.
- [x] Rendering the same project twice decodes each HEIC only once (cache hit asserted).
- [x] A truncated copy of the fixture (first 10 KB) makes `cutline validate` exit 1 with `E_IMAGE_DECODE` at `/assets/<id>`.
- [x] `cutline still tests/fixtures/heic/portrait-tiled.heic --out x.jpg --max-side 1568 --json` prints width 1176 and height 1568 and writes a JPEG the `image` crate can decode.
- [x] Video Studio: preparing a run for a project with one HEIC, one JPEG and one missing HEIC writes `media-previews/<heic-id>.jpg` (longest side 1568), gives the HEIC's manifest entry a `viewable_path`, gives the JPEG none, gives the missing HEIC neither field, and the prompt's media list shows `view: media-previews/<heic-id>.jpg` on the HEIC line.
- [x] Preparing a second run for the unchanged project with the same session/run folder reuses the copy (no `cutline still` call; assert with a counting test double for the cutline binary).
- [x] When `cutline still` fails (test double exits 1 with `E_IMAGE_DECODE: bad file`), the manifest entry has `viewable_error: "E_IMAGE_DECODE: bad file"`, the media line reads `can't be viewed: E_IMAGE_DECODE: bad file`, and the run still starts.
- [ ] Manual check (macOS): ask the spec assistant "What does each photo show?" for a project with two iPhone HEIC photos. It describes both without asking the user.
- [ ] Manual check (Windows): the media list shows thumbnails for the two fixtures.
- [x] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass; `cutline selftest --quick` still passes.

## Design notes

- **Why a decoded still in cutline, not a converted file next to the photo.** Video Studio uses media in place and never copies it (`media.rs`, ui spec §6). A cached PNG in cutline's content-addressed cache keeps that rule, and it means `-loop 1 -i` keeps working everywhere without special cases for tile grids or rotation.
- **Why ffmpeg and not ImageIO/`sips` or WIC.** cutline must behave the same on macOS and Windows (spec §1, M9). Windows needs the paid-store HEIF extension for WIC, whereas the pinned static ffmpeg builds decode HEVC on both. ffmpeg 7.1 composes HEIF tile grids automatically in the CLI; older builds fail with `E_IMAGE_DECODE`, which doctor warns about.
- **Why viewable copies live in the run/session folder.** Claude can already read those folders, so the read policy doesn't change, and the copies disappear with the run. The 1568 px limit keeps image tokens low; renders always use the full-resolution still, never the preview.
- **Spec changes.** This item changes what the top-level `spec.md` describes: §4 Rules (asset kinds by extension: add `heic`/`heif` as images, decoded to a cached still), §5 CLI (new `cutline still` command) and §9 doctor capabilities (optional `heif` demuxer and `hevc` decoder, ffmpeg ≥ 7.1). `ui/spec.md` gains the viewable-copy behaviour for the media manifest and the assistant. Update both specs only when asked.
- **Out of scope (possible follow-ups):** viewable copies for other formats Claude's Read tool can't show (TIFF, BMP, PSD, AVIF); Live Photo motion (the `.mov` half of a Live Photo is handled as a separate video file, as today); keeping HDR gain maps; editing EXIF.

## Implementation log

### 2026-11-19
- Implemented: R1–R4 in full; R5 macOS (ImageIO already shows HEIC upright, checked on the fixtures) and `add_folder`; all automated acceptance criteria; fmt, clippy, the workspace tests and `selftest --quick`/`--full` pass.
- Not done: R5 Windows `media_server.rs` is written (HEIC served as cached JPEGs, placeholder with the reason in the sidebar) but not built or tested on Windows; the two manual checks with real iPhone photos.
- Notes: HEIF is read by ffmpeg's `mov` demuxer (there is no `heif` demuxer), so doctor checks `mov` + `hevc` + ffmpeg ≥ 7.1. Tile grids are composed in ffmpeg's own filter graph, so the decode uses `-pix_fmt` instead of `-vf`. The fixtures are synthetic test cards written by macOS ImageIO in the iPhone layout. The assistant gets the media list in `AssistantRequest.media` (from `project.summary`) and lists the copies after the project summary.
- Files: `crates/cutline-media/src/{probe,still,assets}.rs`, `crates/cutline-cli/src/{still_cmd,doctor,edit}.rs`, `crates/video-studio-core/src/{viewable,assistant}.rs`, `crates/video-studio-core/src/run/{prepare,prompt}.rs`, `apps/windows/src/media_server.rs`

