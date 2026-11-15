//! Syncfusion Video Studio for Windows (ui/spec.md §23).
//!
//! A `windows-rs` shell: native windows, menus, dialogs, taskbar, toasts and single-instance
//! handling, with the window content in WebView2. The web UI talks to the shell through typed
//! JSON messages ([`bridge`]); the shell links `video-studio-core` and the same JSON surface as
//! the macOS shell's bindings (`video_studio_ffi::call`, `start_run`, `start_assistant`).

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod bridge;
#[cfg(windows)]
mod dialogs;
#[cfg(windows)]
mod instance;
#[cfg(windows)]
mod media_server;
#[cfg(windows)]
mod menu;
#[cfg(windows)]
mod selftest;
#[cfg(windows)]
mod selftest_fixture;
#[cfg(windows)]
mod shell;
#[cfg(windows)]
mod webview;
#[cfg(windows)]
mod window;

pub mod protocol;

#[cfg(windows)]
fn main() {
    std::process::exit(app::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Syncfusion Video Studio for Windows only runs on Windows; on macOS use apps/macos.");
    std::process::exit(1);
}
