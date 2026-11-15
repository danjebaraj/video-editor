//! Shell services for the bridge: theme, taskbar progress, file watching, recent files, the
//! bundled helper (for the ElevenLabs key), notifications, Reveal and Open With.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};
use windows::core::HSTRING;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::Shell::{
    ITaskbarList3, SHAddToRecentDocs, TaskbarList, SHARD_PATHW, TBPF_ERROR, TBPF_INDETERMINATE, TBPF_NOPROGRESS,
    TBPF_NORMAL, TBPF_PAUSED,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

use video_studio_core::paths;
use video_studio_core::platform::{OpenWith, RevealInFileManager};

use crate::protocol::{ProgressState, ThemeInfo, Topic};

// MARK: Theme

fn reg_dword(key: &str, value: &str) -> Option<u32> {
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(value),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut _ as *mut _),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    Some(data)
}

/// The system's light or dark app theme, high contrast, accent color and the monitor scale.
pub fn theme(scale: f64) -> ThemeInfo {
    let light =
        reg_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme").unwrap_or(1);
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    let high_contrast = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some(&mut hc as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
            && hc.dwFlags.contains(HCF_HIGHCONTRASTON)
    };
    // AccentColor is 0xAABBGGRR.
    let accent = reg_dword(r"Software\Microsoft\Windows\DWM", "AccentColor")
        .map(|c| format!("#{:02x}{:02x}{:02x}", c & 0xff, (c >> 8) & 0xff, (c >> 16) & 0xff))
        .unwrap_or_else(|| "#0067c0".into());
    // Mica needs transparency effects on (Settings › Personalization › Colors) and isn't drawn
    // in high contrast; the self test's snapshots can't capture it either.
    let transparency =
        reg_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "EnableTransparency").unwrap_or(1);
    let mica = transparency != 0 && !high_contrast && !crate::app::options().selftest;
    ThemeInfo { dark: light == 0, high_contrast, accent, scale, mica }
}

// MARK: Taskbar

pub fn set_progress(hwnd: HWND, state: ProgressState, value: Option<f64>) {
    thread_local! {
        static TASKBAR: Option<ITaskbarList3> = unsafe {
            CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok()
                .and_then(|t| t.HrInit().ok().map(|_| t))
        };
    }
    TASKBAR.with(|t| {
        let Some(t) = t else { return };
        unsafe {
            let flag = match state {
                ProgressState::None => TBPF_NOPROGRESS,
                ProgressState::Normal => TBPF_NORMAL,
                ProgressState::Indeterminate => TBPF_INDETERMINATE,
                ProgressState::Error => TBPF_ERROR,
                ProgressState::Paused => TBPF_PAUSED,
            };
            let _ = t.SetProgressState(hwnd, flag);
            if let (Some(v), ProgressState::Normal | ProgressState::Error | ProgressState::Paused) = (value, state) {
                let _ = t.SetProgressValue(hwnd, (v.clamp(0.0, 1.0) * 1000.0) as u64, 1000);
            }
        }
    });
}

// MARK: Files and links

pub fn open_url(url: &str) {
    let _ = video_studio_core::platform::windows::WinShell.open(Utf8Path::new(url), None);
}

pub fn reveal(path: &str) -> Result<(), String> {
    video_studio_core::platform::windows::WinShell.reveal(Utf8Path::new(path)).map_err(|e| e.to_string())
}

pub fn open(path: &str, application: Option<&str>) -> Result<(), String> {
    video_studio_core::platform::windows::WinShell.open(Utf8Path::new(path), application).map_err(|e| e.to_string())
}

pub fn notify(title: &str, body: &str) {
    let _ = video_studio_core::platform::windows::toast(title, body);
}

// MARK: Recent files

fn recent_file() -> Utf8PathBuf {
    paths::library_dir().join("recent-projects.json")
}

pub fn recent() -> Vec<String> {
    let list: Vec<String> = paths::read_json(&recent_file()).unwrap_or_default();
    list.into_iter().filter(|p| Utf8Path::new(p).is_file()).collect()
}

/// Remembers a project in the start window's list and in the taskbar jump list.
pub fn add_recent(path: &str) {
    let mut list: Vec<String> = paths::read_json(&recent_file()).unwrap_or_default();
    list.retain(|p| !paths::same_path(Utf8Path::new(p), Utf8Path::new(path)));
    list.insert(0, path.to_string());
    list.truncate(12);
    let _ = paths::write_json_atomic(&recent_file(), &list);
    let wide = HSTRING::from(path);
    unsafe { SHAddToRecentDocs(SHARD_PATHW.0 as u32, Some(wide.as_ptr() as *const _)) };
}

// MARK: File watching

struct Watch {
    path: Utf8PathBuf,
    stamp: Option<(SystemTime, u64)>,
}

fn watches() -> &'static Mutex<HashMap<u64, Watch>> {
    static W: OnceLock<Mutex<HashMap<u64, Watch>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(HashMap::new()))
}

fn stamp(p: &Utf8Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

pub fn watch(window: u64, path: &str) {
    let p = Utf8PathBuf::from(path);
    let s = stamp(&p);
    watches().lock().unwrap().insert(window, Watch { path: p, stamp: s });
}

pub fn clear_window(window: u64) {
    watches().lock().unwrap().remove(&window);
    crate::media_server::forget(window);
    crate::bridge::forget_window(window);
}

/// Polls watched project files once a second (the CLI may append runs while a project is open).
pub fn start_file_watcher() {
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_millis(1000));
        let mut changed: Vec<(u64, String)> = Vec::new();
        {
            let mut w = watches().lock().unwrap();
            for (id, watch) in w.iter_mut() {
                let now = stamp(&watch.path);
                if now.is_some() && now != watch.stamp {
                    watch.stamp = now;
                    changed.push((*id, watch.path.to_string()));
                }
            }
        }
        for (id, path) in changed {
            crate::app::send_event(id, Topic::FileChanged, json!({"path": path}));
        }
    });
}

// MARK: Recycle Bin

/// Moves one pasted image to the Recycle Bin. Only regular files directly inside a
/// `<name>.cutproj.assets` sidecar qualify (Clean Up Pasted Images, spec §5); true when moved.
pub fn recycle_sidecar_file(path: &str) -> bool {
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
    };
    let p = paths::normalize(Utf8Path::new(path));
    let in_sidecar = p
        .parent()
        .and_then(|d| d.file_name())
        .map(|n| n.to_ascii_lowercase().ends_with(".cutproj.assets"))
        .unwrap_or(false);
    if !in_sidecar || !p.is_file() {
        return false;
    }
    // double-NUL-terminated list of one path
    let mut from: Vec<u16> = p.as_str().encode_utf16().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    unsafe { SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool() && !p.exists() }
}

// MARK: The helper

/// The bundled `video-studio.exe`: next to the app (installed and built alike).
/// `VIDEO_STUDIO_HELPER` overrides it.
pub fn helper_path() -> Utf8PathBuf {
    if let Some(h) = std::env::var("VIDEO_STUDIO_HELPER").ok().filter(|s| !s.is_empty()) {
        return Utf8PathBuf::from(h);
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("video-studio.exe")))
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("video-studio.exe"))
}

/// `video-studio secret <action> --json`, with the key on stdin for `set`. The app never reads
/// the key itself (docs/decisions.md).
pub fn secret(action: &str, value: Option<&str>) -> Result<Value, String> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(helper_path().as_str());
    cmd.args(["secret", action, "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start the helper: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(value.unwrap_or("").as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(text.trim()).map_err(|_| {
        let err = String::from_utf8_lossy(&out.stderr);
        format!("the helper failed: {}", err.trim())
    })?;
    if v["ok"] == false {
        return Err(v["error"].as_str().unwrap_or("the helper failed").to_string());
    }
    Ok(v)
}
