//! Single instance (ui/spec.md §23.3): a named mutex says whether the app is already running,
//! and a named pipe carries a second launch's file-open request to it. Both names include the
//! user's session, so two users on one machine each get their own app.

use std::io::Read;

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING,
    PIPE_ACCESS_INBOUND,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::CreateMutexW;

fn suffix() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let session = std::env::var("SESSIONNAME").unwrap_or_default();
    // Test runs (and the self test) use their own library: keep them apart from a real instance.
    let home = std::env::var("VIDEO_STUDIO_HOME").unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{user}|{session}|{home}").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn pipe_name() -> String {
    format!(r"\\.\pipe\SyncfusionVideoStudio-{}", suffix())
}

/// Held for the life of the first instance.
pub struct Instance(HANDLE);

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// `Ok(instance)` for the first instance; `Err(())` when another is running (the caller forwards
/// its files with [`forward`] and exits).
pub fn acquire() -> Result<Instance, ()> {
    let name = HSTRING::from(format!("Local\\SyncfusionVideoStudio-{}", suffix()));
    unsafe {
        let h = CreateMutexW(None, true, &name).map_err(|_| ())?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            return Err(());
        }
        Ok(Instance(h))
    }
}

/// Sends the paths (one per line; an empty message just brings the app forward) to the running
/// instance. True when it got them.
pub fn forward(paths: &[String]) -> bool {
    let name = HSTRING::from(pipe_name());
    for _ in 0..50 {
        let h = unsafe {
            CreateFileW(&name, FILE_GENERIC_WRITE.0, FILE_SHARE_NONE, None, OPEN_EXISTING, Default::default(), None)
        };
        if let Ok(h) = h {
            let msg = paths.join("\n");
            let ok = unsafe { WriteFile(h, Some(msg.as_bytes()), None, None).is_ok() };
            unsafe {
                let _ = FlushFileBuffers(h);
                let _ = CloseHandle(h);
            }
            return ok;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

/// Serves the pipe on a background thread, calling `on_paths` for every request.
pub fn serve(on_paths: impl Fn(Vec<String>) + Send + 'static) {
    std::thread::spawn(move || {
        let name = HSTRING::from(pipe_name());
        loop {
            let pipe = unsafe {
                CreateNamedPipeW(
                    &name,
                    PIPE_ACCESS_INBOUND,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    0,
                    64 * 1024,
                    0,
                    None,
                )
            };
            if pipe.is_invalid() {
                return;
            }
            let connected = unsafe { ConnectNamedPipe(pipe, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED };
            if connected {
                let mut reader = PipeReader(pipe);
                let mut text = String::new();
                let _ = reader.read_to_string(&mut text);
                let paths: Vec<String> =
                    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
                on_paths(paths);
            }
            unsafe {
                let _ = DisconnectNamedPipe(pipe);
                let _ = CloseHandle(pipe);
            }
        }
    });
}

struct PipeReader(HANDLE);

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut n = 0u32;
        match unsafe { ReadFile(self.0, Some(buf), Some(&mut n), None) } {
            Ok(()) => Ok(n as usize),
            // the writer closed its end: end of message
            Err(_) => Ok(0),
        }
    }
}
