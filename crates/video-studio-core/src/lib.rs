//! Syncfusion Video Studio core (ui/spec.md).
//!
//! Everything the macOS shell and the `video-studio` CLI share: the `.cutproj` project model,
//! media references and relinking, brand and output presets, validation, the Claude Code client
//! with its permission broker, the run orchestrator, the ElevenLabs helper and run logs.
//!
//! Platform rule (spec §3): no OS-specific API outside [`platform`]. Paths are `camino` UTF-8
//! paths, nothing assumes `/`, and files are written with LF line endings.

pub mod assistant;
pub mod brand;
pub mod capture;
pub mod claude;
pub mod codex;
pub mod eleven;
pub mod error;
pub mod finish;
pub mod ids;
pub mod logs;
pub mod media;
pub mod models;
pub mod paths;
pub mod platform;
pub mod presets;
pub mod project;
pub mod run;
pub mod settings;
pub mod spec;
pub mod standin;
pub mod timeutil;
pub mod validation;
pub mod viewable;

pub use error::{Error, Result};

/// Version of the core crate, recorded in every run log.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Product name, used for the app library folder and in messages.
pub const APP_NAME: &str = "Syncfusion Video Studio";
/// Bundle identifier, also the Keychain / Credential Manager service name.
pub const BUNDLE_ID: &str = "com.syncfusion.videostudio";
