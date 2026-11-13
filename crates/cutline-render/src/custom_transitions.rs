//! Custom composite transitions: whips (fast slide + directional blur), zoom punch, flash cut,
//! glitch. Each is an xfade core plus windowed post-filters during the overlap.

use crate::escape::f;
use crate::Geometry;
use cutline_core::model::Transition;

pub fn custom(a: &str, b: &str, t: &Transition, d: f64, offset: f64, g: &Geometry, out: &str) -> String {
    let en = format!("between(t,{},{})", f(offset), f(offset + d));
    let blur_px = ((g.width.max(g.height) as f64) / 24.0).round().max(4.0) as u32;
    let core = |kind: &str| format!("{a}{b}xfade=transition={kind}:duration={}:offset={}", f(d), f(offset));
    match t.kind.as_str() {
        "whip_left" | "whip_right" => format!(
            "{}[{out}_x];[{out}_x]avgblur=sizeX={blur_px}:sizeY=1:enable='{en}'[{out}]",
            core(if t.kind == "whip_left" { "slideleft" } else { "slideright" })
        ),
        "whip_up" | "whip_down" => format!(
            "{}[{out}_x];[{out}_x]avgblur=sizeX=1:sizeY={blur_px}:enable='{en}'[{out}]",
            core(if t.kind == "whip_up" { "slideup" } else { "slidedown" })
        ),
        "zoom_punch" => {
            let mid = offset + d / 2.0;
            format!(
                "{}[{out}_x];[{out}_x]eq=brightness='0.25*max(0,1-abs(t-{})/{})':eval=frame[{out}]",
                core("zoomin"),
                f(mid),
                f((d / 2.0).max(0.01))
            )
        }
        "flash_cut" => match t.color.as_deref() {
            None | Some("white") => core("fadewhite") + &format!("[{out}]"),
            Some(c) => {
                let col = crate::overlay::ff_color(c);
                format!(
                    "{}[{out}_x];[{out}_x]drawbox=x=0:y=0:w=iw:h=ih:color={col}@0.85:t=fill:enable='between(t,{},{})'[{out}]",
                    core("fade"),
                    f(offset + d * 0.3),
                    f(offset + d * 0.7)
                )
            }
        },
        "glitch" => format!(
            "{}[{out}_x];[{out}_x]rgbashift=rh=14:bh=-14:gv=5:enable='{en}',noise=alls=24:allf=t:all_seed=42:enable='{en}'[{out}]",
            core("pixelize")
        ),
        _ => core("fade") + &format!("[{out}]"),
    }
}
