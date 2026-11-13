//! Synthetic media generators, frame-ID encode/decode, tone analysis and golden image compare.

pub mod analyze;
pub mod frameid;
pub mod gen;

pub use frameid::{decode_frame_id, FrameIdLayout};

/// Toolchain for tests; returns `None` (and prints why) when ffmpeg is unavailable so tests
/// can skip instead of failing on machines without tools.
pub fn test_toolchain() -> Option<&'static cutline_deps::Toolchain> {
    match cutline_deps::toolchain() {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("SKIP: no ffmpeg toolchain: {e}");
            None
        }
    }
}
