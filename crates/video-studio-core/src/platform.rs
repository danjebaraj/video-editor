//! Platform traits (spec §3). Each has a macOS and a Windows implementation. Mac-only APIs that
//! need Foundation (bookmarks, notifications, Finder) are implemented by the Swift shell and
//! passed in through these traits; the Windows ones live in [`windows`] (ui/spec.md §23.4).

use std::process::{Child, Command};
use std::sync::Mutex;

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Error, Result, BUNDLE_ID};

#[cfg(windows)]
pub mod windows;

/// The secret store's service and account for a secret. macOS keeps the bundle ID and the
/// secret's name; Windows uses the service and account names the spec gives for Credential
/// Manager (`Syncfusion Video Studio` / `elevenlabs`).
fn secret_entry(name: &str) -> Result<keyring::Entry> {
    let (service, account) = if cfg!(windows) {
        (
            crate::APP_NAME,
            match name {
                ELEVEN_KEY => "elevenlabs",
                CARTESIA_KEY => "cartesia",
                _ => name,
            },
        )
    } else {
        (BUNDLE_ID, name)
    };
    keyring::Entry::new(service, account).map_err(|e| Error::other(format!("secret store: {e}")))
}

/// Secret storage: Keychain on macOS, Credential Manager on Windows.
pub trait Secrets: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>>;
    fn set(&self, name: &str, value: &str) -> Result<()>;
    fn delete(&self, name: &str) -> Result<()>;
}

/// Name of the ElevenLabs API key entry.
pub const ELEVEN_KEY: &str = "elevenlabs-api-key";
/// Name of the Cartesia API key entry.
pub const CARTESIA_KEY: &str = "cartesia-api-key";

/// The OS secret store through the `keyring` crate.
pub struct OsSecrets;

impl Secrets for OsSecrets {
    fn get(&self, name: &str) -> Result<Option<String>> {
        let entry = secret_entry(name)?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::other(format!("secret store: {e}"))),
        }
    }
    fn set(&self, name: &str, value: &str) -> Result<()> {
        let entry = secret_entry(name)?;
        entry.set_password(value).map_err(|e| Error::other(format!("secret store: {e}")))
    }
    fn delete(&self, name: &str) -> Result<()> {
        let entry = secret_entry(name)?;
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

/// This platform's own file references for code that has no shell to supply them (the CLI, the
/// Windows shell): file IDs on Windows, none elsewhere (macOS bookmarks come from the Swift shell).
pub fn system_file_refs() -> Box<dyn FileRefs> {
    #[cfg(windows)]
    {
        Box::new(windows::WinFileRefs)
    }
    #[cfg(not(windows))]
    {
        Box::new(NoFileRefs)
    }
}

/// The operating system's version as it names itself ("15.2", "10.0.26100"), for the run log.
pub fn os_version() -> Option<String> {
    let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("sw_vers", &["-productVersion"])
    } else if cfg!(windows) {
        ("cmd", &["/C", "ver"])
    } else {
        ("uname", &["-r"])
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        // the app has no console: without this a console window flashes up at each run's start
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let text = if cfg!(windows) { windows_version_number(&text) } else { text };
    (out.status.success() && !text.is_empty()).then_some(text)
}

/// `ver` prints "Microsoft Windows [Version 10.0.26200.6584]"; the run log wants the number.
fn windows_version_number(ver: &str) -> String {
    match (ver.rfind(' '), ver.rfind(']')) {
        (Some(a), Some(b)) if a < b => ver[a + 1..b].to_string(),
        _ => ver.to_string(),
    }
}

#[cfg(test)]
#[test]
fn the_windows_version_number_is_read_from_ver() {
    assert_eq!(windows_version_number("Microsoft Windows [Version 10.0.26200.6584]"), "10.0.26200.6584");
    assert_eq!(windows_version_number("something else"), "something else");
    #[cfg(windows)]
    assert!(os_version().is_some_and(|v| v.starts_with("10.") && !v.contains(' ')), "{:?}", os_version());
}

/// BUG-amber-walrus-72VP: a tree started from a thread that blocks `SIGCHLD` (as Swift's
/// cooperative pool does) starts with nothing blocked, so Codex hears its commands exit.
#[cfg(all(test, unix))]
#[test]
fn a_process_tree_starts_with_no_signals_blocked() {
    let perl = "/usr/bin/perl";
    if !Utf8Path::new(perl).exists() {
        eprintln!("skipped: no {perl}");
        return;
    }
    let script = "use POSIX; my $s = POSIX::SigSet->new; sigprocmask(SIG_BLOCK, POSIX::SigSet->new, $s); \
                  print $s->ismember(SIGCHLD) ? qq(blocked) : qq(clear)";
    let mask = |group: bool| {
        std::thread::spawn(move || {
            // SAFETY: changes only this test thread's own mask.
            unsafe {
                let mut chld: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut chld);
                libc::sigaddset(&mut chld, libc::SIGCHLD);
                libc::pthread_sigmask(libc::SIG_BLOCK, &chld, std::ptr::null_mut());
            }
            let mut cmd = Command::new(perl);
            cmd.args(["-e", script]).stdout(std::process::Stdio::piped());
            let child = if group { ProcessGroup::spawn(&mut cmd) } else { cmd.spawn() }.unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
        })
        .join()
        .unwrap()
    };
    // the control shows the mask really is inherited without the reset
    assert_eq!(mask(false), "blocked");
    assert_eq!(mask(true), "clear");
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
        #[cfg(windows)]
        return windows::WinShell.reveal(path);
        #[allow(unreachable_code)]
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
        #[cfg(windows)]
        return windows::WinShell.open(path, application);
        #[allow(unreachable_code)]
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

/// Clears the calling thread's signal mask and restores `SIGCHLD`'s default action, so a child
/// doesn't inherit the blocked or ignored signals of the thread that started it.
#[cfg(unix)]
fn reset_signals() {
    // SAFETY: plain libc calls on a local, initialized signal set.
    unsafe {
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
        libc::signal(libc::SIGCHLD, libc::SIG_DFL);
    }
}

/// Spawning and killing a process tree: a process group on Unix, a Job Object on Windows.
pub struct ProcessGroup;

impl ProcessGroup {
    /// Makes `cmd` the leader of a new process group so the whole tree can be signalled. On
    /// Windows the child is also created suspended: start it with [`ProcessGroup::spawn`], which
    /// puts it in its Job Object and resumes it.
    ///
    /// On Unix the child also starts with no signals blocked and `SIGCHLD` at its default
    /// (BUG-amber-walrus-72VP). A signal mask and ignored signals survive `exec`, and the macOS
    /// app calls the core from Swift's cooperative pool, whose threads block every signal. Codex
    /// started from there inherited a blocked `SIGCHLD`, never learned that a command or patch
    /// had exited (it waits for `SIGCHLD`), and left each one a zombie "still running".
    pub fn configure(cmd: &mut Command) {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
            // SAFETY: runs in the child between fork and exec, and calls only sigemptyset,
            // sigprocmask and signal, which are async-signal-safe.
            unsafe {
                cmd.pre_exec(|| {
                    reset_signals();
                    Ok(())
                });
            }
        }
        #[cfg(windows)]
        windows::configure(cmd);
        #[cfg(not(any(unix, windows)))]
        let _ = cmd;
    }

    /// Starts `cmd` as the root of its own tree: a new process group on Unix; on Windows also a
    /// kill-on-close Job Object, joined before the child runs, so nothing it starts escapes.
    pub fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
        Self::configure(cmd);
        let child = cmd.spawn()?;
        #[cfg(windows)]
        windows::adopt(&child)?;
        Ok(child)
    }

    /// Asks the tree to stop (SIGTERM on Unix, Ctrl-Break to the process group on Windows).
    pub fn terminate(child: &Child) {
        #[cfg(unix)]
        unsafe {
            libc::killpg(child.id() as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(windows)]
        windows::ctrl_break(child.id());
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
        windows::terminate_tree(child.id());
        let _ = child.kill();
    }

    /// Kills a process tree by its root's PID (used when only the PID is known). On Windows
    /// only trees started by [`ProcessGroup::spawn`] are touched: a PID alone may since have
    /// been reused by an unrelated process.
    pub fn kill_pid(pid: u32) {
        #[cfg(unix)]
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
        #[cfg(windows)]
        windows::terminate_tree(pid);
        #[cfg(not(any(unix, windows)))]
        let _ = pid;
    }

    /// The tree's root has been reaped: forget it. On Windows this closes its job, which ends
    /// anything it left running.
    pub fn release(pid: u32) {
        #[cfg(windows)]
        windows::release(pid);
        #[cfg(not(windows))]
        let _ = pid;
    }
}

/// A command that runs `exe` directly. On Windows an npm `.cmd` shim (`claude.cmd`) is resolved
/// to the script it starts, so arguments aren't parsed again by `cmd.exe`.
pub fn command(exe: &Utf8Path) -> Command {
    #[cfg(windows)]
    {
        windows::command_for(exe)
    }
    #[cfg(not(windows))]
    {
        Command::new(exe.as_str())
    }
}

/// Runs a command to completion with a timeout, capturing stdout and stderr as text.
pub fn run_capture(cmd: &mut Command, timeout: std::time::Duration) -> Result<(i32, String, String)> {
    use std::io::Read;
    use std::process::Stdio;
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = ProcessGroup::spawn(cmd)?;
    let pid = child.id();
    // Whatever the command leaves running when it ends (or times out) goes with it.
    struct Release(u32);
    impl Drop for Release {
        fn drop(&mut self) {
            ProcessGroup::release(self.0);
        }
    }
    let _release = Release(pid);
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
    // Ends anything the command left holding its output pipes, so the readers can finish.
    ProcessGroup::release(pid);
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
