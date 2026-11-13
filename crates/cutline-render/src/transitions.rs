//! Transition filters between two adjacent segment streams.

use crate::escape::f;
use crate::Geometry;
use cutline_core::model::Transition;

/// Filter line joining `a` and `b` (labels with brackets) into `[out]`.
/// `offset` = seconds into `a` where the transition starts.
pub fn pair(a: &str, b: &str, t: &Transition, frames: i64, offset: f64, g: &Geometry, out: &str) -> String {
    let d = g.fps.frames_to_secs(frames);
    let xf = |kind: &str| format!("{a}{b}xfade=transition={kind}:duration={}:offset={}[{out}]", f(d), f(offset));
    match t.kind.as_str() {
        k if cutline_core::catalog::XFADE_TRANSITIONS.contains(&k) => xf(k),
        _ => crate::custom_transitions::custom(a, b, t, d, offset, g, out),
    }
}
