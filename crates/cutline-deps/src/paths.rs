//! Platform directories. `CUTLINE_HOME` overrides everything (tools + cache live under it); a
//! `.cutline-home` with a `tools/` folder near the working directory or the binary is used next.

use std::path::{Path, PathBuf};

/// Where the cutline home came from (reported by `doctor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeSource {
    /// `CUTLINE_HOME`
    Env,
    /// a `.cutline-home` above the working directory
    WorkingDir,
    /// a `.cutline-home` above the cutline binary (e.g. a repo checkout's `target/release`)
    Binary,
    /// the platform data directory
    Platform,
}

/// Folder of the running binary with symlinks resolved, so a link such as
/// `~/.local/bin/cutline → <repo>/bin/cutline` finds the assets and `.cutline-home` of its target.
pub fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let real = std::fs::canonicalize(&exe).unwrap_or(exe);
    real.parent().map(Path::to_path_buf)
}

/// Nearest `.cutline-home` containing `tools/`, searching each start directory and its
/// ancestors in order.
pub fn find_home_from(starts: &[&Path]) -> Option<PathBuf> {
    starts.iter().find_map(|s| s.ancestors().map(|a| a.join(".cutline-home")).find(|h| h.join("tools").is_dir()))
}

/// The cutline home and how it was chosen: `CUTLINE_HOME`, else the nearest `.cutline-home`
/// (with a `tools/` folder) above the working directory, then above the binary, else the
/// platform data dir (`~/Library/Application Support/cutline`, `%LOCALAPPDATA%\cutline`,
/// `~/.local/share/cutline`).
pub fn home() -> (PathBuf, HomeSource) {
    if let Some(h) = std::env::var_os("CUTLINE_HOME") {
        return (PathBuf::from(h), HomeSource::Env);
    }
    if let Some(h) = std::env::current_dir().ok().and_then(|d| find_home_from(&[&d])) {
        return (h, HomeSource::WorkingDir);
    }
    if let Some(h) = exe_dir().and_then(|d| find_home_from(&[&d])) {
        return (h, HomeSource::Binary);
    }
    let platform = directories::ProjectDirs::from("", "", "cutline")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".cutline-home"));
    (platform, HomeSource::Platform)
}

/// Root of cutline's per-user data (see `home`).
pub fn data_dir() -> PathBuf {
    home().0
}

/// Directory holding downloaded tools (ffmpeg, models, chrome).
pub fn tools_dir() -> PathBuf {
    data_dir().join("tools")
}

/// Cache directory (analysis, transcripts, segment mezzanines live in project `.cutline/` instead).
pub fn cache_dir() -> PathBuf {
    let (h, src) = home();
    if src != HomeSource::Platform {
        return h.join("cache");
    }
    directories::ProjectDirs::from("", "", "cutline")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".cutline-cache"))
}

/// Executable file name for the current OS (`ffmpeg` → `ffmpeg.exe` on Windows).
pub fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// Platform identifier used in the lockfile, e.g. `macos-arm64`, `windows-x86_64`.
pub fn platform_id() -> String {
    let os = std::env::consts::OS;
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        a => a,
    };
    format!("{os}-{arch}")
}

/// Bundled assets directory (fonts, LUTs, templates, brand). `CUTLINE_ASSETS` wins, then
/// `<exe_dir>/assets`, `<exe_dir>/../share/cutline/assets`, then the source tree (dev builds).
pub fn assets_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("CUTLINE_ASSETS") {
        return PathBuf::from(p);
    }
    if let Some(d) = exe_dir() {
        for c in [d.join("assets"), d.join("../share/cutline/assets")] {
            if c.join("fonts").is_dir() {
                return c;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// Locate a Chrome/Chromium/Edge executable for headless rendering (the first of
/// [`chrome_candidates`]).
pub fn find_chrome() -> Option<PathBuf> {
    chrome_candidates().into_iter().next()
}

/// Every usable Chrome, in preference order: `CUTLINE_CHROME` (exclusively, when set), the
/// installed headless shell, then system Chrome/Chromium/Edge. The renderer falls back along
/// this list when a browser launches but never produces frames.
pub fn chrome_candidates() -> Vec<PathBuf> {
    if let Some(p) = std::env::var_os("CUTLINE_CHROME") {
        return vec![PathBuf::from(p)];
    }
    let mut cands: Vec<PathBuf> = vec![];
    let tools = tools_dir().join("chrome");
    if let Ok(cur) = std::fs::read_to_string(tools.join("current")) {
        let d = tools.join(cur.trim());
        for n in ["chrome-headless-shell", "chrome-headless-shell.exe"] {
            if let Some(p) = find_named(&d, n, 4) {
                cands.push(p);
                break;
            }
        }
    }
    if cfg!(target_os = "macos") {
        cands.push("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into());
        cands.push("/Applications/Chromium.app/Contents/MacOS/Chromium".into());
        cands.push("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into());
    } else if cfg!(windows) {
        for base in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(b) = std::env::var_os(base) {
                let b = PathBuf::from(b);
                cands.push(b.join("Google/Chrome/Application/chrome.exe"));
                cands.push(b.join("Microsoft/Edge/Application/msedge.exe"));
            }
        }
    }
    for n in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome", "msedge"] {
        if let Ok(p) = which::which(n) {
            cands.push(p);
        }
    }
    let mut out: Vec<PathBuf> = vec![];
    for p in cands {
        if p.exists() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

pub fn find_named(dir: &std::path::Path, name: &str, depth: u32) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.file_name().and_then(|n| n.to_str()) == Some(name) && p.is_file() {
            return Some(p);
        }
        if p.is_dir() {
            if let Some(f) = find_named(&p, name, depth - 1) {
                return Some(f);
            }
        }
    }
    None
}

#[cfg(test)]
mod home_tests {
    use super::find_home_from;

    #[test]
    fn nearest_home_with_tools_wins() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        let bin = repo.join("target").join("release");
        std::fs::create_dir_all(&bin).unwrap();
        // a .cutline-home without tools/ does not count
        std::fs::create_dir_all(bin.join(".cutline-home")).unwrap();
        assert_eq!(find_home_from(&[&bin]), None);
        std::fs::create_dir_all(repo.join(".cutline-home").join("tools")).unwrap();
        assert_eq!(find_home_from(&[&bin]), Some(repo.join(".cutline-home")));
        // earlier starts (the working directory) are searched first
        let other = d.path().join("other");
        std::fs::create_dir_all(other.join(".cutline-home").join("tools")).unwrap();
        let project = other.join("videos").join("demo");
        std::fs::create_dir_all(&project).unwrap();
        assert_eq!(find_home_from(&[&project, &bin]), Some(other.join(".cutline-home")));
    }
}
