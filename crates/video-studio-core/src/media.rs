//! Media references (spec §6): files are used in place and never copied. Each item keeps enough
//! to find the file again after it moves — a bookmark, relative and absolute paths, a fingerprint.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

use camino::{Utf8Path, Utf8PathBuf};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::platform::FileRefs;
use crate::project::Project;
use crate::{ids, paths, timeutil, Error, Result};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Video,
    Audio,
    Image,
    Font,
    Transcript,
    Tape,
    DemoScript,
    Other,
}

impl MediaKind {
    pub fn label(self) -> &'static str {
        match self {
            MediaKind::Video => "Video",
            MediaKind::Audio => "Audio",
            MediaKind::Image => "Images",
            MediaKind::Font => "Fonts",
            MediaKind::Transcript => "Transcripts",
            MediaKind::Tape => "Terminal tapes",
            MediaKind::DemoScript => "Demo scripts",
            MediaKind::Other => "Other",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
            MediaKind::Image => "image",
            MediaKind::Font => "font",
            MediaKind::Transcript => "transcript",
            MediaKind::Tape => "tape",
            MediaKind::DemoScript => "demo_script",
            MediaKind::Other => "other",
        }
    }
    /// Kinds ffprobe can read.
    pub fn is_probeable(self) -> bool {
        matches!(self, MediaKind::Video | MediaKind::Audio | MediaKind::Image)
    }
}

/// A hint for Claude about how a file is meant to be used.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MediaRole {
    Footage,
    BRoll,
    ScreenRecording,
    Logo,
    Music,
    Sfx,
    Voice,
    Reference,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MediaPaths {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute: Option<String>,
    /// Relative to the project file's folder, with forward slashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative: Option<String>,
    /// Security-scoped bookmark (base64), when the macOS shell could make one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookmark_macos: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Fingerprint {
    pub size: u64,
    pub mtime: String,
    /// SHA-256 of the first, middle and last 1 MiB plus the size.
    pub quick_hash: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Probe {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Frame rate as `num/den`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_channels: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness_lufs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vfr: Option<bool>,
    /// The fingerprint hash this probe was made for; a different file is probed again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_hash: Option<String>,
    /// Why probing failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MediaItem {
    /// Stable, short and human-readable, for example `m-intro-take2`.
    pub id: String,
    pub kind: MediaKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<MediaRole>,
    #[serde(default)]
    pub paths: MediaPaths,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// A pasted clipboard image stored in the `.assets` sidecar (the one case the app writes media).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pasted: bool,
}

impl MediaItem {
    pub fn with_path(id: &str, kind: MediaKind, absolute: &str) -> Self {
        MediaItem {
            id: id.to_string(),
            kind,
            role: None,
            paths: MediaPaths { absolute: Some(absolute.to_string()), relative: None, bookmark_macos: None },
            fingerprint: None,
            probe: None,
            notes: None,
            tags: Vec::new(),
            pasted: false,
        }
    }

    pub fn file_name(&self) -> Option<&str> {
        self.paths
            .absolute
            .as_deref()
            .or(self.paths.relative.as_deref())
            .and_then(|p| p.rsplit(['/', '\\']).next())
            .filter(|s| !s.is_empty())
    }

    /// Stored paths in resolution order (relative first, then absolute), made absolute.
    pub fn candidate_paths(&self, project_dir: &Utf8Path) -> Vec<Utf8PathBuf> {
        let mut v = Vec::new();
        if let Some(r) = &self.paths.relative {
            v.push(paths::absolutize(Utf8Path::new(r), project_dir));
        }
        if let Some(a) = &self.paths.absolute {
            let p = paths::normalize(Utf8Path::new(a));
            if !v.contains(&p) {
                v.push(p);
            }
        }
        v
    }
}

/// Kind from the file name. `.demo.json5` and `.tape` are cutline inputs; JSON, SRT and VTT are
/// treated as transcripts (ElevenLabs Scribe JSON).
pub fn kind_for_path(path: &Utf8Path) -> MediaKind {
    let name = path.file_name().unwrap_or("").to_ascii_lowercase();
    if name.ends_with(".demo.json5") || name.ends_with(".demo.json") {
        return MediaKind::DemoScript;
    }
    match path.extension().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4" | "mov" | "m4v" | "mkv" | "webm" | "avi" | "mxf" | "mts" | "m2ts" | "mpg" | "mpeg" | "wmv") => {
            MediaKind::Video
        }
        Some("wav" | "mp3" | "m4a" | "aac" | "flac" | "ogg" | "opus" | "aif" | "aiff" | "wma") => MediaKind::Audio,
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "heic" | "svg" | "psd") => {
            MediaKind::Image
        }
        Some("ttf" | "otf" | "ttc" | "woff" | "woff2") => MediaKind::Font,
        Some("json" | "srt" | "vtt") => MediaKind::Transcript,
        Some("tape") => MediaKind::Tape,
        _ => MediaKind::Other,
    }
}

fn default_role(kind: MediaKind, name: &str) -> Option<MediaRole> {
    let n = name.to_ascii_lowercase();
    match kind {
        MediaKind::Video if n.contains("screen") || n.contains("recording") || n.contains("demo") => {
            Some(MediaRole::ScreenRecording)
        }
        MediaKind::Video if n.contains("broll") || n.contains("b-roll") => Some(MediaRole::BRoll),
        MediaKind::Video => Some(MediaRole::Footage),
        MediaKind::Image if n.contains("logo") => Some(MediaRole::Logo),
        MediaKind::Audio if n.contains("music") || n.contains("track") || n.contains("song") => Some(MediaRole::Music),
        MediaKind::Audio if n.contains("sfx") || n.contains("whoosh") => Some(MediaRole::Sfx),
        MediaKind::Audio if n.contains("vo") || n.contains("voice") || n.contains("narration") => {
            Some(MediaRole::Voice)
        }
        _ => None,
    }
}

const MIB: u64 = 1 << 20;

/// Size, modification time and a fast content hash (first, middle and last 1 MiB plus the size).
pub fn fingerprint(path: &Utf8Path) -> Result<Fingerprint> {
    use sha2::{Digest, Sha256};
    let mut f =
        std::fs::File::open(path).map_err(|e| Error::Io(std::io::Error::new(e.kind(), format!("{path}: {e}"))))?;
    let meta = f.metadata()?;
    let size = meta.len();
    let mut h = Sha256::new();
    let mut buf = vec![0u8; MIB as usize];
    let mut read_at = |f: &mut std::fs::File, offset: u64, len: u64| -> std::io::Result<()> {
        f.seek(SeekFrom::Start(offset))?;
        let mut left = len as usize;
        while left > 0 {
            let n = f.read(&mut buf[..left.min(MIB as usize)])?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            left -= n;
        }
        Ok(())
    };
    if size <= 3 * MIB {
        read_at(&mut f, 0, size)?;
    } else {
        read_at(&mut f, 0, MIB)?;
        read_at(&mut f, size / 2 - MIB / 2, MIB)?;
        read_at(&mut f, size - MIB, MIB)?;
    }
    h.update(size.to_le_bytes());
    let digest = h.finalize();
    Ok(Fingerprint {
        size,
        mtime: meta.modified().map(timeutil::system_time_rfc3339).unwrap_or_default(),
        quick_hash: digest.iter().map(|b| format!("{b:02x}")).collect(),
    })
}

fn project_dir(project_path: Option<&Utf8Path>) -> Utf8PathBuf {
    project_path
        .and_then(|p| p.parent())
        .filter(|p| !p.as_str().is_empty())
        .map(|p| p.to_owned())
        .unwrap_or_else(|| Utf8PathBuf::from("."))
}

fn set_location(item: &mut MediaItem, file: &Utf8Path, project_path: Option<&Utf8Path>, refs: &dyn FileRefs) {
    let abs = paths::normalize(file);
    item.paths.relative = project_path.and_then(|_| paths::relative_to(&abs, &project_dir(project_path)));
    item.paths.bookmark_macos = refs.create(&abs).or(None);
    item.paths.absolute = Some(abs.to_string());
}

/// Adds a file as a media item (by reference) and returns its ID. Adding a file that is already
/// in the project returns the existing item's ID.
pub fn add_media(
    project: &mut Project,
    project_path: Option<&Utf8Path>,
    file: &Utf8Path,
    refs: &dyn FileRefs,
) -> Result<String> {
    if !file.is_file() {
        return Err(Error::NotFound(format!("{file} isn't a file")));
    }
    let abs = paths::absolutize(
        file,
        &std::env::current_dir().ok().and_then(|p| paths::utf8(p).ok()).unwrap_or_else(|| Utf8PathBuf::from(".")),
    );
    let canon = paths::canonical_or_normal(&abs);
    let base = project_dir(project_path);
    for m in &project.media {
        if m.candidate_paths(&base).iter().any(|p| paths::canonical_or_normal(p) == canon) {
            return Ok(m.id.clone());
        }
    }
    let taken: HashSet<String> = project.media.iter().map(|m| m.id.clone()).collect();
    let stem = abs.file_stem().unwrap_or("media");
    let kind = kind_for_path(&abs);
    let mut item = MediaItem::with_path(&ids::unique_slug_id("m", stem, &taken), kind, abs.as_str());
    item.role = default_role(kind, stem);
    item.fingerprint = Some(fingerprint(&abs)?);
    set_location(&mut item, &abs, project_path, refs);
    let id = item.id.clone();
    project.media.push(item);
    Ok(id)
}

/// Adds every media file under `dir` (recursively), optionally only of the given kinds.
/// Hidden files and unknown file types are skipped. Returns the new or existing IDs.
pub fn add_folder(
    project: &mut Project,
    project_path: Option<&Utf8Path>,
    dir: &Utf8Path,
    kinds: Option<&[MediaKind]>,
    refs: &dyn FileRefs,
) -> Result<Vec<String>> {
    if !dir.is_dir() {
        return Err(Error::NotFound(format!("{dir} isn't a folder")));
    }
    let mut files: Vec<Utf8PathBuf> = Vec::new();
    for e in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
    {
        let Ok(e) = e else { continue };
        if !e.file_type().is_file() {
            continue;
        }
        let Ok(p) = paths::utf8(e.path().to_path_buf()) else { continue };
        let kind = kind_for_path(&p);
        if kind == MediaKind::Other || kinds.map(|k| !k.contains(&kind)).unwrap_or(false) {
            continue;
        }
        files.push(p);
    }
    files.sort();
    let mut ids = Vec::new();
    for f in files {
        ids.push(add_media(project, project_path, &f, refs)?);
    }
    Ok(ids)
}

// MARK: Resolution (spec §6.2)

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaStatus {
    Ok,
    /// Found at a different path than the stored one; the project is updated silently.
    Moved,
    /// Found, but the content differs from the fingerprint: Accept or Relink.
    Changed,
    Missing,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FoundBy {
    Bookmark,
    Relative,
    Absolute,
    SearchRoot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resolution {
    pub id: String,
    pub status: MediaStatus,
    /// The file's current absolute path, when found.
    pub path: Option<String>,
    pub found_by: Option<FoundBy>,
    /// The path the project had stored (shown for missing items).
    pub stored_path: Option<String>,
}

/// File names and sizes under the search roots, built once per resolve pass.
#[derive(Default)]
pub struct RootIndex {
    by_name: HashMap<String, Vec<Utf8PathBuf>>,
    by_size: HashMap<u64, Vec<Utf8PathBuf>>,
}

const INDEX_LIMIT: usize = 200_000;

impl RootIndex {
    pub fn build(roots: &[Utf8PathBuf]) -> Self {
        let mut idx = RootIndex::default();
        let mut count = 0;
        for root in roots {
            if !root.is_dir() {
                continue;
            }
            for e in walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
                let n = e.file_name().to_string_lossy();
                !(n.starts_with('.') || n == "node_modules" || n == "target")
            }) {
                let Ok(e) = e else { continue };
                if !e.file_type().is_file() {
                    continue;
                }
                let Ok(p) = paths::utf8(e.path().to_path_buf()) else { continue };
                if let Some(name) = p.file_name() {
                    idx.by_name.entry(name.to_lowercase()).or_default().push(p.clone());
                }
                if let Ok(m) = e.metadata() {
                    idx.by_size.entry(m.len()).or_default().push(p);
                }
                count += 1;
                if count >= INDEX_LIMIT {
                    return idx;
                }
            }
        }
        idx
    }

    /// The best match for an item: same name and fingerprint, then same fingerprint under another
    /// name (renamed), then — when `allow_changed` — the same name with different content.
    fn find(&self, item: &MediaItem, allow_changed: bool) -> Option<(Utf8PathBuf, bool)> {
        let name = item.file_name().map(|n| n.to_lowercase());
        let named: &[Utf8PathBuf] =
            name.as_ref().and_then(|n| self.by_name.get(n)).map(|v| v.as_slice()).unwrap_or(&[]);
        if let Some(fp) = &item.fingerprint {
            for p in named {
                if matches_fingerprint(p, fp) {
                    return Some((p.clone(), true));
                }
            }
            if let Some(same_size) = self.by_size.get(&fp.size) {
                for p in same_size {
                    if kind_for_path(p) == item.kind && matches_fingerprint(p, fp) {
                        return Some((p.clone(), true));
                    }
                }
            }
            if allow_changed {
                return named.first().map(|p| (p.clone(), false));
            }
            None
        } else {
            named.first().map(|p| (p.clone(), true))
        }
    }
}

fn matches_fingerprint(path: &Utf8Path, fp: &Fingerprint) -> bool {
    match std::fs::metadata(path) {
        Ok(m) if m.len() == fp.size => fingerprint(path).map(|f| f.quick_hash == fp.quick_hash).unwrap_or(false),
        _ => false,
    }
}

/// Whether the file at `path` still has the item's content. Size and mtime matching is trusted
/// without hashing; otherwise the quick hash decides.
fn content_matches(path: &Utf8Path, item: &MediaItem) -> bool {
    let Some(fp) = &item.fingerprint else { return true };
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if meta.len() != fp.size {
        return false;
    }
    let mtime = meta.modified().map(timeutil::system_time_rfc3339).unwrap_or_default();
    mtime == fp.mtime || fingerprint(path).map(|f| f.quick_hash == fp.quick_hash).unwrap_or(false)
}

/// Absolute search roots: the project's (relative to its folder) plus app-wide ones.
pub fn search_roots(project: &Project, project_path: Option<&Utf8Path>, app_roots: &[String]) -> Vec<Utf8PathBuf> {
    let base = project_dir(project_path);
    let mut out: Vec<Utf8PathBuf> = Vec::new();
    for r in project.media_roots.iter().chain(app_roots.iter()) {
        let p = paths::absolutize(&paths::expand_tilde(r), &base);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Resolves one item: bookmark, relative path, absolute path, then search roots.
pub fn resolve_item(
    item: &MediaItem,
    project_path: Option<&Utf8Path>,
    index: Option<&RootIndex>,
    refs: &dyn FileRefs,
) -> Resolution {
    let base = project_dir(project_path);
    let stored = item.paths.absolute.clone().or_else(|| item.paths.relative.clone());
    let stored_abs = item.paths.absolute.as_ref().map(|a| paths::normalize(Utf8Path::new(a)));
    let mut found: Option<(Utf8PathBuf, FoundBy)> = None;
    if let Some(b) = &item.paths.bookmark_macos {
        if let Some(p) = refs.resolve(b).filter(|p| p.is_file()) {
            found = Some((p, FoundBy::Bookmark));
        }
    }
    if found.is_none() {
        if let Some(r) = &item.paths.relative {
            let p = paths::absolutize(Utf8Path::new(r), &base);
            if project_path.is_some() && p.is_file() {
                found = Some((p, FoundBy::Relative));
            }
        }
    }
    if found.is_none() {
        if let Some(p) = stored_abs.as_ref().filter(|p| p.is_file()) {
            found = Some((p.clone(), FoundBy::Absolute));
        }
    }
    let mut via_root_exact = true;
    if found.is_none() {
        if let Some((p, exact)) = index.and_then(|i| i.find(item, false)) {
            via_root_exact = exact;
            found = Some((p, FoundBy::SearchRoot));
        }
    }
    match found {
        None => Resolution {
            id: item.id.clone(),
            status: MediaStatus::Missing,
            path: None,
            found_by: None,
            stored_path: stored,
        },
        Some((p, by)) => {
            let status = if !via_root_exact || !content_matches(&p, item) {
                MediaStatus::Changed
            } else if stored_abs
                .as_ref()
                .map(|s| paths::canonical_or_normal(s) != paths::canonical_or_normal(&p))
                .unwrap_or(true)
            {
                MediaStatus::Moved
            } else {
                MediaStatus::Ok
            };
            Resolution {
                id: item.id.clone(),
                status,
                path: Some(p.to_string()),
                found_by: Some(by),
                stored_path: stored,
            }
        }
    }
}

/// Resolves every media item. The search-root index is only built when something is missing
/// from its stored location.
pub fn resolve_all(
    project: &Project,
    project_path: Option<&Utf8Path>,
    app_roots: &[String],
    refs: &dyn FileRefs,
) -> Vec<Resolution> {
    let first: Vec<Resolution> = project.media.iter().map(|m| resolve_item(m, project_path, None, refs)).collect();
    if first.iter().all(|r| r.status != MediaStatus::Missing) {
        return first;
    }
    let roots = search_roots(project, project_path, app_roots);
    if roots.is_empty() {
        return first;
    }
    let index = RootIndex::build(&roots);
    project
        .media
        .iter()
        .zip(first)
        .map(
            |(m, r)| {
                if r.status == MediaStatus::Missing {
                    resolve_item(m, project_path, Some(&index), refs)
                } else {
                    r
                }
            },
        )
        .collect()
}

/// Updates stored paths for items found somewhere else (status Moved), and keeps relative paths
/// current. Returns true when the project changed and should be marked dirty.
pub fn apply_resolutions(
    project: &mut Project,
    project_path: Option<&Utf8Path>,
    resolutions: &[Resolution],
    refs: &dyn FileRefs,
) -> bool {
    let mut dirty = false;
    for r in resolutions {
        let Some(path) = &r.path else { continue };
        let Some(item) = project.media.iter_mut().find(|m| m.id == r.id) else { continue };
        let before = item.paths.clone();
        match r.status {
            MediaStatus::Moved => set_location(item, Utf8Path::new(path), project_path, refs),
            // Keep the relative path in step with where the project file is now.
            MediaStatus::Ok if project_path.is_some() => {
                item.paths.relative = paths::relative_to(Utf8Path::new(path), &project_dir(project_path));
            }
            _ => {}
        }
        dirty |= item.paths != before;
    }
    dirty
}

/// Accepts a changed file as the item's content: refreshes the fingerprint and drops the probe.
pub fn accept_changed(
    project: &mut Project,
    id: &str,
    path: &Utf8Path,
    project_path: Option<&Utf8Path>,
    refs: &dyn FileRefs,
) -> Result<()> {
    let fp = fingerprint(path)?;
    let item = project
        .media
        .iter_mut()
        .find(|m| m.id == id)
        .ok_or_else(|| Error::NotFound(format!("no media item \"{id}\"")))?;
    item.fingerprint = Some(fp);
    item.probe = None;
    set_location(item, path, project_path, refs);
    Ok(())
}

// MARK: Relinking (spec §6.3)

/// "The same old prefix became this new prefix", offered after one item is located.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrefixFix {
    pub old_prefix: String,
    pub new_prefix: String,
}

fn split_components(p: &str) -> Vec<&str> {
    p.split(['/', '\\']).collect()
}

/// Derives a prefix rewrite from one relocated file: the common trailing components are dropped
/// and what remains on each side is the prefix. `/Users/a/Footage/day1/x.mov` located at
/// `/Volumes/Work/Footage/day1/x.mov` gives `/Users/a` → `/Volumes/Work`.
pub fn suggest_prefix_fix(old_abs: &str, new_abs: &str) -> Option<PrefixFix> {
    let old = split_components(old_abs);
    let new = split_components(new_abs);
    let mut common = 0;
    while common < old.len() && common < new.len() && old[old.len() - 1 - common] == new[new.len() - 1 - common] {
        common += 1;
    }
    // Needs at least the file name in common, and prefixes that differ.
    if common == 0 || common == old.len() || common == new.len() {
        return None;
    }
    let sep_old = if old_abs.contains('\\') && !old_abs.contains('/') { "\\" } else { "/" };
    let sep_new = if new_abs.contains('\\') && !new_abs.contains('/') { "\\" } else { "/" };
    let old_prefix = old[..old.len() - common].join(sep_old);
    let new_prefix = new[..new.len() - common].join(sep_new);
    (old_prefix != new_prefix && !old_prefix.is_empty()).then_some(PrefixFix { old_prefix, new_prefix })
}

/// Applies a prefix rewrite to a stored path, if it starts with the old prefix.
pub fn rewrite_prefix(stored: &str, fix: &PrefixFix) -> Option<String> {
    let rest = stored.strip_prefix(&fix.old_prefix)?;
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')) {
        return None;
    }
    let rest = if fix.new_prefix.contains('/') || !fix.new_prefix.contains('\\') {
        rest.replace('\\', "/")
    } else {
        rest.to_string()
    };
    Some(format!("{}{}", fix.new_prefix, rest))
}

/// Points an item at a file the user picked. Returns a prefix fix to offer for the other missing
/// items when one can be derived from the old and new locations.
pub fn relink_item(
    project: &mut Project,
    project_path: Option<&Utf8Path>,
    id: &str,
    new_file: &Utf8Path,
    refs: &dyn FileRefs,
) -> Result<Option<PrefixFix>> {
    if !new_file.is_file() {
        return Err(Error::NotFound(format!("{new_file} isn't a file")));
    }
    let item = project
        .media
        .iter_mut()
        .find(|m| m.id == id)
        .ok_or_else(|| Error::NotFound(format!("no media item \"{id}\"")))?;
    let old_abs = item.paths.absolute.clone();
    let same_content = item.fingerprint.as_ref().map(|fp| matches_fingerprint(new_file, fp)).unwrap_or(false);
    if !same_content {
        // A different file was chosen on purpose: it becomes the item's content.
        item.fingerprint = Some(fingerprint(new_file)?);
        item.probe = None;
        item.kind = match kind_for_path(new_file) {
            MediaKind::Other => item.kind,
            k => k,
        };
    }
    set_location(item, new_file, project_path, refs);
    let new_abs = item.paths.absolute.clone().unwrap_or_default();
    Ok(old_abs.and_then(|o| suggest_prefix_fix(&o, &new_abs)))
}

/// Items a prefix fix would relink right now: `(id, new path)` for those whose rewritten path exists.
pub fn prefix_fix_candidates(
    project: &Project,
    project_path: Option<&Utf8Path>,
    fix: &PrefixFix,
    refs: &dyn FileRefs,
) -> Vec<(String, String)> {
    project
        .media
        .iter()
        .filter(|m| resolve_item(m, project_path, None, refs).status == MediaStatus::Missing)
        .filter_map(|m| {
            let new = rewrite_prefix(m.paths.absolute.as_deref()?, fix)?;
            Utf8Path::new(&new).is_file().then(|| (m.id.clone(), new))
        })
        .collect()
}

/// Rewrites the prefix for every missing item whose new path exists. Returns the relinked IDs.
pub fn apply_prefix_fix(
    project: &mut Project,
    project_path: Option<&Utf8Path>,
    fix: &PrefixFix,
    refs: &dyn FileRefs,
) -> Vec<String> {
    let candidates = prefix_fix_candidates(project, project_path, fix, refs);
    for (id, new) in &candidates {
        if let Some(item) = project.media.iter_mut().find(|m| &m.id == id) {
            set_location(item, Utf8Path::new(new), project_path, refs);
        }
    }
    candidates.into_iter().map(|(id, _)| id).collect()
}

/// Scans a folder and relinks every missing item it can match by name, then fingerprint.
/// A name match with different content is linked too and shows as Changed.
pub fn search_folder(
    project: &mut Project,
    project_path: Option<&Utf8Path>,
    dir: &Utf8Path,
    refs: &dyn FileRefs,
) -> Result<Vec<String>> {
    if !dir.is_dir() {
        return Err(Error::NotFound(format!("{dir} isn't a folder")));
    }
    let index = RootIndex::build(&[dir.to_owned()]);
    let mut relinked = Vec::new();
    let missing: Vec<String> = project
        .media
        .iter()
        .filter(|m| resolve_item(m, project_path, None, refs).status == MediaStatus::Missing)
        .map(|m| m.id.clone())
        .collect();
    for id in missing {
        let Some(item) = project.media.iter_mut().find(|m| m.id == id) else { continue };
        if let Some((p, _exact)) = index.find(item, true) {
            set_location(item, &p, project_path, refs);
            relinked.push(id);
        }
    }
    Ok(relinked)
}

// MARK: Probing

/// Reads media info with `cutline probe` (which wraps the pinned ffprobe) and maps it to [`Probe`].
pub fn probe_file(path: &Utf8Path, cutline: &Utf8Path, loudness: bool) -> Result<Probe> {
    let mut cmd = std::process::Command::new(cutline.as_str());
    cmd.arg("probe").arg("--json");
    if loudness {
        cmd.arg("--loudness");
    }
    cmd.arg(path.as_str());
    let (code, out, err) = crate::platform::run_capture(&mut cmd, std::time::Duration::from_secs(120))?;
    if code != 0 {
        let msg = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("probe failed").trim().to_string();
        return Err(Error::other(msg));
    }
    let v: serde_json::Value =
        serde_json::from_str(&out).map_err(|e| Error::other(format!("unexpected probe output: {e}")))?;
    Ok(probe_from_cutline(&v))
}

/// Maps `cutline probe --json` output to [`Probe`].
pub fn probe_from_cutline(v: &serde_json::Value) -> Probe {
    let video = &v["video"];
    let audio = &v["audio"];
    let u32_of = |x: &serde_json::Value| x.as_u64().map(|n| n as u32);
    Probe {
        duration_s: v["duration"].as_f64().filter(|d| *d > 0.0),
        width: u32_of(&video["display_width"]).or_else(|| u32_of(&video["width"])),
        height: u32_of(&video["display_height"]).or_else(|| u32_of(&video["height"])),
        fps: video["fps"].as_str().map(str::to_string),
        video_codec: video["codec"].as_str().map(str::to_string),
        audio_codec: audio["codec"].as_str().map(str::to_string),
        audio_channels: u32_of(&audio["channels"]),
        loudness_lufs: v["loudness"]["integrated_lufs"].as_f64(),
        rotation: video["rotation"].as_i64().map(|r| r as i32).filter(|r| *r != 0),
        vfr: video["vfr"].as_bool().filter(|b| *b),
        for_hash: None,
        error: None,
    }
}

/// Probes every resolved, probeable item whose cache is missing or stale. Returns true when any
/// probe was added or replaced.
pub fn probe_all(project: &mut Project, resolutions: &[Resolution], cutline: &Utf8Path) -> bool {
    let mut changed = false;
    for r in resolutions {
        let Some(path) = &r.path else { continue };
        let Some(item) = project.media.iter_mut().find(|m| m.id == r.id) else { continue };
        if !item.kind.is_probeable() || r.status == MediaStatus::Missing {
            continue;
        }
        let hash = item.fingerprint.as_ref().map(|f| f.quick_hash.clone());
        if item.probe.as_ref().map(|p| p.for_hash == hash && (p.error.is_none() || hash.is_none())).unwrap_or(false) {
            continue;
        }
        // Loudness needs a decode, so it is measured for audio files only (music, voice).
        let mut probe = match probe_file(Utf8Path::new(path), cutline, item.kind == MediaKind::Audio) {
            Ok(p) => p,
            Err(e) => Probe { error: Some(e.to_string()), ..Probe::default() },
        };
        probe.for_hash = hash;
        item.probe = Some(probe);
        changed = true;
    }
    changed
}

/// `1:05`, `12.4 s` style duration for lists and prompts.
pub fn format_duration(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        let total = seconds.round() as u64;
        if total >= 3600 {
            format!("{}:{:02}:{:02}", total / 3600, (total % 3600) / 60, total % 60)
        } else {
            format!("{}:{:02}", total / 60, total % 60)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::NoFileRefs;

    fn write(p: &Utf8Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    /// A project at `<root>/proj/p.cutproj` with three files under `<root>/footage`.
    fn fixture() -> (tempfile::TempDir, Utf8PathBuf, Utf8PathBuf, Project) {
        let dir = tempfile::tempdir().unwrap();
        let root = paths::canonical_or_normal(Utf8Path::from_path(dir.path()).unwrap());
        write(&root.join("footage/day1/hero.mov"), b"hero-video-bytes");
        write(&root.join("footage/day1/broll.mp4"), b"broll-bytes");
        write(&root.join("footage/music/track.wav"), b"music-bytes!");
        let path = root.join("proj/p.cutproj");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut p = Project::new("Fixture");
        add_folder(&mut p, Some(&path), &root.join("footage"), None, &NoFileRefs).unwrap();
        (dir, root, path, p)
    }

    #[test]
    fn adds_by_reference_with_readable_ids() {
        let (_d, root, path, mut p) = fixture();
        let ids: Vec<&str> = p.media.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["m-broll", "m-hero", "m-track"]);
        let hero = p.media_by_id("m-hero").unwrap();
        assert_eq!(hero.kind, MediaKind::Video);
        assert_eq!(hero.role, Some(MediaRole::Footage));
        assert_eq!(hero.paths.relative.as_deref(), Some("../footage/day1/hero.mov"));
        assert_eq!(hero.fingerprint.as_ref().unwrap().size, 16);
        assert_eq!(p.media_by_id("m-track").unwrap().role, Some(MediaRole::Music));
        // adding again returns the same item
        assert_eq!(add_media(&mut p, Some(&path), &root.join("footage/day1/hero.mov"), &NoFileRefs).unwrap(), "m-hero");
        assert_eq!(p.media.len(), 3);
        let all = resolve_all(&p, Some(&path), &[], &NoFileRefs);
        assert!(all.iter().all(|r| r.status == MediaStatus::Ok));
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_for_path("a/x.demo.json5".into()), MediaKind::DemoScript);
        assert_eq!(kind_for_path("x.tape".into()), MediaKind::Tape);
        assert_eq!(kind_for_path("scribe.json".into()), MediaKind::Transcript);
        assert_eq!(kind_for_path("Inter.TTF".into()), MediaKind::Font);
        assert_eq!(kind_for_path("notes.txt".into()), MediaKind::Other);
    }

    #[test]
    fn moved_project_resolves_by_absolute_then_updates_relative() {
        let (_d, root, path, mut p) = fixture();
        // the project file moves; the media stay
        let new_path = root.join("elsewhere/deep/p.cutproj");
        std::fs::create_dir_all(new_path.parent().unwrap()).unwrap();
        let res = resolve_all(&p, Some(&new_path), &[], &NoFileRefs);
        assert!(res.iter().all(|r| r.status == MediaStatus::Ok && r.found_by == Some(FoundBy::Absolute)));
        assert!(apply_resolutions(&mut p, Some(&new_path), &res, &NoFileRefs), "relative paths are refreshed");
        assert_eq!(p.media_by_id("m-hero").unwrap().paths.relative.as_deref(), Some("../../footage/day1/hero.mov"));
        let _ = path;
    }

    #[test]
    fn moved_together_resolves_by_relative_path() {
        let (_d, root, path, mut p) = fixture();
        // project and footage both move to a "new machine": absolute paths are dead, relative ones hold
        let machine = root.join("new-machine");
        std::fs::create_dir_all(&machine).unwrap();
        std::fs::rename(root.join("footage"), machine.join("footage")).unwrap();
        std::fs::rename(root.join("proj"), machine.join("proj")).unwrap();
        let new_path = machine.join("proj/p.cutproj");
        let res = resolve_all(&p, Some(&new_path), &[], &NoFileRefs);
        assert!(res.iter().all(|r| r.status == MediaStatus::Moved && r.found_by == Some(FoundBy::Relative)), "{res:?}");
        assert!(apply_resolutions(&mut p, Some(&new_path), &res, &NoFileRefs));
        assert!(p.media_by_id("m-hero").unwrap().paths.absolute.as_deref().unwrap().contains("new-machine"));
        let _ = path;
    }

    #[test]
    fn search_roots_find_renamed_and_moved_files_by_fingerprint() {
        let (_d, root, path, mut p) = fixture();
        std::fs::create_dir_all(root.join("archive/x")).unwrap();
        std::fs::rename(root.join("footage/day1/hero.mov"), root.join("archive/x/hero-final.mov")).unwrap();
        let res = resolve_all(&p, Some(&path), &[], &NoFileRefs);
        assert_eq!(res.iter().find(|r| r.id == "m-hero").unwrap().status, MediaStatus::Missing);
        p.media_roots = vec!["../archive".into()];
        let res = resolve_all(&p, Some(&path), &[], &NoFileRefs);
        let hero = res.iter().find(|r| r.id == "m-hero").unwrap();
        assert_eq!((hero.status, hero.found_by), (MediaStatus::Moved, Some(FoundBy::SearchRoot)));
        assert!(hero.path.as_deref().unwrap().ends_with("hero-final.mov"));
        // an app-wide root works the same way
        p.media_roots.clear();
        let res = resolve_all(&p, Some(&path), &[root.join("archive").to_string()], &NoFileRefs);
        assert_eq!(res.iter().find(|r| r.id == "m-hero").unwrap().status, MediaStatus::Moved);
    }

    #[test]
    fn changed_content_is_flagged_and_can_be_accepted() {
        let (_d, root, path, mut p) = fixture();
        let hero = root.join("footage/day1/hero.mov");
        std::fs::write(&hero, b"a different edit of the hero shot").unwrap();
        let res = resolve_all(&p, Some(&path), &[], &NoFileRefs);
        assert_eq!(res.iter().find(|r| r.id == "m-hero").unwrap().status, MediaStatus::Changed);
        accept_changed(&mut p, "m-hero", &hero, Some(&path), &NoFileRefs).unwrap();
        let res = resolve_all(&p, Some(&path), &[], &NoFileRefs);
        assert_eq!(res.iter().find(|r| r.id == "m-hero").unwrap().status, MediaStatus::Ok);
    }

    #[test]
    fn locate_one_then_prefix_fix_relinks_the_rest() {
        let (_d, root, path, mut p) = fixture();
        std::fs::create_dir_all(root.join("Volumes/Work")).unwrap();
        std::fs::rename(root.join("footage"), root.join("Volumes/Work/footage")).unwrap();
        // absolute and relative paths are both dead now
        let res = resolve_all(&p, Some(&path), &[], &NoFileRefs);
        assert!(res.iter().all(|r| r.status == MediaStatus::Missing));
        let fix =
            relink_item(&mut p, Some(&path), "m-hero", &root.join("Volumes/Work/footage/day1/hero.mov"), &NoFileRefs)
                .unwrap()
                .expect("a prefix fix is offered");
        assert_eq!(fix.old_prefix, root.to_string());
        assert_eq!(fix.new_prefix, root.join("Volumes/Work").to_string());
        let would = prefix_fix_candidates(&p, Some(&path), &fix, &NoFileRefs);
        assert_eq!(would.len(), 2);
        let done = apply_prefix_fix(&mut p, Some(&path), &fix, &NoFileRefs);
        assert_eq!(done, vec!["m-broll".to_string(), "m-track".to_string()]);
        let res = resolve_all(&p, Some(&path), &[], &NoFileRefs);
        assert!(res.iter().all(|r| r.status == MediaStatus::Ok), "{res:?}");
    }

    #[test]
    fn search_folder_matches_by_name_then_fingerprint() {
        let (_d, root, path, mut p) = fixture();
        std::fs::create_dir_all(root.join("drive/a")).unwrap();
        std::fs::rename(root.join("footage/day1/hero.mov"), root.join("drive/a/hero.mov")).unwrap();
        std::fs::rename(root.join("footage/day1/broll.mp4"), root.join("drive/a/renamed-broll.mp4")).unwrap();
        // same name, different content
        std::fs::remove_file(root.join("footage/music/track.wav")).unwrap();
        write(&root.join("drive/track.wav"), b"a remastered track");
        let relinked = search_folder(&mut p, Some(&path), &root.join("drive"), &NoFileRefs).unwrap();
        assert_eq!(relinked.len(), 3);
        let res = resolve_all(&p, Some(&path), &[], &NoFileRefs);
        let status = |id: &str| res.iter().find(|r| r.id == id).unwrap().status;
        assert_eq!(status("m-hero"), MediaStatus::Ok);
        assert_eq!(status("m-broll"), MediaStatus::Ok, "found under a new name by fingerprint");
        assert_eq!(status("m-track"), MediaStatus::Changed, "name matched, content differs");
    }

    #[test]
    fn prefix_suggestions() {
        let f = suggest_prefix_fix("/Users/a/Footage/day1/x.mov", "/Volumes/Work/Footage/day1/x.mov").unwrap();
        assert_eq!((f.old_prefix.as_str(), f.new_prefix.as_str()), ("/Users/a", "/Volumes/Work"));
        assert_eq!(rewrite_prefix("/Users/a/Footage/y.mov", &f).unwrap(), "/Volumes/Work/Footage/y.mov");
        assert_eq!(rewrite_prefix("/Users/ab/Footage/y.mov", &f), None, "prefix must end at a path separator");
        assert_eq!(suggest_prefix_fix("/a/x.mov", "/b/y.mov"), None);
        assert_eq!(suggest_prefix_fix("/a/x.mov", "/a/x.mov"), None);
        // a Windows project opened on a Mac
        let w = suggest_prefix_fix("D:\\Footage\\day1\\x.mov", "/Volumes/Work/Footage/day1/x.mov").unwrap();
        assert_eq!(w.old_prefix, "D:");
        assert_eq!(rewrite_prefix("D:\\Footage\\music\\t.wav", &w).unwrap(), "/Volumes/Work/Footage/music/t.wav");
    }

    #[test]
    fn bookmark_wins_when_it_resolves() {
        struct Refs(Utf8PathBuf);
        impl FileRefs for Refs {
            fn create(&self, _p: &Utf8Path) -> Option<String> {
                Some("Ym9va21hcms=".into())
            }
            fn resolve(&self, _r: &str) -> Option<Utf8PathBuf> {
                Some(self.0.clone())
            }
        }
        let (_d, root, path, mut p) = fixture();
        let moved = root.join("renamed-on-same-volume.mov");
        std::fs::rename(root.join("footage/day1/hero.mov"), &moved).unwrap();
        p.media.iter_mut().find(|m| m.id == "m-hero").unwrap().paths.bookmark_macos = Some("Ym9va21hcms=".into());
        let refs = Refs(moved.clone());
        let r = resolve_item(p.media_by_id("m-hero").unwrap(), Some(&path), None, &refs);
        assert_eq!((r.status, r.found_by), (MediaStatus::Moved, Some(FoundBy::Bookmark)));
        assert!(apply_resolutions(&mut p, Some(&path), &[r], &refs));
        assert_eq!(p.media_by_id("m-hero").unwrap().paths.absolute.as_deref(), Some(moved.as_str()));
    }

    #[test]
    fn maps_cutline_probe_output() {
        let v = serde_json::json!({"duration": 32.1, "video": {"codec": "h264", "width": 1080, "height": 1920,
            "display_width": 1920, "display_height": 1080, "fps": "30000/1001", "vfr": true, "rotation": 90},
            "audio": {"codec": "aac", "channels": 2}, "loudness": {"integrated_lufs": -16.2}});
        let p = probe_from_cutline(&v);
        assert_eq!((p.width, p.height), (Some(1920), Some(1080)));
        assert_eq!(p.fps.as_deref(), Some("30000/1001"));
        assert_eq!((p.vfr, p.rotation, p.audio_channels), (Some(true), Some(90), Some(2)));
        assert_eq!(p.loudness_lufs, Some(-16.2));
        assert_eq!(format_duration(65.2), "1:05");
        assert_eq!(format_duration(12.44), "12.4 s");
    }
}
