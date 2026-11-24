//! `video-studio capture …`: the commands Claude runs during a production run to drive a
//! desktop app or a web app and record it (see `video_studio_core::capture`). Output is short
//! plain text Claude can act on.

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, Subcommand};

use video_studio_core::capture::{self, Frame, Pointer, Session};
use video_studio_core::claude::Mode;
use video_studio_core::eleven::{HelperContext, CONTEXT_ENV};
use video_studio_core::Error;

#[derive(Args, Clone)]
pub struct Target {
    /// The app to act on (default: the app opened last)
    #[arg(long, global = true)]
    app: Option<String>,
    /// Use the whole main display instead of the app's front window
    #[arg(long, global = true)]
    screen: bool,
}

impl Target {
    fn frame(&self) -> Frame {
        if self.screen {
            Frame::Screen
        } else {
            Frame::Window
        }
    }
}

#[derive(Subcommand)]
pub enum RecordCommand {
    /// Start recording the app's front window (or the screen) to captures/<out>.mov
    Start {
        #[arg(long)]
        out: String,
        /// How long to record, 1 to 600: every step you will perform plus a few seconds
        #[arg(long)]
        seconds: u32,
        /// Show a ring around each click
        #[arg(long)]
        clicks: bool,
    },
    /// Wait for the recording to reach its length; the file is complete after this
    Wait,
    /// Drop the running recording
    Cancel,
}

#[derive(Subcommand)]
pub enum CaptureCommand {
    /// Show whether screen recording and control of apps are allowed
    Check,
    /// Open an app by name ("Safari"), or an http(s) address in a browser
    Open {
        target: String,
        /// Browser to open the address in
        #[arg(long)]
        with: Option<String>,
    },
    /// Show the front window's size and position, or change them
    Window {
        /// WIDTHxHEIGHT in points, for example 1280x800
        #[arg(long)]
        size: Option<String>,
        /// X,Y of the top-left corner in points
        #[arg(long)]
        at: Option<String>,
    },
    /// Save a PNG of the app's front window (or the screen) to captures/<out>.png
    Screenshot {
        #[arg(long)]
        out: String,
    },
    /// Click at X Y, in the pixel coordinates of the latest screenshot
    Click {
        x: f64,
        y: f64,
        #[arg(long)]
        double: bool,
        #[arg(long)]
        right: bool,
    },
    /// Move the pointer to X Y
    Move { x: f64, y: f64 },
    /// Drag from X Y to TO_X TO_Y
    Drag { x: f64, y: f64, to_x: f64, to_y: f64 },
    /// Scroll at X Y by LINES (negative scrolls down)
    Scroll {
        x: f64,
        y: f64,
        #[arg(allow_hyphen_values = true)]
        lines: i32,
        /// Spread the scroll evenly over SECONDS (0.1 to 60) in small steps, for a smooth movement
        /// in a recording
        #[arg(long, value_name = "SECONDS")]
        over: Option<f64>,
    },
    /// Type text into the app, at a pace that reads well on video
    Type { text: String },
    /// Press a key or shortcut: return, tab, escape, cmd+n, cmd+shift+z, down
    Key {
        combo: String,
        #[arg(long, default_value_t = 1)]
        times: u32,
    },
    /// Choose a menu item: capture menu "File" "New Project…"
    Menu {
        #[arg(required = true, num_args = 2..)]
        path: Vec<String>,
    },
    /// Pause for SECONDS (at most 30), for example while a page loads
    Wait { seconds: f64 },
    /// Record the screen
    Record {
        #[command(subcommand)]
        command: RecordCommand,
    },
}

fn pair(s: &str, sep: char, what: &str) -> Result<(f64, f64), Error> {
    let parts: Vec<f64> = s.split([sep, sep.to_ascii_uppercase()]).filter_map(|p| p.trim().parse().ok()).collect();
    match parts.as_slice() {
        [a, b] => Ok((*a, *b)),
        _ => Err(Error::other(format!("{what} isn't valid: `{s}`."))),
    }
}

fn session() -> Result<(Session, Mode), Error> {
    let ctx = HelperContext::from_env()?;
    let settings = ctx.capture.clone().ok_or_else(|| {
        Error::Denied(
            "App recording is turned off for this project. The user can turn it on under Project › App recording."
                .into(),
        )
    })?;
    let context_file = std::env::var(CONTEXT_ENV).unwrap_or_default();
    let context_dir = Utf8Path::new(&context_file).parent().map(Utf8PathBuf::from).unwrap_or_default();
    Ok((
        Session {
            settings,
            run_folder: ctx.run_folder.clone(),
            state_file: capture::state_file(&context_dir),
            backend: capture::backend(),
        },
        ctx.mode,
    ))
}

fn execute(command: CaptureCommand, target: &Target) -> Result<String, Error> {
    let (s, mode) = session()?;
    let app = target.app.as_deref();
    let frame = target.frame();
    if mode == Mode::DryRun && !matches!(command, CaptureCommand::Check) {
        return Err(Error::Denied("Apps aren't driven or recorded in a dry run.".into()));
    }
    match command {
        CaptureCommand::Check => s.check(false),
        CaptureCommand::Open { target, with } => s.open(&target, with.as_deref()),
        CaptureCommand::Window { size, at } => {
            let size = size.as_deref().map(|v| pair(v, 'x', "--size (WIDTHxHEIGHT)")).transpose()?;
            let at = at.as_deref().map(|v| pair(v, ',', "--at (X,Y)")).transpose()?;
            s.window(app, size, at)
        }
        CaptureCommand::Screenshot { out } => s.screenshot(&out, app, frame),
        CaptureCommand::Click { x, y, double, right } => {
            s.pointer(Pointer::Click { x, y, right, count: if double { 2 } else { 1 } }, app, frame)
        }
        CaptureCommand::Move { x, y } => s.pointer(Pointer::Move { x, y }, app, frame),
        CaptureCommand::Drag { x, y, to_x, to_y } => s.pointer(Pointer::Drag { x, y, to_x, to_y }, app, frame),
        CaptureCommand::Scroll { x, y, lines, over } => s.pointer(Pointer::Scroll { x, y, lines, over }, app, frame),
        CaptureCommand::Type { text } => s.type_text(&text, app),
        CaptureCommand::Key { combo, times } => s.key(&combo, times, app),
        CaptureCommand::Menu { path } => s.menu(&path, app),
        CaptureCommand::Wait { seconds } => s.wait(seconds),
        CaptureCommand::Record { command: RecordCommand::Start { out, seconds, clicks } } => {
            s.record_start(&out, seconds, app, frame, clicks)
        }
        CaptureCommand::Record { command: RecordCommand::Wait } => s.record_wait(),
        CaptureCommand::Record { command: RecordCommand::Cancel } => s.record_cancel(),
    }
}

pub fn run(command: CaptureCommand, target: &Target) -> i32 {
    // Codex's sandbox can't see the permissions the user gave the app, or record the screen:
    // the run's host drives and records instead (BUG-stellar-lily-K8CN)
    if let Ok(ctx) = HelperContext::from_env() {
        if let Some(dir) = ctx.relay_dir.as_ref().filter(|_| video_studio_core::run::relay::sandboxed()) {
            let args: Vec<String> = std::env::args().skip(1).collect();
            return video_studio_core::run::relay::forward(dir, &args);
        }
    }
    match execute(command, target) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(e) => {
            eprintln!("capture: {e}");
            1
        }
    }
}
