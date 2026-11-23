//! Runs (spec §12): preparing the run folder, driving Claude Code, progress events,
//! cancellation, and collecting results.

pub mod collect;
pub mod events;
pub mod orchestrator;
pub mod prepare;
pub mod prompt;
pub mod proxy;
pub mod readiness;
pub mod relay;

pub use events::{Event, EventKind, EventSink};
pub use orchestrator::{start_run, RunHandle, RunOutcome};
pub use prepare::{RunEnv, RunRequest};
pub use readiness::ReadinessReport;

/// CLI exit codes (spec §13).
pub mod exit_code {
    pub const SUCCESS: i32 = 0;
    pub const SUCCESS_WITH_WARNINGS: i32 = 1;
    pub const VALIDATION_BLOCKERS: i32 = 2;
    pub const RUN_FAILED: i32 = 3;
    pub const CANCELLED: i32 = 4;
    pub const ENVIRONMENT: i32 = 5;

    use crate::project::RunStatus;

    pub fn for_status(status: RunStatus) -> i32 {
        match status {
            RunStatus::Succeeded => SUCCESS,
            RunStatus::SucceededWithWarnings => SUCCESS_WITH_WARNINGS,
            RunStatus::Failed | RunStatus::Running => RUN_FAILED,
            RunStatus::Cancelled => CANCELLED,
        }
    }
}
