//! Per-clip color grading and motion effects (applied at target resolution after geometry).

use crate::escape::{f, filter_path};
use crate::segment::ClipExtras;
use crate::Geometry;
use cutline_core::model::{ColorGrade, Ease, Effect, SpeedKey};
use cutline_core::timeline::TClip;

// ---------------------------------------------------------------- easing

/// Easing in Rust (used by tests and by the planner for sendcmd tables).
pub fn ease(e: Ease, u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    match e {
        Ease::Linear => u,
        Ease::InQuad => u * u,
        Ease::OutQuad => 1.0 - (1.0 - u).powi(2),
        Ease::InOutQuad => {
            if u < 0.5 {
                2.0 * u * u
            } else {
                1.0 - (-2.0 * u + 2.0).powi(2) / 2.0
            }
        }
        Ease::InCubic => u.powi(3),
        Ease::OutCubic => 1.0 - (1.0 - u).powi(3),
        Ease::InOutCubic => {
            if u < 0.5 {
                4.0 * u.powi(3)
            } else {
                1.0 - (-2.0 * u + 2.0).powi(3) / 2.0
            }
        }
        Ease::InExpo => {
            if u <= 0.0 {
                0.0
            } else {
                2f64.powf(10.0 * u - 10.0)
            }
        }
        Ease::OutExpo => {
            if u >= 1.0 {
                1.0
            } else {
                1.0 - 2f64.powf(-10.0 * u)
            }
        }
        Ease::InOutExpo => {
            if u <= 0.0 {
                0.0
            } else if u >= 1.0 {
                1.0
            } else if u < 0.5 {
                2f64.powf(20.0 * u - 10.0) / 2.0
            } else {
                (2.0 - 2f64.powf(-20.0 * u + 10.0)) / 2.0
            }
        }
        Ease::Back => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            1.0 + c3 * (u - 1.0).powi(3) + c1 * (u - 1.0).powi(2)
        }
        Ease::Spring => {
            if u >= 1.0 {
                1.0
            } else {
                1.0 - (-6.0 * u).exp() * (10.0 * u).cos() * (1.0 - u)
            }
        }
    }
}

/// ffmpeg expression for `ease(u)` where `u` is an expression already clipped to 0..1.
pub fn ease_expr(e: Ease, u: &str) -> String {
    match e {
        Ease::Linear => u.to_string(),
        Ease::InQuad => format!("pow({u},2)"),
        Ease::OutQuad => format!("(1-pow(1-{u},2))"),
        Ease::InOutQuad => format!("if(lt({u},0.5),2*pow({u},2),1-pow(-2*{u}+2,2)/2)"),
        Ease::InCubic => format!("pow({u},3)"),
        Ease::OutCubic => format!("(1-pow(1-{u},3))"),
        Ease::InOutCubic => format!("if(lt({u},0.5),4*pow({u},3),1-pow(-2*{u}+2,3)/2)"),
        Ease::InExpo => format!("if(lte({u},0),0,pow(2,10*{u}-10))"),
        Ease::OutExpo => format!("if(gte({u},1),1,1-pow(2,-10*{u}))"),
        Ease::InOutExpo => {
            format!("if(lte({u},0),0,if(gte({u},1),1,if(lt({u},0.5),pow(2,20*{u}-10)/2,(2-pow(2,-20*{u}+10))/2)))")
        }
        Ease::Back => format!("(1+2.70158*pow({u}-1,3)+1.70158*pow({u}-1,2))"),
        Ease::Spring => format!("if(gte({u},1),1,1-exp(-6*{u})*cos(10*{u})*(1-{u}))"),
    }
}

fn u_expr(t: &str, a: f64, d: f64) -> String {
    format!("clip(({t}-{})/{},0,1)", f(a), f(d.max(1e-6)))
}

// ---------------------------------------------------------------- zoom model

/// One multiplicative scale contribution over time.
#[derive(Debug, Clone)]
pub enum ScaleTerm {
    /// from → to over [a, a+d] with easing; holds `to` after.
    Ramp { a: f64, d: f64, from: f64, to: f64, ease: Ease },
    /// back from `to` → 1 over [a, a+d] (punch-in release).
    Release { a: f64, d: f64, to: f64, ease: Ease },
    /// constant margin factor inside [a, b] (shake headroom).
    Window { a: f64, b: f64, k: f64 },
}

impl ScaleTerm {
    pub fn eval(&self, t: f64) -> f64 {
        match *self {
            ScaleTerm::Ramp { a, d, from, to, ease: e } => from + (to - from) * ease(e, (t - a) / d.max(1e-6)),
            ScaleTerm::Release { a, d, to, ease: e } => {
                if t < a {
                    1.0
                } else {
                    (to + (1.0 - to) * ease(e, (t - a) / d.max(1e-6))) / to
                }
            }
            ScaleTerm::Window { a, b, k } => {
                if t >= a && t <= b {
                    k
                } else {
                    1.0
                }
            }
        }
    }
    pub fn expr(&self, t: &str) -> String {
        match *self {
            ScaleTerm::Ramp { a, d, from, to, ease: e } => {
                format!("({}+{}*{})", f(from), f(to - from), ease_expr(e, &u_expr(t, a, d)))
            }
            ScaleTerm::Release { a, d, to, ease: e } => {
                format!(
                    "if(lt({t},{}),1,({}+{}*{})/{})",
                    f(a),
                    f(to),
                    f(1.0 - to),
                    ease_expr(e, &u_expr(t, a, d)),
                    f(to)
                )
            }
            ScaleTerm::Window { a, b, k } => format!("if(between({t},{},{}),{},1)", f(a), f(b), f(k)),
        }
    }
}

/// Pan offset terms (output pixels) — shake.
#[derive(Debug, Clone)]
pub struct Shake {
    pub a: f64,
    pub d: f64,
    pub amp_x: f64,
    pub amp_y: f64,
}

/// Collect zoom/shake terms for a clip. Returns (terms, focus, shakes, ken-burns pan).
pub fn zoom_terms(
    c: &TClip,
    dur: f64,
) -> (Vec<ScaleTerm>, (f64, f64), Vec<Shake>, Option<((f64, f64), (f64, f64), Ease)>) {
    let mut terms = vec![];
    let mut focus = (0.5, 0.5);
    let mut shakes = vec![];
    let mut kb = None;
    for e in &c.clip.effects {
        match e {
            Effect::PunchIn { at, scale, duration, ease: ez, hold, focus: fo, .. } => {
                terms.push(ScaleTerm::Ramp { a: *at, d: *duration, from: 1.0, to: *scale, ease: *ez });
                if let Some(h) = hold {
                    terms.push(ScaleTerm::Release { a: at + duration + h, d: *duration, to: *scale, ease: *ez });
                }
                if let Some(p) = fo {
                    focus = (p.x, p.y);
                }
            }
            Effect::Zoom { from, to, start, end, ease: ez, focus: fo } => {
                let e2 = end.unwrap_or(dur);
                terms.push(ScaleTerm::Ramp { a: *start, d: e2 - start, from: *from, to: *to, ease: *ez });
                if let Some(p) = fo {
                    focus = (p.x, p.y);
                }
            }
            Effect::ZoomTo { rect, start, duration, ease: ez, padding, end } => {
                let (s, p) = cutline_core::model::zoom_to_params(rect, padding.unwrap_or(0.03), 3.0);
                if s > 1.0 {
                    terms.push(ScaleTerm::Ramp { a: *start, d: *duration, from: 1.0, to: s, ease: *ez });
                    if let Some(e) = end {
                        terms.push(ScaleTerm::Release { a: *e, d: *duration, to: s, ease: *ez });
                    }
                    focus = (p.x, p.y);
                }
            }
            Effect::KenBurns { from_scale, to_scale, from, to, ease: ez } => {
                terms.push(ScaleTerm::Ramp { a: 0.0, d: dur, from: *from_scale, to: *to_scale, ease: *ez });
                let p0 = from.map(|p| (p.x, p.y)).unwrap_or((0.5, 0.5));
                let p1 = to.map(|p| (p.x, p.y)).unwrap_or((0.5, 0.5));
                kb = Some((p0, p1, *ez));
            }
            Effect::Shake { at, duration, intensity } => {
                terms.push(ScaleTerm::Window { a: *at, b: at + duration, k: 1.0 + 2.5 * intensity });
                shakes.push(Shake { a: *at, d: *duration, amp_x: *intensity, amp_y: *intensity * 0.7 });
            }
            _ => {}
        }
    }
    (terms, focus, shakes, kb)
}

/// Largest combined zoom scale over the clip (sampled).
pub fn max_zoom(c: &TClip, dur: f64) -> f64 {
    let (terms, ..) = zoom_terms(c, dur);
    if terms.is_empty() {
        return 1.0;
    }
    (0..=200)
        .map(|i| {
            let t = dur * i as f64 / 200.0;
            terms.iter().map(|x| x.eval(t)).product::<f64>()
        })
        .fold(1.0, f64::max)
}

/// The zoom at clip-local time `t`: scale, and the left/top of the visible window as fractions
/// of the frame. Mirrors `zoom_chain` (without shake), so overlays can follow the picture.
pub fn zoom_at(c: &TClip, dur: f64, t: f64) -> (f64, f64, f64) {
    let (terms, focus, _, kb) = zoom_terms(c, dur);
    if terms.is_empty() {
        return (1.0, 0.0, 0.0);
    }
    let s = terms.iter().map(|x| x.eval(t)).product::<f64>().max(1e-6);
    let (fx, fy) = match kb {
        Some(((x0, y0), (x1, y1), ez)) => {
            let u = ease(ez, t / dur.max(1e-6));
            (x0 + (x1 - x0) * u, y0 + (y1 - y0) * u)
        }
        None => focus,
    };
    let win = |f: f64| ((s - 1.0) * f / s).clamp(0.0, (1.0 - 1.0 / s).max(0.0));
    (s, win(fx), win(fy))
}

/// Zoom/pan/shake chain with sub-pixel motion.
///
/// The frame is resampled by `perspective` (cubic), whose source rectangle is evaluated per
/// frame in fractional pixels: the picture glides instead of advancing in whole-pixel steps
/// (a `scale`+`crop` zoom can only grow by 2 px and pan by 1 px at a time, which shows as
/// ticking on slow moves and as text shimmer on screen recordings).
///
/// `perspective` works on 8-bit planes and has no `t`; time is `(in-1)/fps` (`in` counts
/// from 1). `g` may be a supersampled geometry (see `segment::build`).
pub fn zoom_chain(c: &TClip, g: &Geometry, dur: f64) -> Option<String> {
    let (terms, focus, shakes, kb) = zoom_terms(c, dur);
    if terms.is_empty() {
        return None;
    }
    let (w, h) = (g.width, g.height);
    let t = format!("((in-1)/({}))", g.fps.ff());
    let s = format!("({})", terms.iter().map(|x| x.expr(&t)).collect::<Vec<_>>().join("*"));
    let (fx, fy) = match kb {
        Some(((x0, y0), (x1, y1), ez)) => {
            let u = ease_expr(ez, &u_expr(&t, 0.0, dur));
            (format!("({}+{}*{u})", f(x0), f(x1 - x0)), format!("({}+{}*{u})", f(y0), f(y1 - y0)))
        }
        None => (f(focus.0), f(focus.1)),
    };
    let mut sx = String::new();
    let mut sy = String::new();
    for sk in &shakes {
        let env = format!("between({t},{},{})*(1-{})", f(sk.a), f(sk.a + sk.d), u_expr(&t, sk.a, sk.d));
        sx.push_str(&format!("+{}*{w}*sin(2*PI*23*{t})*{env}", f(sk.amp_x)));
        sy.push_str(&format!("+{}*{h}*cos(2*PI*17*{t})*{env}", f(sk.amp_y)));
    }
    // Source window (pixels): size (w/s, h/s), placed so the focus point keeps its relative
    // position; shake offsets are in zoomed pixels, hence `/s`. The `0.5/s-0.5` term makes
    // the mapping exact at pixel centers.
    let left = format!("(clip(({w}*({s}-1)*{fx}{sx})/{s},0,{w}-{w}/{s})+0.5/{s}-0.5)");
    let top = format!("(clip(({h}*({s}-1)*{fy}{sy})/{s},0,{h}-{h}/{s})+0.5/{s}-0.5)");
    let right = format!("({left}+{w}/{s})");
    let bottom = format!("({top}+{h}/{s})");
    Some(format!(
        "format=yuv444p,perspective=x0='{left}':y0='{top}':x1='{right}':y1='{top}':x2='{left}':y2='{bottom}':x3='{right}':y3='{bottom}':interpolation=cubic:sense=source:eval=frame"
    ))
}

// ---------------------------------------------------------------- color

/// Color grading chain. Returns filter graph text from `[{inp}]` to `[{out}]`.
pub fn color_graph(cg: &ColorGrade, inp: &str, out: &str, x: &ClipExtras) -> Option<String> {
    let mut ch: Vec<String> = vec![];
    let preset = cg.preset.as_deref().unwrap_or("");
    let (mut con, mut sat, mut temp, mut sharp, mut bw) = (1.0f64, 1.0f64, None, 0.0f64, false);
    match preset {
        "natural" => {
            con = 1.03;
            sat = 1.05;
        }
        "punchy" => {
            con = 1.12;
            sat = 1.25;
            sharp = 0.4;
        }
        "warm_film" => {
            con = 1.05;
            sat = 0.95;
            temp = Some(5600.0);
        }
        "cool_clean" => {
            con = 1.05;
            sat = 1.05;
            temp = Some(7600.0);
        }
        "bw_contrast" => {
            con = 1.3;
            bw = true;
        }
        _ => {}
    }
    if let Some(c) = cg.contrast {
        con *= c;
    }
    if let Some(s) = cg.saturation {
        sat *= s;
    }
    if let Some(t) = cg.temperature {
        temp = Some(t);
    }
    if let Some(s) = cg.sharpen {
        sharp = s;
    }
    if let Some(e) = cg.exposure {
        if e.abs() > 1e-6 {
            ch.push(format!("exposure=exposure={}", f(e)));
        }
    }
    if (con - 1.0).abs() > 1e-6 || (sat - 1.0).abs() > 1e-6 {
        ch.push(format!("eq=contrast={}:saturation={}", f(con), f(sat)));
    }
    if bw {
        ch.push("hue=s=0".into());
    }
    if let Some(t) = temp {
        ch.push(format!("colortemperature=temperature={}", f(t)));
    }
    if let Some(t) = cg.tint {
        if t.abs() > 1e-6 {
            ch.push(format!("colorbalance=gm={}:rm={}:bm={}", f(-0.15 * t), f(0.07 * t), f(0.07 * t)));
        }
    }
    if sharp > 0.0 {
        ch.push(format!("unsharp=5:5:{}:5:5:0", f(sharp)));
    }
    let lut = x.lut_path.as_ref();
    if ch.is_empty() && lut.is_none() {
        return None;
    }
    let mut g = String::new();
    let mut cur = inp.to_string();
    if let Some(l) = lut {
        let i = cg.intensity.unwrap_or(1.0).clamp(0.0, 1.0);
        if (i - 1.0).abs() < 1e-6 {
            g.push_str(&format!("[{cur}]lut3d=file={}:interp=tetrahedral[{out}_l];", filter_path(l)));
        } else {
            g.push_str(&format!(
                "[{cur}]split=2[{out}_o][{out}_i];[{out}_i]lut3d=file={}:interp=tetrahedral[{out}_li];[{out}_o][{out}_li]blend=all_expr='A*{}+B*{}'[{out}_l];",
                filter_path(l),
                f(1.0 - i),
                f(i)
            ));
        }
        cur = format!("{out}_l");
    }
    if ch.is_empty() {
        g.push_str(&format!("[{cur}]null[{out}]"));
    } else {
        g.push_str(&format!("[{cur}]{}[{out}]", ch.join(",")));
    }
    Some(g)
}

// ---------------------------------------------------------------- per-clip chains

/// Look effects after zoom: flash, glitch, vignette, blur-in, motion blur.
fn look_chain(c: &TClip, g: &Geometry) -> Vec<String> {
    let mut v = vec![];
    let fps = g.fps.as_f64();
    for e in &c.clip.effects {
        match e {
            Effect::Flash { at, duration, color } => {
                let env = format!("max(0,1-abs(t-{})/{})", f(at + duration / 2.0), f((duration / 2.0).max(1.0 / fps)));
                match color.as_deref() {
                    None | Some("white") | Some("#FFFFFF") | Some("#ffffff") => {
                        v.push(format!("eq=brightness='0.85*{env}':eval=frame"))
                    }
                    Some(col) => {
                        let c = crate::overlay::ff_color(col);
                        // tint toward the color via a full-frame drawbox blended by time window
                        v.push(format!(
                            "drawbox=x=0:y=0:w=iw:h=ih:color={c}@0.75:t=fill:enable='between(t,{},{})'",
                            f(*at),
                            f(at + duration)
                        ));
                    }
                }
            }
            Effect::Glitch { at, duration, amount } => {
                let a = amount.round() as i64;
                let en = format!("between(t,{},{})", f(*at), f(at + duration));
                v.push(format!("rgbashift=rh={a}:bh={}:gv={}:enable='{en}'", -a, a / 3));
                v.push(format!("noise=alls=18:allf=t:all_seed=42:enable='{en}'"));
            }
            Effect::Vignette { strength } => {
                v.push(format!("vignette=angle={}", f(0.25 + 0.45 * strength.clamp(0.0, 1.0))));
            }
            Effect::MotionBlur { at, duration } => {
                v.push(format!("tmix=frames=3:weights='1 2 1':enable='between(t,{},{})'", f(*at), f(at + duration)));
            }
            _ => {}
        }
    }
    v
}

/// Blur-in via runtime commands: gblur sigma ramps from 30 → 0 over `duration`.
fn blur_in(c: &TClip, g: &Geometry) -> Option<String> {
    let d =
        c.clip.effects.iter().find_map(|e| if let Effect::BlurIn { duration } = e { Some(*duration) } else { None })?;
    let fps = g.fps.as_f64();
    let n = (d * fps).ceil() as i64;
    let mut cmds = vec![];
    for i in 0..=n {
        let t = i as f64 / fps;
        let s = 30.0 * (1.0 - ease(Ease::OutCubic, t / d));
        cmds.push(format!("{} gblur@bi sigma {}", f(t), f(s.max(0.0))));
    }
    // sendcmd syntax: commands separated by ';' inside quotes; escape for filter level
    Some(format!("sendcmd=c='{}',gblur@bi=sigma=30", cmds.join(";").replace(';', "\\;")))
}

/// Color + effects chain applied after geometry. Returns comma-chain elements; a color graph
/// with LUT blending is emitted separately by `video_graph`.
///
/// `zg` is the geometry the zoom works in: the target, or a supersampled one when the source
/// has more pixels than the target (the zoom then reveals real detail instead of enlarging
/// an already downscaled frame); the chain scales back to `g` after the zoom.
pub fn video_chain(c: &TClip, g: &Geometry, zg: &Geometry, _x: &ClipExtras) -> Vec<String> {
    let dur = g.fps.frames_to_secs(c.frames);
    let mut v = vec![];
    if let Some(z) = zoom_chain(c, zg, dur) {
        v.push(z);
        if zg.width != g.width || zg.height != g.height {
            v.push(format!("scale={}:{}:flags=lanczos", g.width, g.height));
        }
    }
    if let Some(b) = blur_in(c, g) {
        v.push(b);
    }
    v.extend(look_chain(c, g));
    v
}

/// Freeze-frame filters: `loop` repeats the frame at `at` for `duration`.
pub fn freeze_chain(c: &TClip, g: &Geometry) -> Vec<String> {
    let mut v = vec![];
    let fps = g.fps;
    for e in &c.clip.effects {
        if let Effect::Freeze { at, duration } = e {
            let start = fps.secs_to_frames(*at);
            let n = fps.secs_to_frames(*duration);
            v.push(format!("loop=loop={n}:size=1:start={start}"));
            v.push(format!("setpts=N/({})/TB", fps.ff()));
        }
    }
    v
}

/// Audio during a freeze: duplicate the span then mute it (keeps sync, silent hold).
pub fn freeze_audio(c: &TClip, _g: &Geometry) -> Vec<String> {
    let mut v = vec![];
    for e in &c.clip.effects {
        if let Effect::Freeze { at, duration } = e {
            let x = (at * 48000.0).round() as i64;
            let s = (duration * 48000.0).round() as i64;
            v.push(format!("aloop=loop=1:size={s}:start={x}"));
            v.push("asetpts=N/SR/TB".into());
            v.push(format!("volume=0:enable='between(t,{},{})'", f(*at), f(at + duration)));
        }
    }
    v
}

/// setpts for a piecewise-linear speed ramp (keys at source-relative seconds).
/// Output time T(s) = ∫₀ˢ dx / v(x), closed form per linear segment.
pub fn speed_ramp_setpts(keys: &[SpeedKey]) -> String {
    if keys.is_empty() {
        return "setpts=PTS".into();
    }
    let mut ks: Vec<(f64, f64)> = keys.iter().map(|k| (k.t, k.speed.max(0.01))).collect();
    ks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    if ks[0].0 > 0.0 {
        ks.insert(0, (0.0, ks[0].1));
    }
    // integral of segment i from its start to x (as expression in x)
    let seg = |s0: f64, v0: f64, s1: f64, v1: f64, x: &str| -> String {
        let k = (v1 - v0) / (s1 - s0).max(1e-9);
        if k.abs() < 1e-9 {
            format!("(({x})-{})/{}", f(s0), f(v0))
        } else {
            format!("log(({}+{}*(({x})-{}))/{})/{}", f(v0), f(k), f(s0), f(v0), f(k))
        }
    };
    let full = |s0: f64, v0: f64, s1: f64, v1: f64| -> f64 {
        let k = (v1 - v0) / (s1 - s0).max(1e-9);
        if k.abs() < 1e-9 {
            (s1 - s0) / v0
        } else {
            ((v0 + k * (s1 - s0)) / v0).ln() / k
        }
    };
    let x = "(T)";
    let mut expr = {
        let (s, v) = *ks.last().unwrap();
        // after the last key: constant speed
        let base: f64 = ks.windows(2).map(|w| full(w[0].0, w[0].1, w[1].0, w[1].1)).sum();
        format!("{}+(({x})-{})/{}", f(base), f(s), f(v))
    };
    let mut acc: Vec<f64> = vec![0.0];
    for w in ks.windows(2) {
        acc.push(acc.last().unwrap() + full(w[0].0, w[0].1, w[1].0, w[1].1));
    }
    for i in (0..ks.len() - 1).rev() {
        let (s0, v0) = ks[i];
        let (s1, v1) = ks[i + 1];
        expr = format!("if(lt({x},{}),{}+{},{expr})", f(s1), f(acc[i]), seg(s0, v0, s1, v1, x));
    }
    format!("setpts='({expr})/TB'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_endpoints_and_monotone() {
        for e in [
            Ease::Linear,
            Ease::InQuad,
            Ease::OutQuad,
            Ease::InOutQuad,
            Ease::InCubic,
            Ease::OutCubic,
            Ease::InOutCubic,
            Ease::InExpo,
            Ease::OutExpo,
            Ease::InOutExpo,
        ] {
            assert!(ease(e, 0.0).abs() < 1e-3, "{e:?}");
            assert!((ease(e, 1.0) - 1.0).abs() < 1e-3, "{e:?}");
            let mut prev = -1.0;
            for i in 0..=100 {
                let v = ease(e, i as f64 / 100.0);
                assert!(v >= prev - 1e-12, "{e:?} not monotone");
                prev = v;
            }
        }
        assert!(ease(Ease::Back, 0.8) > 1.0, "back overshoots");
    }

    #[test]
    fn ramp_integral() {
        // constant 2× → T = s/2 at s=1
        let s = speed_ramp_setpts(&[SpeedKey { t: 0.0, speed: 2.0 }, SpeedKey { t: 2.0, speed: 2.0 }]);
        assert!(s.contains("/2"), "{s}");
    }
}
