//! Per-clip color grading and motion effects (filled in by M3).

use crate::segment::ClipExtras;
use crate::Geometry;
use cutline_core::model::SpeedKey;
use cutline_core::timeline::TClip;

/// Color + effects chain applied after geometry (operates at target resolution).
pub fn video_chain(_c: &TClip, _g: &Geometry, _x: &ClipExtras) -> Vec<String> {
    vec![]
}

/// Freeze-frame filters (extend the clip by holding a frame).
pub fn freeze_chain(_c: &TClip, _g: &Geometry) -> Vec<String> {
    vec![]
}

pub fn freeze_audio(_c: &TClip, _g: &Geometry) -> Vec<String> {
    vec![]
}

/// setpts expression for a keyframed speed ramp.
pub fn speed_ramp_setpts(_keys: &[SpeedKey]) -> String {
    "setpts=PTS".into()
}
