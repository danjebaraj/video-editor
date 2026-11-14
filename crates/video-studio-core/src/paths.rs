//! Path helpers and the app library location.

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

use crate::{Error, Result, APP_NAME};

pub fn home_dir() -> Option<Utf8PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(var).ok().filter(|s| !s.is_empty()).map(Utf8PathBuf::from)
}

/// Expands a leading `~` to the home directory.
pub fn expand_tilde(p: &str) -> Utf8PathBuf {
    if p == "~" {
        return home_dir().unwrap_or_else(|| Utf8PathBuf::from(p));
    }
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        if let Some(h) = home_dir() {
            return h.join(rest);
        }
    }
    Utf8PathBuf::from(p)
}

/// Replaces the home directory prefix with `~` for display and portable storage.
pub fn collapse_tilde(p: &Utf8Path) -> String {
    if let Some(h) = home_dir() {
        if let Ok(rest) = p.strip_prefix(&h) {
            return if rest.as_str().is_empty() { "~".into() } else { format!("~/{}", rest.as_str().replace('\\', "/")) };
        }
    }
    p.to_string()
}

/// The app library: brands, output presets, settings, caches (spec §8).
/// `VIDEO_STUDIO_HOME` overrides the location (tests, portable setups).
pub fn library_dir() -> Utf8PathBuf {
    if let Ok(p) = std::env::var("VIDEO_STUDIO_HOME") {
        if !p.is_empty() {
            return Utf8PathBuf::from(p);
        }
    }
    let home = home_dir().unwrap_or_else(|| Utf8PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support").join(APP_NAME)
    } else if cfg!(windows) {
        std::env::var("APPDATA").map(Utf8PathBuf::from).unwrap_or(home).join(APP_NAME)
    } else {
        std::env::var("XDG_DATA_HOME")
            .map(Utf8PathBuf::from)
            .unwrap_or_else(|_| home.join(".local/share"))
            .join("syncfusion-video-studio")
    }
}

/// The default parent of run folders: `~/Movies/Syncfusion Video Studio/`.
pub fn default_output_root() -> Utf8PathBuf {
    let home = home_dir().unwrap_or_else(|| Utf8PathBuf::from("."));
    let movies = if cfg!(windows) { "Videos" } else { "Movies" };
    home.join(movies).join(APP_NAME)
}

pub fn utf8(p: std::path::PathBuf) -> Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(p).map_err(|p| Error::other(format!("path is not valid UTF-8: {}", p.display())))
}

/// Lexically normalizes a path: resolves `.` and `..` without touching the file system.
pub fn normalize(p: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for c in p.components() {
        match c {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                let popped = matches!(out.components().next_back(), Some(Utf8Component::Normal(_)));
                if popped {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_str()),
        }
    }
    out
}

/// Absolute, normalized form of `p` (relative paths are taken from `base`).
pub fn absolutize(p: &Utf8Path, base: &Utf8Path) -> Utf8PathBuf {
    if p.is_absolute() {
        normalize(p)
    } else {
        normalize(&base.join(p))
    }
}

/// Canonical path when the file exists (symlinks resolved), else the normalized path.
pub fn canonical_or_normal(p: &Utf8Path) -> Utf8PathBuf {
    std::fs::canonicalize(p).ok().and_then(|c| Utf8PathBuf::from_path_buf(c).ok()).unwrap_or_else(|| normalize(p))
}

/// True when `p` is `root` or inside it. Both are compared in canonical form when they exist.
pub fn is_inside(p: &Utf8Path, root: &Utf8Path) -> bool {
    let p = canonical_or_normal_deep(p);
    let root = canonical_or_normal(root);
    p.starts_with(&root)
}

/// Like `canonical_or_normal`, but for a path that doesn't exist yet it canonicalizes the
/// nearest existing ancestor, so `/tmp/x/new.txt` and `/private/tmp/x/new.txt` compare equal.
pub fn canonical_or_normal_deep(p: &Utf8Path) -> Utf8PathBuf {
    let p = normalize(p);
    let mut existing = p.as_path();
    let mut tail: Vec<&str> = Vec::new();
    loop {
        if existing.exists() {
            break;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                existing = parent;
            }
            _ => return p,
        }
    }
    let mut out = canonical_or_normal(existing);
    for name in tail.iter().rev() {
        out.push(name);
    }
    out
}

/// `target` relative to the folder `from`, with forward slashes. `None` when they share no root
/// (different Windows drives).
pub fn relative_to(target: &Utf8Path, from: &Utf8Path) -> Option<String> {
    let t = normalize(target);
    let f = normalize(from);
    if t.is_absolute() != f.is_absolute() {
        return None;
    }
    let tc: Vec<_> = t.components().collect();
    let fc: Vec<_> = f.components().collect();
    if let (Some(Utf8Component::Prefix(a)), Some(Utf8Component::Prefix(b))) = (tc.first(), fc.first()) {
        if a.as_str() != b.as_str() {
            return None;
        }
    }
    let common = tc.iter().zip(fc.iter()).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = Vec::new();
    for _ in common..fc.len() {
        parts.push("..");
    }
    for c in &tc[common..] {
        parts.push(c.as_str());
    }
    Some(if parts.is_empty() { ".".into() } else { parts.join("/") })
}

/// Writes a file atomically: a temporary file in the same folder, then a rename.
pub fn write_atomic(path: &Utf8Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let tmp = path.with_file_name(format!(".{}.{}.tmp", path.file_name().unwrap_or("file"), std::process::id()));
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

/// Pretty JSON with LF line endings and a trailing newline.
pub fn to_pretty_json<T: serde::Serialize>(v: &T) -> Result<String> {
    let mut s = serde_json::to_string_pretty(v)?;
    s.push('\n');
    Ok(s)
}

pub fn write_json_atomic<T: serde::Serialize>(path: &Utf8Path, v: &T) -> Result<()> {
    write_atomic(path, to_pretty_json(v)?.as_bytes())
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Utf8Path) -> Result<T> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::Io(std::io::Error::new(e.kind(), format!("{path}: {e}"))))?;
    serde_json::from_str(&text).map_err(|e| Error::InvalidProject(format!("{path}: {e}")))
}

/// Human-readable size.
pub fn format_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(relative_to("/a/b/footage/x.mov".into(), "/a/b/proj".into()).unwrap(), "../footage/x.mov");
        assert_eq!(relative_to("/a/b/x.mov".into(), "/a/b".into()).unwrap(), "x.mov");
        assert_eq!(relative_to("/a/b".into(), "/a/b".into()).unwrap(), ".");
        assert_eq!(normalize("/a/b/../c/./d".into()), Utf8PathBuf::from("/a/c/d"));
        assert_eq!(absolutize("../x".into(), "/a/b".into()), Utf8PathBuf::from("/a/x"));
    }

    #[test]
    fn sizes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(8_000_000), "8.0 MB");
    }
}
