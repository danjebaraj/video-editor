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
            return if rest.as_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.as_str().replace('\\', "/"))
            };
        }
    }
    p.to_string()
}

/// The app library: brands, output presets, settings, caches (spec §8).
/// `VIDEO_STUDIO_HOME` overrides the location (tests, portable setups).
pub fn library_dir() -> Utf8PathBuf {
    if let Ok(p) = std::env::var("VIDEO_STUDIO_HOME") {
        if !p.is_empty() {
            return plain(Utf8Path::new(&p));
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

/// The default parent of run folders: `~/Movies/Syncfusion Video Studio/` on macOS, the Videos
/// known folder on Windows (it can be redirected, so it isn't assumed to be `~\Videos`).
pub fn default_output_root() -> Utf8PathBuf {
    #[cfg(windows)]
    if let Some(videos) = crate::platform::windows::known_folder_videos() {
        return videos.join(APP_NAME);
    }
    let home = home_dir().unwrap_or_else(|| Utf8PathBuf::from("."));
    let movies = if cfg!(windows) { "Videos" } else { "Movies" };
    home.join(movies).join(APP_NAME)
}

/// Where caches live for a library: `<library>/cache` on macOS and in tests, and
/// `%LOCALAPPDATA%\Syncfusion Video Studio\Cache` for the default library on Windows, so roaming
/// profiles don't carry them (ui/spec.md §23.5).
pub fn cache_root(library: &Utf8Path) -> Utf8PathBuf {
    #[cfg(windows)]
    {
        let overridden = std::env::var("VIDEO_STUDIO_HOME").map(|v| !v.is_empty()).unwrap_or(false);
        if !overridden && same_path(library, &library_dir()) {
            if let Some(local) = std::env::var("LOCALAPPDATA").ok().filter(|s| !s.is_empty()) {
                return Utf8PathBuf::from(local).join(APP_NAME).join("Cache");
            }
        }
    }
    library.join("cache")
}

/// `p` without a Windows verbatim prefix (`\\?\C:\x` → `C:\x`, `\\?\UNC\srv\share` →
/// `\\srv\share`). `canonicalize` returns verbatim paths on Windows; they name the same file as
/// the plain form but compare, print and pass to other programs differently. Long paths keep
/// working without the prefix: the standard library adds it itself when a path needs it.
pub fn plain(p: &Utf8Path) -> Utf8PathBuf {
    let s = p.as_str();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return Utf8PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => Utf8PathBuf::from(rest),
        _ => p.to_owned(),
    }
}

/// A path as Git Bash writes it, in Windows form: `/c/Users/a` → `C:\Users\a`. Claude Code runs
/// its shell tool through Git Bash on Windows, so paths in commands can take either form.
/// Other platforms (and other paths) are returned unchanged.
pub fn from_shell_path(p: &str) -> String {
    if cfg!(windows) {
        let b = p.as_bytes();
        if b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && (b.len() == 2 || b[2] == b'/') {
            let drive = (b[1] as char).to_ascii_uppercase();
            let rest = if b.len() > 3 { &p[3..] } else { "" };
            return format!("{drive}:\\{}", rest.replace('/', "\\"));
        }
    }
    p.to_string()
}

/// Whether two path components are the same: exact on Unix, case-insensitive on Windows (where
/// `C:\Runs\x` and `c:/runs/X/` are one folder).
fn same_component(a: &Utf8Component, b: &Utf8Component) -> bool {
    if cfg!(windows) {
        a.as_str().to_lowercase() == b.as_str().to_lowercase()
    } else {
        a == b
    }
}

/// `p` starts with `root`, component by component (case-insensitive on Windows).
pub fn starts_with_path(p: &Utf8Path, root: &Utf8Path) -> bool {
    let mut pc = p.components();
    for r in root.components() {
        match pc.next() {
            Some(c) if same_component(&c, &r) => {}
            _ => return false,
        }
    }
    true
}

/// `p` relative to `root` when `p` is inside it (components compared as [`starts_with_path`]).
pub fn strip_path_prefix(p: &Utf8Path, root: &Utf8Path) -> Option<Utf8PathBuf> {
    if !starts_with_path(p, root) {
        return None;
    }
    let skip = root.components().count();
    Some(p.components().skip(skip).map(|c| c.as_str()).collect())
}

/// Whether two paths name the same location after normalizing (case-insensitive on Windows).
pub fn same_path(a: &Utf8Path, b: &Utf8Path) -> bool {
    let (a, b) = (plain(&normalize(a)), plain(&normalize(b)));
    a.components().count() == b.components().count() && starts_with_path(&a, &b)
}

pub fn utf8(p: std::path::PathBuf) -> Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(p).map_err(|p| Error::other(format!("path is not valid UTF-8: {}", p.display())))
}

/// Lexically normalizes a path: resolves `.` and `..` without touching the file system, and on
/// Windows drops a verbatim `\\?\` prefix (see [`plain`]).
pub fn normalize(p: &Utf8Path) -> Utf8PathBuf {
    let p = &plain(p);
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
    std::fs::canonicalize(p)
        .ok()
        .and_then(|c| Utf8PathBuf::from_path_buf(c).ok())
        .map(|c| plain(&c))
        .unwrap_or_else(|| normalize(p))
}

/// True when `p` is `root` or inside it. Both are compared in canonical form when they exist,
/// case-insensitively on Windows.
pub fn is_inside(p: &Utf8Path, root: &Utf8Path) -> bool {
    let p = canonical_or_normal_deep(p);
    let root = canonical_or_normal(root);
    starts_with_path(&p, &root)
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
    if let (Some(a @ Utf8Component::Prefix(_)), Some(b @ Utf8Component::Prefix(_))) = (tc.first(), fc.first()) {
        if !same_component(a, b) {
            return None;
        }
    }
    let common = tc.iter().zip(fc.iter()).take_while(|(a, b)| same_component(a, b)).count();
    let mut parts: Vec<&str> = vec![".."; fc.len() - common];
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
    // On Windows a reader (the CLI, the app, a virus scanner) holding the target open briefly
    // makes the replace fail with a sharing violation: retry for a moment before giving up.
    #[cfg(windows)]
    for _ in 0..20 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32) => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            Err(_) => break,
        }
    }
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
    let text =
        std::fs::read_to_string(path).map_err(|e| Error::Io(std::io::Error::new(e.kind(), format!("{path}: {e}"))))?;
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

    #[cfg(windows)]
    #[test]
    fn windows_paths() {
        // case-insensitive, either separator, trailing separator ignored
        assert!(starts_with_path(&normalize("c:/runs/X/".into()), &normalize(r"C:\Runs\x".into())));
        assert!(same_path(r"C:\Runs\x".into(), "c:/runs/X/".into()));
        assert!(!same_path(r"C:\Runs\x".into(), r"C:\Runs\xy".into()));
        assert!(!starts_with_path(r"C:\Runs\xy\a".into(), r"C:\Runs\x".into()));
        assert!(!same_path(r"C:\Runs".into(), r"D:\Runs".into()));
        // verbatim forms are the same place
        assert_eq!(plain(r"\\?\C:\Runs\x".into()), Utf8PathBuf::from(r"C:\Runs\x"));
        assert_eq!(plain(r"\\?\UNC\server\share\f".into()), Utf8PathBuf::from(r"\\server\share\f"));
        assert!(same_path(r"\\?\C:\Runs\x".into(), r"c:\runs\x".into()));
        // UNC shares
        assert!(starts_with_path(r"\\server\share\Footage\a.mov".into(), r"\\SERVER\share".into()));
        assert_eq!(relative_to(r"\\srv\s\footage\x.mov".into(), r"\\srv\s\proj".into()).unwrap(), "../footage/x.mov");
        // relative paths across drives don't exist; across case they do
        assert_eq!(relative_to(r"D:\a\x.mov".into(), r"C:\a".into()), None);
        assert_eq!(relative_to(r"C:\A\footage\x.mov".into(), r"c:\a\proj".into()).unwrap(), "../footage/x.mov");
        // Git Bash forms
        assert_eq!(from_shell_path("/c/Users/a b/x.mp4"), r"C:\Users\a b\x.mp4");
        assert_eq!(from_shell_path("/d"), r"D:\");
        assert_eq!(from_shell_path("/tmp/x"), "/tmp/x");
        assert_eq!(from_shell_path("outputs/x.mp4"), "outputs/x.mp4");
    }

    #[cfg(windows)]
    #[test]
    fn windows_long_paths_and_inside_checks() {
        let d = tempfile::tempdir().unwrap();
        let root = canonical_or_normal(Utf8Path::from_path(d.path()).unwrap());
        assert!(!root.as_str().starts_with(r"\\?\"), "canonical paths are plain: {root}");
        // a folder well past MAX_PATH
        let mut deep = root.clone();
        while deep.as_str().len() < 400 {
            deep.push("a-rather-long-folder-name-for-testing");
        }
        std::fs::create_dir_all(&deep).unwrap();
        let f = deep.join("file.txt");
        write_atomic(&f, b"x\n").unwrap();
        write_atomic(&f, b"y\n").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"y\n");
        assert!(is_inside(&f, &root));
        let upper = Utf8PathBuf::from(f.as_str().to_uppercase().replace('\\', "/"));
        assert!(is_inside(&upper, &root), "case and separators don't matter");
        assert!(!is_inside(&root.join("..").join("elsewhere"), &root));
    }

    #[test]
    fn sizes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(8_000_000), "8.0 MB");
    }
}
