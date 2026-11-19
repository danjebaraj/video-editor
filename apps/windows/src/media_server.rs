//! `https://media.video-studio.invalid/` (ui/spec.md §23.3): serves media thumbnails, players
//! and pasted images to a window's page, but only files that are in that window's project media
//! list or in its `.assets` sidecar. Byte ranges are honoured so video can seek.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Mutex, OnceLock};

use camino::{Utf8Path, Utf8PathBuf};
use webview2_com::CoTaskMemPWSTR;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Deferral, ICoreWebView2Environment, ICoreWebView2WebResourceRequestedEventArgs,
};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::UI::Shell::SHCreateMemStream;

use video_studio_core::paths;

#[derive(Default)]
struct Allowed {
    files: HashSet<String>,
    sidecar: Option<Utf8PathBuf>,
}

fn allowed() -> &'static Mutex<HashMap<u64, Allowed>> {
    static A: OnceLock<Mutex<HashMap<u64, Allowed>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The comparison key of a path: normalized, plain, lower case (Windows file names).
fn key(p: &Utf8Path) -> String {
    paths::normalize(p).as_str().to_lowercase()
}

pub fn set_allowed(window: u64, files: &[String], sidecar: Option<&str>) {
    let mut a = allowed().lock().unwrap();
    a.insert(
        window,
        Allowed {
            files: files.iter().map(|f| key(Utf8Path::new(f))).collect(),
            sidecar: sidecar.map(|s| paths::normalize(Utf8Path::new(s))),
        },
    );
}

/// Allows more files (a run's outputs and logs) without dropping the project's.
pub fn add_allowed(window: u64, files: &[String]) {
    let mut a = allowed().lock().unwrap();
    let w = a.entry(window).or_default();
    w.files.extend(files.iter().map(|f| key(Utf8Path::new(f))));
}

pub fn forget(window: u64) {
    allowed().lock().unwrap().remove(&window);
}

pub fn is_allowed(window: u64, path: &Utf8Path) -> bool {
    let a = allowed().lock().unwrap();
    let Some(w) = a.get(&window) else { return false };
    if w.files.contains(&key(path)) {
        return true;
    }
    match &w.sidecar {
        Some(dir) => {
            let p = paths::canonical_or_normal_deep(path);
            let d = paths::canonical_or_normal(dir);
            paths::starts_with_path(&p, &d) && p.components().count() > d.components().count()
        }
        None => false,
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).ok()?, 16).ok()?);
            i += 3;
        } else if b[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The URL the page uses for a media file.
pub fn url_for(path: &str) -> String {
    let name = Utf8Path::new(path).file_name().unwrap_or("file");
    format!("https://{}/file/{}?path={}", crate::webview::MEDIA_HOST, percent_encode(name), percent_encode(path))
}

fn path_from_url(url: &str) -> Option<Utf8PathBuf> {
    let q = url.split_once("?path=")?.1;
    let q = q.split(['&', '#']).next()?;
    Some(Utf8PathBuf::from(percent_decode(q)?))
}

pub fn content_type(p: &Utf8Path) -> &'static str {
    match p.extension().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("bmp") => "image/bmp",
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("m4a" | "aac") => "audio/mp4",
        Some("flac") => "audio/flac",
        Some("ogg" | "oga") => "audio/ogg",
        Some("json" | "json5") => "application/json",
        Some("srt" | "vtt" | "txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// `bytes=a-b` → (start, inclusive end) within a file of `len` bytes.
pub fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split(',').next()?.split_once('-')?;
    if len == 0 {
        return None;
    }
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (len.saturating_sub(n), len - 1)
        }
        (s, "") => (s.parse().ok()?, len - 1),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(len - 1)),
    };
    (start <= end && start < len).then_some((start, end))
}

const MAX_CHUNK: u64 = 8 * 1024 * 1024;

/// `thumb?path=…&n=…&frames=…&dur=…` → (path, frame index, frame count, duration).
fn thumb_request(url: &str) -> Option<(Utf8PathBuf, u32, u32, f64)> {
    let q = url.split_once("/thumb?")?.1;
    let mut path = None;
    let (mut n, mut frames, mut dur) = (0u32, 1u32, 0f64);
    for kv in q.split('&') {
        let (k, v) = kv.split_once('=')?;
        match k {
            "path" => path = percent_decode(v).map(Utf8PathBuf::from),
            "n" => n = v.parse().unwrap_or(0),
            "frames" => frames = v.parse().unwrap_or(1).max(1),
            "dur" => dur = v.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    Some((path?, n, frames, dur))
}

/// ffmpeg, as cutline installs it (`cutline deps path ffmpeg`), found once.
fn ffmpeg() -> Option<Utf8PathBuf> {
    static F: OnceLock<Option<Utf8PathBuf>> = OnceLock::new();
    F.get_or_init(|| {
        let s = video_studio_core::settings::Settings::load(&paths::library_dir());
        let c = video_studio_core::claude::locate::find_cutline(&s.cutline_path, &s.cutline_repo)?;
        let mut cmd = std::process::Command::new(&c.path);
        cmd.args(["deps", "path", "ffmpeg"]);
        let (code, out, _) =
            video_studio_core::platform::run_capture(&mut cmd, std::time::Duration::from_secs(20)).ok()?;
        let p = Utf8PathBuf::from(out.trim());
        (code == 0 && p.is_file()).then_some(p)
    })
    .clone()
}

/// cutline, as Settings finds it, found once.
fn cutline() -> Option<Utf8PathBuf> {
    static C: OnceLock<Option<Utf8PathBuf>> = OnceLock::new();
    C.get_or_init(|| {
        let s = video_studio_core::settings::Settings::load(&paths::library_dir());
        video_studio_core::claude::locate::find_cutline(&s.cutline_path, &s.cutline_repo)
            .map(|c| Utf8PathBuf::from(c.path))
    })
    .clone()
}

/// A HEIC/HEIF photo as a JPEG (WebView2 can't show HEIC): a viewable copy made with
/// `cutline still --max-side 1568`, cached by the file's quick hash. Err: why there is none.
fn heic_jpeg(path: &Utf8Path) -> Result<Vec<u8>, String> {
    let cutline = cutline().ok_or_else(|| "cutline isn't installed".to_string())?;
    let dir = paths::cache_root(&paths::library_dir()).join("heic");
    let jpg = video_studio_core::viewable::cached_copy(path, &dir, &cutline)?;
    std::fs::read(&jpg).map_err(|e| e.to_string())
}

/// The answer for a HEIC/HEIF: the JPEG, or 422 with the reason as text (the page shows it).
fn heic_response(path: &Utf8Path) -> (i32, &'static str, String, Vec<u8>) {
    match heic_jpeg(path) {
        Ok(b) => (200, "OK", format!("Content-Type: image/jpeg\r\nContent-Length: {}", b.len()), b),
        Err(why) => (422, "Unprocessable Content", "Content-Type: text/plain; charset=utf-8".into(), why.into_bytes()),
    }
}

/// A JPEG still of a video frame (cached), at most 640 px wide.
fn video_frame(path: &Utf8Path, n: u32, frames: u32, dur: f64) -> Option<Vec<u8>> {
    let fp = std::fs::metadata(path).ok()?;
    let mtime = fp.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{path}|{}|{mtime}|{n}|{frames}", fp.len()).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let cache = paths::cache_root(&paths::library_dir()).join("thumbs").join(format!("{h:016x}.jpg"));
    if let Ok(b) = std::fs::read(&cache) {
        return Some(b);
    }
    let t = if dur > 0.0 { dur * (n as f64 + 0.5) / frames as f64 } else { 0.0 };
    let mut cmd = std::process::Command::new(ffmpeg()?.as_str());
    cmd.args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{t:.3}"), "-i", path.as_str()])
        .args(["-frames:v", "1", "-vf", "scale='min(640,iw)':-2", "-q:v", "4", "-f", "image2", "-y"])
        .arg(cache.as_str());
    let _ = std::fs::create_dir_all(cache.parent()?);
    let (code, _, _) = video_studio_core::platform::run_capture(&mut cmd, std::time::Duration::from_secs(20)).ok()?;
    if code != 0 {
        return None;
    }
    std::fs::read(&cache).ok()
}

type Pending = (ICoreWebView2WebResourceRequestedEventArgs, ICoreWebView2Deferral, ICoreWebView2Environment);

thread_local! {
    static PENDING: std::cell::RefCell<HashMap<u64, Pending>> = std::cell::RefCell::new(HashMap::new());
}

fn complete(
    env: &ICoreWebView2Environment,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    r: (i32, &str, String, Vec<u8>),
) {
    let (status, reason, mut headers, body) = r;
    // The page (app host) reads logs and traces with fetch(): a different origin.
    let cors = format!("Access-Control-Allow-Origin: https://{}", crate::webview::APP_HOST);
    headers = if headers.is_empty() { cors } else { format!("{headers}\r\n{cors}") };
    unsafe {
        let stream = SHCreateMemStream(Some(&body));
        if let Ok(resp) =
            env.CreateWebResourceResponse(stream.as_ref(), status, &HSTRING::from(reason), &HSTRING::from(headers))
        {
            let _ = args.SetResponse(&resp);
        }
    }
}

pub fn respond(env: &ICoreWebView2Environment, window: u64, args: &ICoreWebView2WebResourceRequestedEventArgs) {
    unsafe {
        let Ok(request) = args.Request() else { return };
        let mut uri = PWSTR::null();
        if request.Uri(&mut uri).is_err() {
            return;
        }
        let uri = CoTaskMemPWSTR::from(uri).to_string();
        // Video stills and HEIC photos take a cutline/ffmpeg call: answer later, off the UI thread.
        let heic = path_from_url(&uri).filter(|p| video_studio_core::viewable::needs_copy(p));
        if let Some((path, n, frames, dur)) = thumb_request(&uri).or(heic.map(|p| (p, 0, 1, 0.0))) {
            if !is_allowed(window, &path) {
                complete(env, args, (403, "Forbidden", String::new(), Vec::new()));
                return;
            }
            let heic = video_studio_core::viewable::needs_copy(&path);
            if !heic && !content_type(&path).starts_with("video/") {
                complete(env, args, serve_file(&path, ""));
                return;
            }
            let Ok(deferral) = args.GetDeferral() else { return };
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let key = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            PENDING.with(|p| p.borrow_mut().insert(key, (args.clone(), deferral, env.clone())));
            std::thread::spawn(move || {
                let r = if heic {
                    heic_response(&path)
                } else {
                    match video_frame(&path, n, frames, dur) {
                        Some(b) => (200, "OK", format!("Content-Type: image/jpeg\r\nContent-Length: {}", b.len()), b),
                        None => (404, "Not Found", String::new(), Vec::new()),
                    }
                };
                crate::app::post(move || {
                    if let Some((args, deferral, env)) = PENDING.with(|p| p.borrow_mut().remove(&key)) {
                        complete(&env, &args, r);
                        let _ = deferral.Complete();
                    }
                });
            });
            return;
        }
        let mut range_header = String::new();
        if let Ok(headers) = request.Headers() {
            let mut v = PWSTR::null();
            if headers.GetHeader(&HSTRING::from("Range"), &mut v).is_ok() {
                range_header = CoTaskMemPWSTR::from(v).to_string();
            }
        }
        let r = match path_from_url(&uri) {
            Some(p) if is_allowed(window, &p) => serve_file(&p, &range_header),
            Some(_) => (403, "Forbidden", String::new(), b"not a file of this project".to_vec()),
            None => (404, "Not Found", String::new(), Vec::new()),
        };
        complete(env, args, r);
    }
}

fn serve_file(p: &Utf8Path, range: &str) -> (i32, &'static str, String, Vec<u8>) {
    let Ok(mut f) = std::fs::File::open(p) else {
        return (404, "Not Found", String::new(), Vec::new());
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let ct = content_type(p);
    let base = format!("Content-Type: {ct}\r\nAccept-Ranges: bytes\r\nCache-Control: no-cache");
    if !range.is_empty() {
        if let Some((start, end)) = parse_range(range, len) {
            let end = end.min(start + MAX_CHUNK - 1);
            let mut buf = vec![0u8; (end - start + 1) as usize];
            if f.seek(SeekFrom::Start(start)).is_ok() && f.read_exact(&mut buf).is_ok() {
                let h = format!("{base}\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {}", buf.len());
                return (206, "Partial Content", h, buf);
            }
        }
        return (416, "Range Not Satisfiable", format!("Content-Range: bytes */{len}"), Vec::new());
    }
    // Whole files only for things that aren't streamed (images, small files).
    if len > MAX_CHUNK && (ct.starts_with("video/") || ct.starts_with("audio/")) {
        let mut buf = vec![0u8; MAX_CHUNK as usize];
        if f.read_exact(&mut buf).is_ok() {
            let h =
                format!("{base}\r\nContent-Range: bytes 0-{}/{len}\r\nContent-Length: {}", MAX_CHUNK - 1, buf.len());
            return (206, "Partial Content", h, buf);
        }
    }
    let mut buf = Vec::with_capacity(len as usize);
    let _ = f.read_to_end(&mut buf);
    (200, "OK", format!("{base}\r\nContent-Length: {}", buf.len()), buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=0-5000", 1000), Some((0, 999)));
        assert_eq!(parse_range("bytes=2000-", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        // the URL the UI builds with encodeURIComponent for a video still
        let (p, n, frames, dur) = thumb_request(
            "https://media.video-studio.invalid/thumb?path=C%3A%5CMy%20Footage%5Ca%20(1).mp4&n=3&frames=12&dur=6.5",
        )
        .unwrap();
        assert_eq!((p.as_str(), n, frames, dur), (r"C:\My Footage\a (1).mp4", 3, 12, 6.5));
    }

    #[test]
    fn urls_round_trip_and_only_project_files_are_served() {
        let d = tempfile::tempdir().unwrap();
        let root = paths::canonical_or_normal(Utf8Path::from_path(d.path()).unwrap());
        let hero = root.join("My Footage").join("hero (1).mov");
        std::fs::create_dir_all(hero.parent().unwrap()).unwrap();
        std::fs::write(&hero, b"0123456789").unwrap();
        let sidecar = root.join("p.cutproj.assets");
        std::fs::create_dir_all(&sidecar).unwrap();
        std::fs::write(sidecar.join("abc.png"), b"png").unwrap();
        let url = url_for(hero.as_str());
        assert_eq!(path_from_url(&url).unwrap(), hero);
        set_allowed(7, &[hero.to_string()], Some(sidecar.as_str()));
        assert!(is_allowed(7, &hero));
        assert!(is_allowed(7, &Utf8PathBuf::from(hero.as_str().to_uppercase())), "case-insensitive");
        assert!(is_allowed(7, &sidecar.join("abc.png")));
        assert!(!is_allowed(7, &sidecar), "the folder itself isn't a file");
        assert!(!is_allowed(7, &sidecar.join("..").join("secret.txt")));
        assert!(!is_allowed(8, &hero), "another window's project");
        assert!(!is_allowed(7, &root.join("other.mov")));
        let (status, _, headers, body) = serve_file(&hero, "bytes=2-5");
        assert_eq!((status, body.as_slice()), (206, b"2345".as_slice()));
        assert!(headers.contains("Content-Range: bytes 2-5/10"));
        let (status, _, _, body) = serve_file(&hero, "");
        assert_eq!((status, body.len()), (200, 10));
        forget(7);
        assert!(!is_allowed(7, &hero));
    }
}
