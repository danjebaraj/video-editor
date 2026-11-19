//! Viewable copies of photos Claude can't open (FEAT-0004). Claude Code's Read tool shows PNG,
//! JPEG, GIF and WebP only, so each HEIC/HEIF in the media list gets a small JPEG next to the
//! run (or assistant session) made with `cutline still`. The user's file is never touched.

use std::collections::BTreeMap;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};

use crate::run::prepare::ManifestEntry;
use crate::{media, paths};

/// The folder of copies inside the run or session folder. The app writes it; Claude reads it.
pub const PREVIEWS_DIR: &str = "media-previews";
/// Where Claude itself puts copies it makes with `cutline still` of photos outside the media
/// list (in a media or reference folder). Claude writes it; the app never reads it.
pub const CLAUDE_COPIES_DIR: &str = "photo-copies";
/// Longest side of a copy: the vision-input limit the review pack uses (spec §7.3).
pub const MAX_SIDE: u32 = 1568;
const QUALITY: u32 = 85;
const TIMEOUT: Duration = Duration::from_secs(60);

/// True for the photo formats Claude can't open itself.
pub fn needs_copy(path: &Utf8Path) -> bool {
    matches!(path.extension().map(|e| e.to_ascii_lowercase()).as_deref(), Some("heic" | "heif"))
}

/// The prompt line that tells Claude how to open any other HEIC/HEIF it finds: `cutline` is the
/// command as Claude should type it, `copies` the folder the copy must go to (relative to the
/// working folder or absolute).
pub fn on_demand_note(cutline: &str, copies: &str) -> String {
    format!(
        "For a HEIC/HEIF photo that isn't in that list (one you find in a media or reference folder), don't try Read on it: make a JPEG copy first with exactly `{cutline} still '<photo>' --out '{copies}/<name>.jpg' --max-side {MAX_SIDE}`, with the photo's full path and a plain file name filled in, then Read the copy. Run one photo per command, written out literally: no variables, `$(…)`, `~`, pipes, `&&`, `mkdir` or temporary folders; the copy must go into {copies}/, which already exists. Refer to the original photo, never the copy, in what you write."
    )
}

/// Why a copy can't be made with a cutline from before `cutline still` (BUG-0002).
pub const OUTDATED: &str =
    "this cutline is an older build that can't convert HEIC/HEIF photos; update cutline (Settings › cutline)";

/// One JPEG of `src` at `out` (longest side `MAX_SIDE`), or why there is none: the last
/// non-empty line cutline wrote to stderr (without its `error: ` prefix), or `timed out`.
pub fn make_copy(src: &Utf8Path, out: &Utf8Path, cutline: &Utf8Path) -> Result<(), String> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("can't create {d}: {e}"))?;
    }
    let mut cmd = std::process::Command::new(cutline.as_str());
    cmd.arg("still").arg(src.as_str()).arg("--out").arg(out.as_str()).args([
        "--max-side",
        &MAX_SIDE.to_string(),
        "--quality",
        &QUALITY.to_string(),
    ]);
    match crate::platform::run_capture(&mut cmd, TIMEOUT) {
        Ok((0, _, _)) if out.is_file() => Ok(()),
        Ok((_, _, err)) if err.contains("unrecognized subcommand 'still'") => Err(OUTDATED.into()),
        Ok((code, _, err)) => {
            let line = err.lines().rev().map(str::trim).find(|l| !l.is_empty());
            Err(line
                .map(|l| l.strip_prefix("error: ").unwrap_or(l).to_string())
                .unwrap_or_else(|| format!("cutline still exited with {code}")))
        }
        Err(e) if e.to_string().contains("timed out") => Err("timed out".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Makes `<out_dir>/media-previews/<media-id>.jpg` for every image entry whose file is a
/// HEIC/HEIF, and records it in the entry as `viewable_path` (or `viewable_error`). Copies are
/// reused while the photo's quick hash is unchanged (`index.json`); copies of media no longer
/// listed are deleted. One conversion at a time, each with a 60-second limit. A failure never
/// stops the caller. Returns true when every photo that needed a copy has one.
pub fn ensure_viewable_copies(entries: &mut [ManifestEntry], out_dir: &Utf8Path, cutline: &Utf8Path) -> bool {
    let dir = out_dir.join(PREVIEWS_DIR);
    let index_path = dir.join("index.json");
    let old: BTreeMap<String, String> =
        std::fs::read_to_string(&index_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut index: BTreeMap<String, String> = BTreeMap::new();
    let mut all = true;
    for e in entries.iter_mut() {
        e.viewable_path = None;
        e.viewable_error = None;
        let Some(src) = e.path.as_deref().map(Utf8PathBuf::from) else { continue };
        if e.kind != "image" || !needs_copy(&src) {
            continue;
        }
        let hash = match media::fingerprint(&src) {
            Ok(f) => f.quick_hash,
            Err(err) => {
                e.viewable_error = Some(format!("can't read the photo: {err}"));
                all = false;
                continue;
            }
        };
        let out = dir.join(format!("{}.jpg", e.id));
        let fresh = out.is_file() && old.get(&e.id) == Some(&hash);
        let made = if fresh { Ok(()) } else { make_copy(&src, &out, cutline) };
        match made {
            Ok(()) => {
                e.viewable_path = Some(paths::canonical_or_normal(&out).to_string());
                index.insert(e.id.clone(), hash);
            }
            Err(why) => {
                let _ = std::fs::remove_file(&out);
                e.viewable_error = Some(why);
                all = false;
            }
        }
    }
    // copies of media that left the list (or whose copy failed) go
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for f in rd.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            if let Some(id) = name.strip_suffix(".jpg") {
                if !index.contains_key(id) {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        }
    }
    if !index.is_empty() || index_path.exists() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = paths::write_json_atomic(&index_path, &index);
    }
    all
}

/// A JPEG copy of the photo `src` in an app-wide cache folder, keyed by its quick hash (the
/// Windows media host serves these: WebView2 can't show HEIC). Made once per version of the file.
pub fn cached_copy(src: &Utf8Path, cache_dir: &Utf8Path, cutline: &Utf8Path) -> Result<Utf8PathBuf, String> {
    let hash = media::fingerprint(src).map_err(|e| format!("can't read the photo: {e}"))?.quick_hash;
    let out = cache_dir.join(format!("{hash}.jpg"));
    if out.is_file() {
        return Ok(out);
    }
    // written beside, then moved: a half-written copy is never served
    let tmp = cache_dir.join(format!("{hash}.{}.tmp.jpg", std::process::id()));
    make_copy(src, &tmp, cutline)?;
    std::fs::rename(&tmp, &out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("can't keep the copy: {e}")
    })?;
    Ok(out)
}
