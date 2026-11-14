//! Project model, JSON5 parsing, schema, validation, time math and timeline resolution.

pub mod brand;
pub mod catalog;
pub mod diag;
pub mod frame;
pub mod model;
pub mod parse;
pub mod target;
pub mod time;
pub mod timeline;

pub use diag::{Diagnostic, Severity};
pub use model::Project;
pub use timeline::{resolve, Context, MediaFacts, MediaLookup, Timeline};

/// JSON Schema for the project file.
pub fn schema_json() -> String {
    let s = schemars::schema_for!(model::Project);
    let mut out = serde_json::to_string_pretty(&s).expect("schema serializes");
    out.push('\n');
    out
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
