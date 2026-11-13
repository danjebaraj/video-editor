//! Platform directories. `CUTLINE_HOME` overrides everything (tools + cache live under it).

use std::path::PathBuf;

/// Root of cutline's per-user data. `CUTLINE_HOME` wins; otherwise the platform data dir
/// (`~/Library/Application Support/cutline`, `%LOCALAPPDATA%\cutline`, `~/.local/share/cutline`).
pub fn data_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("CUTLINE_HOME") {
        return PathBuf::from(h);
    }
    directories::ProjectDirs::from("", "", "cutline")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".cutline-home"))
}

/// Directory holding downloaded tools (ffmpeg, models, chrome).
pub fn tools_dir() -> PathBuf {
    data_dir().join("tools")
}

/// Cache directory (analysis, transcripts, segment mezzanines live in project `.cutline/` instead).
pub fn cache_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("CUTLINE_HOME") {
        return PathBuf::from(h).join("cache");
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
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            for c in [d.join("assets"), d.join("../share/cutline/assets")] {
                if c.join("fonts").is_dir() {
                    return c;
                }
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
