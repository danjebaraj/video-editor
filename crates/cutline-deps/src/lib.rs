//! External tool management: download, verification, lockfile and discovery.

pub mod download;
pub mod install;
pub mod lockfile;
pub mod paths;
pub mod tools;

pub use tools::{toolchain, Toolchain};
