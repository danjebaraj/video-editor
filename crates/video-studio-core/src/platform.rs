//! Platform traits (spec §3). Each has a macOS implementation now and a Windows stub or
//! cross-platform fallback. Mac-only APIs that need Foundation (bookmarks, notifications, Finder)
//! are implemented by the Swift shell and passed in through these traits.

use std::process::{Child, Command};
use std::sync::Mutex;

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Error, Result, BUNDLE_ID};

/// Secret storage: Keychain on macOS, Credential Manager on Windows.
pub trait Secrets: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>>;
    fn set(&self, name: &str, value: &str) -> Result<()>;
    fn delete(&self, name: &str) -> Result<()>;
}

/// Name of the ElevenLabs API key entry.
pub const ELEVEN_KEY: &str = "elevenlabs-api-key";

/// The OS secret store through the `keyring` crate.
pub struct OsSecrets;

impl Secrets for OsSecrets {
    fn get(&self, name: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(BUNDLE_ID, name).map_err(|e| Error::other(format!("secret store: {e}")))?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::other(format!("secret store: {e}"))),
        }
    }
    fn set(&self, name: &str, value: &str) -> Result<()> {
        let entry = keyring::Entry::new(BUNDLE_ID, name).map_err(|e| Error::other(format!("secret store: {e}")))?;
        entry.set_password(value).map_err(|e| Error::other(format!("secret store: {e}")))
    }
    fn delete(&self, name: &str) -> Result<()> {
        let entry = keyring::Entry::new(BUNDLE_ID, name).map_err(|e| Error::other(format!("secret store: {e}")))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::other(format!("secret store: {e}"))),
        }
    }
}

/// In-memory secrets for tests and for runs against a fake server.
#[derive(Default)]
pub struct MemorySecrets(Mutex<std::collections::HashMap<String, String>>);

impl MemorySecrets {
    pub fn with(name: &str, value: &str) -> Self {
        let s = MemorySecrets::default();
        s.0.lock().unwrap().insert(name.into(), value.into());
        s
    }
}

impl Secrets for MemorySecrets {
    fn get(&self, name: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
    fn set(&self, name: &str, value: &str) -> Result<()> {
        self.0.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<()> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

/// Durable file references: security-scoped bookmarks on macOS, plain paths plus a file ID on
/// Windows. The value is an opaque base64 string stored in the project.
pub trait FileRefs: Send + Sync {
    /// A reference for an existing file, or `None` when the platform has none.
    fn create(&self, path: &Utf8Path) -> Option<String>;
    /// The file's current path, if the reference still resolves.
    fn resolve(&self, reference: &str) -> Option<Utf8PathBuf>;
}

/// No durable references: resolution falls back to relative path, absolute path and search roots.
pub struct NoFileRefs;

impl FileRefs for NoFileRefs {
    fn create(&self, _path: &Utf8Path) -> Option<String> {
        None
    }
    fn resolve(&self, _reference: &str) -> Option<Utf8PathBuf> {
        None
    }
}

/// User notifications (run finished, question waiting).
pub trait Notifications: Send + Sync {
    fn notify(&self, title: &str, body: &str);
}

pub struct NoNotifications;
impl Notifications for NoNotifications {
    fn notify(&self, _title: &str, _body: &str) {}
}

/// Reveal a file in Finder / Explorer.
pub trait RevealInFileManager: Send + Sync {
    fn reveal(&self, path: &Utf8Path) -> Result<()>;
}

/// Open a file with the default or a chosen application.
pub trait OpenWith: Send + Sync {
    fn open(&self, path: &Utf8Path, application: Option<&str>) -> Result<()>;
}

/// Cross-platform implementation using the OS command-line openers.
pub struct SystemOpener;

impl RevealInFileManager for SystemOpener {
    fn reveal(&self, path: &Utf8Path) -> Result<()> {
        let status = if cfg!(target_os = "macos") {
            Command::new("open").arg("-R").arg(path.as_str()).status()
        } else if cfg!(windows) {
            Command::new("explorer").arg(format!("/select,{path}")).status()
        } else {
            Command::new("xdg-open").arg(path.parent().unwrap_or(path).as_str()).status()
        };
        status.map(|_| ()).map_err(Error::from)
    }
}

impl OpenWith for SystemOpener {
    fn open(&self, path: &Utf8Path, application: Option<&str>) -> Result<()> {
        let mut cmd = if cfg!(target_os = "macos") {
            let mut c = Command::new("open");
            if let Some(app) = application {
                c.arg("-a").arg(app);
            }
            c.arg(path.as_str());
            c
        } else if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "start", ""]).arg(path.as_str());
            c
        } else {
            let mut c = Command::new("xdg-open");
            c.arg(path.as_str());
            c
        };
        cmd.status().map(|_| ()).map_err(Error::from)
    }
}

/// Spawning and killing a process tree: a process group on Unix, a Job Object on Windows.
pub struct ProcessGroup;

impl ProcessGroup {
    /// Makes `cmd` the leader of a new process group so the whole tree can be signalled.
    pub fn configure(cmd: &mut Command) {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NEW_PROCESS_GROUP: the tree is killed with `taskkill /T` below.
            // A Job Object (windows-rs) replaces this in the Windows shell milestone (S10).
            cmd.creation_flags(0x0000_0200);
        }
        #[cfg(not(any(unix, windows)))]
        let _ = cmd;
    }

    /// Asks the tree to stop (SIGTERM / taskkill).
    pub fn terminate(child: &Child) {
        #[cfg(unix)]
        unsafe {
            libc::killpg(child.id() as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill").args(["/T", "/PID", &child.id().to_string()]).output();
        }
        #[cfg(not(any(unix, windows)))]
        let _ = child;
    }

    /// Kills the whole tree now (Claude, cutline, ffmpeg, Chrome).
    pub fn kill(child: &mut Child) {
        #[cfg(unix)]
        unsafe {
            libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &child.id().to_string()]).output();
        }
        let _ = child.kill();
    }

    /// Kills a process group by its leader's PID (used when only the PID is known).
    pub fn kill_pid(pid: u32) {
        #[cfg(unix)]
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).output();
        }
        #[cfg(not(any(unix, windows)))]
        let _ = pid;
    }
}

/// Runs a command to completion with a timeout, capturing stdout and stderr as text.
pub fn run_capture(cmd: &mut Command, timeout: std::time::Duration) -> Result<(i32, String, String)> {
    use std::io::Read;
    use std::process::Stdio;
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    ProcessGroup::configure(cmd);
    let mut child = cmd.spawn()?;
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out.read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let t_err = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = err.read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(s) => break s,
            None if std::time::Instant::now() >= deadline => {
                ProcessGroup::kill(&mut child);
                let _ = child.wait();
                return Err(Error::other("the command timed out"));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(15)),
        }
    };
    Ok((status.code().unwrap_or(-1), t_out.join().unwrap_or_default(), t_err.join().unwrap_or_default()))
}

/// Free space on the volume holding `path` (or its nearest existing ancestor).
pub fn free_space(path: &Utf8Path) -> Option<u64> {
    let mut p = path;
    loop {
        if p.exists() {
            return fs2::available_space(p).ok();
        }
        p = p.parent()?;
    }
}

/// True when a folder can be created and written at `path`.
pub fn is_writable_dir(path: &Utf8Path) -> bool {
    let mut p = path;
    loop {
        if p.is_dir() {
            let probe = p.join(format!(".vs-write-test-{}", std::process::id()));
            let ok = std::fs::write(&probe, b"").is_ok();
            let _ = std::fs::remove_file(&probe);
            return ok;
        }
        match p.parent() {
            Some(parent) if !parent.as_str().is_empty() => p = parent,
            _ => return false,
        }
    }
}
