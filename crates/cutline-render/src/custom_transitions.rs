//! Custom composite transitions (whips, zoom punch, flash cut, glitch). Expanded in M3.

use crate::escape::f;
use crate::Geometry;
use cutline_core::model::Transition;

pub fn custom(a: &str, b: &str, t: &Transition, d: f64, offset: f64, _g: &Geometry, out: &str) -> String {
    let kind = match t.kind.as_str() {
        "whip_left" => "slideleft",
        "whip_right" => "slideright",
        "whip_up" => "slideup",
        "whip_down" => "slidedown",
        "zoom_punch" => "zoomin",
        "flash_cut" => "fadewhite",
        "glitch" => "pixelize",
        _ => "fade",
    };
    format!("{a}{b}xfade=transition={kind}:duration={}:offset={}[{out}]", f(d), f(offset))
}
