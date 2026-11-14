//! Claude Code integration (spec §4): finding the CLI, the stream-json protocol, the permission
//! broker and the child process.

pub mod broker;
pub mod locate;
pub mod process;
pub mod stream;

pub use broker::{Decision, Mode, Policy};
pub use locate::{ClaudeInfo, CutlineInfo};
pub use process::{ClaudeOptions, ClaudeProcess, Incoming};
pub use stream::{StreamEvent, Usage};
