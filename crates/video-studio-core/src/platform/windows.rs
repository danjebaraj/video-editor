//! Windows implementations of the platform traits (ui/spec.md §23.4).
//!
//! - **Process trees** run in a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. A child is
//!   created suspended, put in its job, then resumed, so nothing it starts can escape before it
//!   is in the job. Stopping sends `CTRL_BREAK_EVENT` to the tree's process group; killing
//!   terminates the job. When the app exits or crashes, the OS closes the job handles and the
//!   trees go with them.
//! - **File references** are `<volume serial>:<128-bit file ID>`, resolved with `OpenFileById`,
//!   so a media file is found again after a rename or a move on the same volume.
//! - **Locations** come from known folders (Videos can be redirected).
//! - **Reveal / Open With** use the shell; **notifications** are toasts.

use std::collections::HashMap;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::sync::{Mutex, OnceLock};

use camino::{Utf8Path, Utf8PathBuf};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ExtendedFileIdType, FileIdInfo, GetDriveTypeW, GetFileInformationByHandleEx,
    GetFinalPathNameByHandleW, GetLogicalDrives, OpenFileById, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_128,
    FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_ID_INFO, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GETFINALPATHNAMEBYHANDLE_FLAGS, OPEN_EXISTING,
    VOLUME_NAME_DOS,
};
use windows::Win32::System::Console::{
    AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, GetConsoleCP, SetConsoleCtrlHandler, CTRL_BREAK_EVENT,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

use super::{FileRefs, Notifications, OpenWith, RevealInFileManager};
use crate::{Error, Result};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_SUSPENDED: u32 = 0x0000_0004;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_REMOVABLE: u32 = 2;

// MARK: Process trees

/// Job handles by the PID of the process each job was made for.
fn jobs() -> &'static Mutex<HashMap<u32, OwnedHandle>> {
    static JOBS: OnceLock<Mutex<HashMap<u32, OwnedHandle>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether this process is attached to a console (the CLI is, the app isn't). A pseudo-console
/// (Windows Terminal, an IDE) has no window, so the console's code page is what tells.
fn has_console() -> bool {
    unsafe { GetConsoleCP() != 0 }
}

/// Creation flags for a tree's root: its own process group (for Ctrl-Break), created suspended
/// (so it is in its job before it runs), and without a console window when the caller is the
/// app (a GUI process would otherwise flash one up for every console child).
pub fn configure(cmd: &mut Command) {
    let mut flags = CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED;
    if !has_console() {
        flags |= CREATE_NO_WINDOW;
    }
    cmd.creation_flags(flags);
}

/// Puts a child started by [`configure`] in a new kill-on-close job, then resumes it. The child
/// is always resumed, even if the job can't be made (it then runs without one).
pub fn adopt(child: &Child) -> std::io::Result<()> {
    let pid = child.id();
    let assigned = unsafe { make_job(child.as_raw_handle()) };
    let resumed = resume_threads(pid);
    match assigned {
        Ok(job) => {
            jobs().lock().unwrap().insert(pid, job);
        }
        Err(e) => {
            crate::claude::broker::append_line(
                &crate::paths::library_dir().join("logs/process.log"),
                &format!("{}: couldn't put process {pid} in a job: {e}", crate::timeutil::now_rfc3339()),
            );
        }
    }
    resumed
}

unsafe fn make_job(process: RawHandle) -> windows::core::Result<OwnedHandle> {
    let job = CreateJobObjectW(None, PCWSTR::null())?;
    let owned = OwnedHandle::from_raw_handle(job.0 as RawHandle);
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    SetInformationJobObject(
        job,
        JobObjectExtendedLimitInformation,
        &info as *const _ as *const _,
        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
    )?;
    AssignProcessToJobObject(job, HANDLE(process as _))?;
    Ok(owned)
}

/// Resumes every thread of a process created suspended.
fn resume_threads(pid: u32) -> std::io::Result<()> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).map_err(std::io::Error::other)?;
        let mut entry = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut resumed = 0;
        let mut ok = Thread32First(snap, &mut entry).is_ok();
        while ok {
            if entry.th32OwnerProcessID == pid {
                if let Ok(t) = OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) {
                    ResumeThread(t);
                    let _ = CloseHandle(t);
                    resumed += 1;
                }
            }
            ok = Thread32Next(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
        if resumed == 0 {
            return Err(std::io::Error::other(format!("process {pid} has no thread to resume")));
        }
    }
    Ok(())
}

/// Serializes console attach and detach: a process has one console at a time.
fn console_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Sends `CTRL_BREAK_EVENT` to the process group led by `pid` (graceful stop, spec §12.4).
/// A console process (the CLI) shares its console with the tree and signals it directly; the app
/// has none, so it attaches to the tree's hidden console for the call.
pub fn ctrl_break(pid: u32) -> bool {
    let _guard = console_lock().lock().unwrap();
    unsafe {
        if has_console() {
            return GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid).is_ok();
        }
        if AttachConsole(pid).is_err() {
            return false;
        }
        // Don't let the event stop this process too while it is attached.
        let _ = SetConsoleCtrlHandler(None, true);
        let sent = GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid).is_ok();
        let _ = FreeConsole();
        let _ = SetConsoleCtrlHandler(None, false);
        sent
    }
}

/// Terminates every process in the tree started as `pid`. False when there is no job for it.
pub fn terminate_tree(pid: u32) -> bool {
    let jobs = jobs().lock().unwrap();
    match jobs.get(&pid) {
        Some(job) => unsafe { TerminateJobObject(HANDLE(job.as_raw_handle() as _), 1).is_ok() },
        None => false,
    }
}

/// Forgets the tree's job. Closing its last handle ends whatever is still running in it.
pub fn release(pid: u32) {
    jobs().lock().unwrap().remove(&pid);
}

/// Whether a job is still held for `pid` (tests).
pub fn has_job(pid: u32) -> bool {
    jobs().lock().unwrap().contains_key(&pid)
}

/// A command for `exe` that doesn't go through `cmd.exe`: an npm `.cmd` shim is resolved to the
/// Node script (or program) it starts, so arguments aren't parsed again by `cmd.exe`
/// (ui/spec.md §23.5). Anything else is run as it is.
pub fn command_for(exe: &Utf8Path) -> Command {
    let lower = exe.as_str().to_ascii_lowercase();
    if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        if let Some((program, args)) = resolve_cmd_shim(exe) {
            let mut c = Command::new(program.as_str());
            c.args(args);
            return c;
        }
    }
    Command::new(exe.as_str())
}

/// What an npm-style `.cmd` shim runs: `(node, [script])` or `(program, [])`.
pub fn resolve_cmd_shim(shim: &Utf8Path) -> Option<(Utf8PathBuf, Vec<String>)> {
    let text = std::fs::read_to_string(shim).ok()?;
    let dir = shim.parent()?.to_owned();
    // Quoted targets relative to the shim's folder: "%dp0%\node_modules\…\cli.js" or "%~dp0\…".
    let mut target: Option<Utf8PathBuf> = None;
    for piece in text.split('"') {
        let p = piece.trim();
        let rest = p
            .strip_prefix("%dp0%\\")
            .or_else(|| p.strip_prefix("%~dp0\\"))
            .or_else(|| p.strip_prefix("%dp0%/"))
            .or_else(|| p.strip_prefix("%~dp0/"));
        let Some(rest) = rest else { continue };
        let lower = rest.to_ascii_lowercase();
        if lower.ends_with(".js") || lower.ends_with(".cjs") || lower.ends_with(".mjs") || lower.ends_with(".exe") {
            if lower.ends_with("node.exe") {
                continue;
            }
            target = Some(dir.join(rest.replace('/', "\\")));
        }
    }
    let target = target.filter(|t| t.is_file())?;
    if target.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false) {
        return Some((target, Vec::new()));
    }
    let local_node = dir.join("node.exe");
    let node = if local_node.is_file() {
        local_node
    } else {
        let path = std::env::var("PATH").ok()?;
        crate::claude::locate::which_in("node", &path)?
    };
    Some((node, vec![target.to_string()]))
}

// MARK: File references

/// Durable references made from the volume serial number and the 128-bit file ID.
pub struct WinFileRefs;

fn open_for_attributes(path: &str, directory: bool) -> Option<HANDLE> {
    let flags = if directory { FILE_FLAG_BACKUP_SEMANTICS } else { Default::default() };
    unsafe {
        CreateFileW(
            &HSTRING::from(path),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            flags,
            None,
        )
        .ok()
    }
}

fn file_id_info(h: HANDLE) -> Option<FILE_ID_INFO> {
    let mut info = FILE_ID_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            h,
            FileIdInfo,
            &mut info as *mut _ as *mut _,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
        .ok()?;
    }
    Some(info)
}

fn final_path(h: HANDLE) -> Option<Utf8PathBuf> {
    let mut buf = vec![0u16; 1024];
    loop {
        // FILE_NAME_NORMALIZED and VOLUME_NAME_DOS are both 0: a normalized `C:\…` path.
        let n = unsafe {
            GetFinalPathNameByHandleW(
                h,
                &mut buf,
                GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0),
            )
        } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            let s = String::from_utf16(&buf[..n]).ok()?;
            return Some(crate::paths::plain(Utf8Path::new(&s)));
        }
        buf.resize(n + 1, 0);
    }
}

/// `<volume serial, 16 hex>:<file id, 32 hex>`.
pub fn format_file_id(serial: u64, id: &[u8; 16]) -> String {
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    format!("{serial:016x}:{hex}")
}

pub fn parse_file_id(r: &str) -> Option<(u64, [u8; 16])> {
    let (serial, id) = r.trim().split_once(':')?;
    let serial = u64::from_str_radix(serial, 16).ok()?;
    if id.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&id[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some((serial, out))
}

impl FileRefs for WinFileRefs {
    fn create(&self, path: &Utf8Path) -> Option<String> {
        let h = open_for_attributes(path.as_str(), path.is_dir())?;
        let info = file_id_info(h);
        unsafe {
            let _ = CloseHandle(h);
        }
        let info = info?;
        Some(format_file_id(info.VolumeSerialNumber, &info.FileId.Identifier))
    }

    fn resolve(&self, reference: &str) -> Option<Utf8PathBuf> {
        let (serial, id) = parse_file_id(reference)?;
        let drives = unsafe { GetLogicalDrives() };
        for i in 0..26u32 {
            if drives & (1 << i) == 0 {
                continue;
            }
            let root = format!("{}:\\", (b'A' + i as u8) as char);
            let kind = unsafe { GetDriveTypeW(&HSTRING::from(root.as_str())) };
            if ![DRIVE_FIXED, DRIVE_REMOVABLE, DRIVE_REMOTE].contains(&kind) {
                continue;
            }
            let Some(vol) = open_for_attributes(&root, true) else { continue };
            let same_volume = file_id_info(vol).map(|v| v.VolumeSerialNumber == serial).unwrap_or(false);
            let found = if same_volume {
                let desc = FILE_ID_DESCRIPTOR {
                    dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32,
                    Type: ExtendedFileIdType,
                    Anonymous: FILE_ID_DESCRIPTOR_0 { ExtendedFileId: FILE_ID_128 { Identifier: id } },
                };
                unsafe {
                    OpenFileById(
                        vol,
                        &desc,
                        FILE_READ_ATTRIBUTES.0,
                        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                        None,
                        FILE_FLAG_BACKUP_SEMANTICS,
                    )
                    .ok()
                }
                .and_then(|h| {
                    let p = final_path(h);
                    unsafe {
                        let _ = CloseHandle(h);
                    }
                    p
                })
            } else {
                None
            };
            unsafe {
                let _ = CloseHandle(vol);
            }
            if found.is_some() {
                return found;
            }
            if same_volume {
                // The volume is right but the file is gone (deleted, or moved to another volume).
                return None;
            }
        }
        None
    }
}

// MARK: Locations

/// The Videos known folder (`FOLDERID_Videos`), which can be redirected.
pub fn known_folder_videos() -> Option<Utf8PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Videos, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty()).map(Utf8PathBuf::from)
    }
}

// MARK: Reveal, Open With

/// Explorer through the shell: `SHOpenFolderAndSelectItems`, and `ShellExecuteExW` /
/// `SHOpenWithDialog` for opening.
pub struct WinShell;

fn ensure_com() {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    // Fails harmlessly when the thread already has COM in another mode.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

impl RevealInFileManager for WinShell {
    fn reveal(&self, path: &Utf8Path) -> Result<()> {
        use windows::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};
        ensure_com();
        let target = crate::paths::plain(path);
        unsafe {
            let pidl = ILCreateFromPathW(&HSTRING::from(target.as_str()));
            if pidl.is_null() {
                return Err(Error::NotFound(format!("{path} doesn't exist")));
            }
            let r = SHOpenFolderAndSelectItems(pidl, None, 0);
            ILFree(Some(pidl));
            r.map_err(|e| Error::other(format!("couldn't show {path} in File Explorer: {e}")))
        }
    }
}

impl OpenWith for WinShell {
    /// `None` opens with the default app; `Some("")` shows the "Open with" dialog; `Some(app)`
    /// opens the file with that program.
    fn open(&self, path: &Utf8Path, application: Option<&str>) -> Result<()> {
        use windows::Win32::UI::Shell::{
            SHOpenWithDialog, ShellExecuteExW, OAIF_EXEC, OAIF_HIDE_REGISTRATION, OPENASINFO, SEE_MASK_NOASYNC,
            SHELLEXECUTEINFOW,
        };
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        ensure_com();
        let file = HSTRING::from(crate::paths::plain(path).as_str());
        unsafe {
            match application {
                Some(app) if app.trim().is_empty() => {
                    let info = OPENASINFO {
                        pcszFile: PCWSTR(file.as_ptr()),
                        pcszClass: PCWSTR::null(),
                        oaifInFlags: OAIF_EXEC | OAIF_HIDE_REGISTRATION,
                    };
                    SHOpenWithDialog(None, &info).map_err(|e| Error::other(format!("Open With: {e}")))
                }
                other => {
                    let verb = HSTRING::from("open");
                    let (target, params) = match other {
                        Some(app) => (HSTRING::from(app), Some(HSTRING::from(format!("\"{}\"", path)))),
                        None => (file.clone(), None),
                    };
                    let mut sei = SHELLEXECUTEINFOW {
                        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                        fMask: SEE_MASK_NOASYNC,
                        lpVerb: PCWSTR(verb.as_ptr()),
                        lpFile: PCWSTR(target.as_ptr()),
                        lpParameters: params.as_ref().map(|p| PCWSTR(p.as_ptr())).unwrap_or(PCWSTR::null()),
                        nShow: SW_SHOWNORMAL.0,
                        ..Default::default()
                    };
                    ShellExecuteExW(&mut sei).map_err(|e| Error::other(format!("couldn't open {path}: {e}")))
                }
            }
        }
    }
}

// MARK: Notifications

/// The app's AUMID when it runs unpackaged (the MSIX package gives it an identity of its own).
pub const APP_USER_MODEL_ID: &str = "Syncfusion.VideoStudio";

/// Toast notifications. Packaged, they use the package identity; unpackaged, the AUMID above,
/// which Windows only shows once a Start menu shortcut carries it. Failures are silent: a
/// notification that can't be shown must never stop a run.
pub struct ToastNotifications;

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn toast(title: &str, body: &str) -> windows::core::Result<()> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_escape(title),
        xml_escape(body)
    );
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    let notifier = ToastNotificationManager::CreateToastNotifier()
        .or_else(|_| ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_USER_MODEL_ID)))?;
    notifier.Show(&toast)
}

impl Notifications for ToastNotifications {
    fn notify(&self, title: &str, body: &str) {
        let _ = toast(title, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::ProcessGroup;
    use std::time::{Duration, Instant};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    /// (pid, parent pid) of every process.
    fn processes() -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).unwrap();
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                out.push((e.th32ProcessID, e.th32ParentProcessID));
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        out
    }

    fn descendants(root: u32) -> Vec<u32> {
        let all = processes();
        let mut found = vec![root];
        let mut i = 0;
        while i < found.len() {
            let p = found[i];
            found.extend(all.iter().filter(|(_, parent)| *parent == p).map(|(pid, _)| *pid));
            i += 1;
        }
        found.remove(0);
        found
    }

    fn alive(pid: u32) -> bool {
        processes().iter().any(|(p, _)| *p == pid)
    }

    fn system32() -> String {
        format!("{}\\System32", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
    }

    fn wait_until(what: impl Fn() -> bool, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if what() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        what()
    }

    /// Run by the tests below as a child: sleeps, so only a signal or a kill ends it early.
    #[test]
    #[ignore]
    fn helper_sleeps_when_asked() {
        if std::env::var_os("VS_TEST_SLEEP").is_some() {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    #[test]
    fn job_object_kills_the_whole_tree_including_grandchildren() {
        // cmd → cmd → ping: a child and a grandchild (full paths: PATH may lack System32)
        let sys = system32();
        let mut cmd = Command::new(format!("{sys}\\cmd.exe"));
        cmd.args(["/d", "/c", &format!("{sys}\\cmd.exe /d /c {sys}\\PING.EXE -n 60 127.0.0.1")]);
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let mut child = ProcessGroup::spawn(&mut cmd).unwrap();
        let pid = child.id();
        assert!(has_job(pid));
        let started = wait_until(|| descendants(pid).len() >= 2, 10);
        assert!(started, "the tree started: {:?} (root exited: {:?})", descendants(pid), child.try_wait());
        let tree = descendants(pid);
        ProcessGroup::kill(&mut child);
        let _ = child.wait();
        assert!(wait_until(|| tree.iter().all(|p| !alive(*p)), 10), "every grandchild is gone: {tree:?}");
        ProcessGroup::release(pid);
        assert!(!has_job(pid));
    }

    #[test]
    fn releasing_the_job_ends_what_the_root_left_running() {
        // the root exits at once and leaves a background ping behind
        let sys = system32();
        let mut cmd = Command::new(format!("{sys}\\cmd.exe"));
        cmd.args(["/d", "/c", &format!("start /b {sys}\\PING.EXE -n 60 127.0.0.1 >nul")]);
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let mut child = ProcessGroup::spawn(&mut cmd).unwrap();
        let pid = child.id();
        assert!(wait_until(|| !descendants(pid).is_empty(), 10));
        let orphans = descendants(pid);
        let _ = child.wait();
        assert!(orphans.iter().all(|p| alive(*p)), "the orphan outlives its parent until the job closes");
        ProcessGroup::release(pid);
        assert!(wait_until(|| orphans.iter().all(|p| !alive(*p)), 10), "closing the job ended {orphans:?}");
    }

    #[test]
    fn ctrl_break_stops_the_tree_gracefully() {
        let exe = std::env::current_exe().unwrap();
        let mut cmd = Command::new(exe);
        cmd.args(["--ignored", "--exact", "platform::windows::tests::helper_sleeps_when_asked", "--nocapture"]);
        cmd.env("VS_TEST_SLEEP", "1");
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let mut child = ProcessGroup::spawn(&mut cmd).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let started = Instant::now();
        ProcessGroup::terminate(&child);
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "Ctrl-Break didn't stop the child");
            std::thread::sleep(Duration::from_millis(25));
        };
        // the default handler ends the process with STATUS_CONTROL_C_EXIT
        assert_eq!(status.code(), Some(0xC000013Au32 as i32), "{status:?}");
        ProcessGroup::release(child.id());
    }

    #[test]
    fn processes_start_running_after_adoption() {
        // created suspended: if the threads weren't resumed this would never finish
        let mut cmd = Command::new("cmd");
        cmd.args(["/d", "/c", "exit 7"]);
        let mut child = ProcessGroup::spawn(&mut cmd).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            assert!(Instant::now() < deadline, "the child never ran");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(status.code(), Some(7));
        ProcessGroup::release(child.id());
        // run_capture goes through the same path
        let (code, out, _) =
            crate::platform::run_capture(Command::new("cmd").args(["/d", "/c", "echo hi"]), Duration::from_secs(10))
                .unwrap();
        assert_eq!((code, out.trim()), (0, "hi"));
    }

    #[test]
    fn file_ids_round_trip_as_text() {
        let id: [u8; 16] = [0xde, 0xad, 0xbe, 0xef, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0xff];
        let s = format_file_id(0x1234_abcd, &id);
        assert_eq!(s, "000000001234abcd:deadbeef000102030405060708090aff");
        assert_eq!(parse_file_id(&s), Some((0x1234_abcd, id)));
        assert_eq!(parse_file_id("nonsense"), None);
        assert_eq!(parse_file_id("12:34"), None);
    }

    #[test]
    fn file_refs_follow_renames_and_moves_on_the_same_volume() {
        let d = tempfile::tempdir().unwrap();
        let root = crate::paths::canonical_or_normal(Utf8Path::from_path(d.path()).unwrap());
        let a = root.join("footage").join("hero.mov");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::write(&a, b"hero").unwrap();
        let r = WinFileRefs.create(&a).expect("a file ID on NTFS");
        assert_eq!(WinFileRefs.resolve(&r).map(|p| crate::paths::same_path(&p, &a)), Some(true));
        // renamed
        let b = root.join("footage").join("hero-final.mov");
        std::fs::rename(&a, &b).unwrap();
        assert!(crate::paths::same_path(&WinFileRefs.resolve(&r).unwrap(), &b));
        // moved to another folder on the same volume
        let c = root.join("archive").join("2026").join("hero-final.mov");
        std::fs::create_dir_all(c.parent().unwrap()).unwrap();
        std::fs::rename(&b, &c).unwrap();
        assert!(crate::paths::same_path(&WinFileRefs.resolve(&r).unwrap(), &c));
        // deleted: no answer, so resolution falls back to paths and search roots
        std::fs::remove_file(&c).unwrap();
        assert_eq!(WinFileRefs.resolve(&r), None);
        // a reference from another volume (or another machine) doesn't resolve here
        assert_eq!(WinFileRefs.resolve("ffffffffffffffff:00000000000000000000000000000001"), None);
    }

    #[test]
    fn videos_known_folder_exists() {
        let v = known_folder_videos().expect("the Videos known folder");
        assert!(v.is_absolute(), "{v}");
    }

    #[test]
    fn npm_cmd_shims_resolve_to_the_script() {
        let d = tempfile::tempdir().unwrap();
        let dir = crate::paths::canonical_or_normal(Utf8Path::from_path(d.path()).unwrap());
        let script = dir.join("node_modules/@anthropic-ai/claude-code/cli.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "console.log(1)").unwrap();
        std::fs::write(dir.join("node.exe"), b"").unwrap();
        // what npm writes for a package binary
        let shim = dir.join("claude.cmd");
        std::fs::write(
            &shim,
            "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n",
        )
        .unwrap();
        let (program, args) = resolve_cmd_shim(&shim).expect("resolved");
        assert!(crate::paths::same_path(&program, &dir.join("node.exe")), "{program}");
        assert_eq!(args.len(), 1);
        assert!(crate::paths::same_path(Utf8Path::new(&args[0]), &script), "{args:?}");
        // a shim for a native program runs the program itself
        let exe = dir.join("bin/tool.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        let shim2 = dir.join("tool.cmd");
        std::fs::write(&shim2, "@\"%~dp0\\bin\\tool.exe\" %*\r\n").unwrap();
        let (program, args) = resolve_cmd_shim(&shim2).unwrap();
        assert!(crate::paths::same_path(&program, &exe) && args.is_empty());
        // something that isn't a shim
        let other = dir.join("other.cmd");
        std::fs::write(&other, "@echo hi\r\n").unwrap();
        assert!(resolve_cmd_shim(&other).is_none());
    }
}
