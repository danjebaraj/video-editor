//! Overlay pass on the joined main video: B-roll, images, alpha video, image sequences, HTML
//! renders, ASS text/captions and the progress bar.

use crate::escape::{f, filter_path};
use crate::job::Job;
use crate::{Geometry, PlanExtras};
use anyhow::Result;
use cutline_core::model::{Anim, AssetKind, Ease, Fit, Layer, Position, Project, Visual};
use cutline_core::timeline::{TLayer, Timeline};
use cutline_media::assets::ProjectMedia;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct OverlayOut {
    pub input_args: Vec<String>,
    pub scripts: Vec<(PathBuf, String)>,
    pub files: Vec<(PathBuf, Vec<u8>)>,
    pub pre_jobs: Vec<Job>,
}

/// `#RRGGBB`/`#RRGGBBAA` → ffmpeg `0xRRGGBB[AA]`; names pass through.
pub fn ff_color(c: &str) -> String {
    let c = c.trim();
    if let Some(h) = c.strip_prefix('#') {
        format!("0x{h}")
    } else {
        c.to_string()
    }
}

/// Parse `#RRGGBB` / `0xRRGGBB` to RGB.
pub fn rgb_of(c: &str) -> [u8; 3] {
    let h = c.trim().trim_start_matches('#').trim_start_matches("0x");
    if h.len() >= 6 {
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
        return [p(0), p(2), p(4)];
    }
    match c {
        "white" => [255, 255, 255],
        "black" => [0, 0, 0],
        "red" => [255, 0, 0],
        _ => [255, 255, 255],
    }
}

/// Top-left position expressions (`x`, `y`) for an overlay of size `w×h` (ffmpeg overlay vars).
pub fn position_exprs(pos: Option<&Position>, safe: (f64, f64, f64, f64), default: &str) -> (String, String) {
    let (sx, sy, sw, sh) = safe;
    let preset = match pos {
        Some(Position::Point(p)) => return (format!("{}*W-w/2", f(p.x)), format!("{}*H-h/2", f(p.y))),
        Some(Position::Preset(s)) => s.as_str(),
        None => default,
    };
    let (l, t, r, b) = (f(sx), f(sy), format!("{}-w", f(sx + sw)), format!("{}-h", f(sy + sh)));
    let cx = "(W-w)/2".to_string();
    let cy = "(H-h)/2".to_string();
    match preset {
        "top_left" => (l, t),
        "top_right" => (r, t),
        "bottom_left" => (l, b),
        "bottom_right" => (r, b),
        "top" => (cx, t),
        "bottom" => (cx, b),
        "left" => (l, cy),
        "right" => (r, cy),
        "lower_mid" => (cx, format!("{}-h/2", f(sy + sh * 0.72))),
        "upper_mid" => (cx, format!("{}-h/2", f(sy + sh * 0.25))),
        _ => (cx, cy),
    }
}

fn u(t0: f64, d: f64) -> String {
    format!("clip((t-{})/{},0,1)", f(t0), f(d.max(1e-3)))
}

/// Slide offsets (dx, dy) for enter/exit animations, in main-frame pixels.
fn slide_offsets(v: &Visual, s: f64, e: f64) -> (String, String) {
    let ad = v.anim_duration.unwrap_or(0.3);
    let ein = crate::effects::ease_expr(Ease::OutCubic, &u(s, ad));
    let eout = crate::effects::ease_expr(Ease::InCubic, &u(e - ad, ad));
    let mut dx = String::new();
    let mut dy = String::new();
    match v.enter {
        Some(Anim::SlideLeft) => dx.push_str(&format!("+W*(1-{ein})")),
        Some(Anim::SlideRight) => dx.push_str(&format!("-W*(1-{ein})")),
        Some(Anim::SlideUp) => dy.push_str(&format!("+H*(1-{ein})")),
        Some(Anim::SlideDown) => dy.push_str(&format!("-H*(1-{ein})")),
        _ => {}
    }
    match v.exit {
        Some(Anim::SlideLeft) => dx.push_str(&format!("-W*{eout}")),
        Some(Anim::SlideRight) => dx.push_str(&format!("+W*{eout}")),
        Some(Anim::SlideUp) => dy.push_str(&format!("-H*{eout}")),
        Some(Anim::SlideDown) => dy.push_str(&format!("+H*{eout}")),
        _ => {}
    }
    (dx, dy)
}

/// Layer-local chain for fades / scale animations (local time starts at 0).
fn anim_chain(v: &Visual, len: f64, w: u32) -> Vec<String> {
    let ad = v.anim_duration.unwrap_or(0.3).min(len / 2.0);
    let mut ch = vec![];
    let scale_in = matches!(v.enter, Some(Anim::Pop) | Some(Anim::ScaleIn));
    let scale_out = matches!(v.exit, Some(Anim::Pop) | Some(Anim::ScaleIn));
    if scale_in || scale_out {
        let mut k = String::from("1");
        if scale_in {
            let (from, e) = if v.enter == Some(Anim::Pop) { (0.2, Ease::Back) } else { (0.6, Ease::OutCubic) };
            k = format!("({}+{}*{})", f(from), f(1.0 - from), crate::effects::ease_expr(e, &u(0.0, ad)));
        }
        if scale_out {
            k = format!("{k}*(1-0.8*{})", crate::effects::ease_expr(Ease::InCubic, &u(len - ad, ad)));
        }
        ch.push(format!("scale=w='max(2,2*round({w}*{k}/2))':h=-2:eval=frame"));
    }
    if matches!(v.enter, Some(Anim::Fade) | Some(Anim::ScaleIn)) {
        ch.push(format!("fade=t=in:st=0:d={}:alpha=1", f(ad)));
    }
    if matches!(v.exit, Some(Anim::Fade) | Some(Anim::ScaleIn)) {
        ch.push(format!("fade=t=out:st={}:d={}:alpha=1", f((len - ad).max(0.0)), f(ad)));
    }
    ch
}

/// Rounded-rect alpha mask (white inside) as PNG bytes.
pub fn rounded_mask_png(w: u32, h: u32, r: f64) -> Vec<u8> {
    let img = image::GrayImage::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
        let cx = fx.clamp(r, w as f64 - r);
        let cy = fy.clamp(r, h as f64 - r);
        let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
        let a = (r - d + 0.5).clamp(0.0, 1.0);
        image::Luma([(a * 255.0).round() as u8])
    });
    let mut buf = std::io::Cursor::new(vec![]);
    img.write_to(&mut buf, image::ImageFormat::Png).expect("png encode");
    buf.into_inner()
}

/// Frame PNG: soft drop shadow + rounded border behind content of `w×h` (RGBA).
/// Returns (png, total_w, total_h, content offset).
pub fn frame_png(w: u32, h: u32, r: f64, border: f64, border_rgb: [u8; 3], shadow: bool) -> (Vec<u8>, u32, u32, u32) {
    let pad = if shadow { 24u32 } else { 0 } + border.ceil() as u32;
    let (tw, th) = (w + 2 * pad, h + 2 * pad);
    let br = r + border;
    let (bx0, by0) = (pad as f64 - border, pad as f64 - border);
    let (bw, bh) = (w as f64 + 2.0 * border, h as f64 + 2.0 * border);
    let sdf = |fx: f64, fy: f64, x0: f64, y0: f64, ww: f64, hh: f64, rr: f64| -> f64 {
        let cx = fx.clamp(x0 + rr, x0 + ww - rr);
        let cy = fy.clamp(y0 + rr, y0 + hh - rr);
        ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() - rr
    };
    let img = image::RgbaImage::from_fn(tw, th, |x, y| {
        let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
        let mut px = [0f64; 4];
        if shadow {
            let d = sdf(fx, fy - 8.0, bx0, by0, bw, bh, br);
            let a = (0.45 * (1.0 - (d / 18.0).clamp(0.0, 1.0))).max(0.0);
            px = [0.0, 0.0, 0.0, a];
        }
        if border > 0.0 {
            let d = sdf(fx, fy, bx0, by0, bw, bh, br);
            let a = (0.5 - d).clamp(0.0, 1.0);
            if a > 0.0 {
                let c = border_rgb.map(|v| v as f64 / 255.0);
                let oa = a + px[3] * (1.0 - a);
                for i in 0..3 {
                    px[i] = (c[i] * a + px[i] * px[3] * (1.0 - a)) / oa.max(1e-9);
                }
                px[3] = oa;
            }
        }
        image::Rgba([
            (px[0] * 255.0) as u8,
            (px[1] * 255.0) as u8,
            (px[2] * 255.0) as u8,
            (px[3] * 255.0).round() as u8,
        ])
    });
    let mut buf = std::io::Cursor::new(vec![]);
    img.write_to(&mut buf, image::ImageFormat::Png).expect("png encode");
    (buf.into_inner(), tw, th, pad)
}

/// A mask layer, resolved for one render geometry.
pub struct MaskSpec<'a> {
    pub rect: cutline_core::model::Rect,
    pub mode: cutline_core::model::MaskMode,
    pub color: Option<&'a str>,
    pub strength: f64,
    pub text: Option<&'a str>,
    pub text_color: Option<&'a str>,
    /// Text size in px at this geometry.
    pub text_size: Option<f64>,
}

/// Pixel rectangle `(x, y, w, h)` of a normalized rect in a `w×h` frame (at least 2×2, inside).
pub fn mask_px(r: &cutline_core::model::Rect, (w, h): (u32, u32)) -> (u32, u32, u32, u32) {
    let px = |v: f64, n: u32| ((v * n as f64).round().max(0.0) as u32).min(n);
    let (x, y) = (px(r.x, w).min(w.saturating_sub(2)), px(r.y, h).min(h.saturating_sub(2)));
    let mw = px(r.x + r.w, w).saturating_sub(x).clamp(2, w - x);
    let mh = px(r.y + r.h, h).saturating_sub(y).clamp(2, h - y);
    (x, y, mw, mh)
}

/// Filter lines hiding a region of `cur` during `[s, e]`, plus optional replacement text.
pub fn mask_lines(
    lines: &mut Vec<String>,
    cur: &mut String,
    lab: &str,
    m: MaskSpec,
    frame: (u32, u32),
    (s, e): (f64, f64),
    fonts_dir: Option<&Path>,
) {
    use cutline_core::model::MaskMode;
    let (x, y, mw, mh) = mask_px(&m.rect, frame);
    let en = format!("enable='between(t,{},{})'", f(s), f(e));
    let short = mw.min(mh) as f64;
    match m.mode {
        MaskMode::Fill => {
            let col = ff_color(m.color.unwrap_or("#000000"));
            lines.push(format!("[{cur}]drawbox=x={x}:y={y}:w={mw}:h={mh}:color={col}:t=fill:{en}[{lab}o]"));
        }
        MaskMode::Blur => {
            // sigma of half the region's short side wipes out text that fills the region
            let sigma = (0.5 * short * m.strength).clamp(2.0, 150.0);
            lines.push(format!("[{cur}]split[{lab}a][{lab}b]"));
            lines.push(format!("[{lab}b]crop={mw}:{mh}:{x}:{y},gblur=sigma={}:steps=3[{lab}c]", f(sigma)));
            lines.push(format!("[{lab}a][{lab}c]overlay={x}:{y}:{en}[{lab}o]"));
        }
        MaskMode::Pixelate => {
            let block = ((short / 3.0 * m.strength).round() as u32).clamp(2, mw.min(mh).max(2));
            let (bw, bh) = ((mw / block).max(1), (mh / block).max(1));
            lines.push(format!("[{cur}]split[{lab}a][{lab}b]"));
            lines.push(format!(
                "[{lab}b]crop={mw}:{mh}:{x}:{y},scale={bw}:{bh}:flags=area,scale={mw}:{mh}:flags=neighbor[{lab}c]"
            ));
            lines.push(format!("[{lab}a][{lab}c]overlay={x}:{y}:{en}[{lab}o]"));
        }
    }
    *cur = format!("{lab}o");
    let Some(t) = m.text.filter(|t| !t.is_empty()) else { return };
    let Some(font) = fonts_dir.map(|d| d.join("Inter[opsz,wght].ttf")).filter(|p| p.exists()) else {
        tracing::warn!("mask text skipped: bundled Inter font not found");
        return;
    };
    let size = m.text_size.unwrap_or(0.6 * mh as f64).max(4.0);
    lines.push(format!(
        "[{cur}]drawtext=fontfile={}:text={}:fontsize={}:fontcolor={}:x={}:y={y}+({mh}-th)/2:{en}[{lab}t]",
        filter_path(&font),
        crate::escape::drawtext_text(t),
        f(size),
        ff_color(m.text_color.unwrap_or("#9AA0A6")),
        f(x as f64 + 0.3 * size)
    ));
    *cur = format!("{lab}t");
}

#[allow(clippy::too_many_arguments)]
pub fn apply(
    tl: &Timeline,
    g: &Geometry,
    full: &Geometry,
    lines: &mut Vec<String>,
    cur: &mut String,
    next_input: &mut usize,
    extras: &PlanExtras,
    p: &Project,
    media: &ProjectMedia,
    work: &Path,
) -> Result<OverlayOut> {
    let mut o = OverlayOut::default();
    let (w, h) = (g.width, g.height);
    let k = g.scale_to(full.width);
    let (sx, sy, sw, sh) = tl.target.safe_rect();
    let safe = (sx * k, sy * k, sw * k, sh * k);
    let fps = g.fps;
    let mut n = 0usize;
    let mut push_overlay = |lines: &mut Vec<String>, cur: &mut String, ov: &str, x: &str, y: &str, s: f64, e: f64| {
        n += 1;
        let out = format!("ov{n}");
        lines.push(format!(
            "[{cur}][{ov}]overlay=x='{x}':y='{y}':enable='between(t,{},{})':eof_action=pass:format=auto[{out}]",
            f(s),
            f(e)
        ));
        *cur = out;
    };
    for (li, tlay) in tl.layers.iter().enumerate() {
        let TLayer { start, end, layer, .. } = tlay;
        let (s, e) = (tl.secs(*start), tl.secs(*end));
        let len = e - s;
        let lab = format!("l{li}");
        match layer {
            Layer::Image { asset, look, .. } | Layer::Video { asset, look, .. } | Layer::Broll { asset, look, .. }
                if media.get(asset).is_some() =>
            {
                let m = media.get(asset).unwrap();
                let is_broll = matches!(layer, Layer::Broll { .. });
                let kb = matches!(
                    layer,
                    Layer::Image { ken_burns: Some(true), .. } | Layer::Broll { ken_burns: Some(true), .. }
                );
                let path = m.path.to_string_lossy().to_string();
                let idx = *next_input;
                *next_input += 1;
                match m.facts.kind {
                    AssetKind::Image => o.input_args.extend(
                        ["-loop", "1", "-framerate", &fps.ff(), "-t", &f(len + 0.1), "-i", &path].map(String::from),
                    ),
                    AssetKind::ImageSequence => {
                        let sf = m.facts.fps.unwrap_or(fps);
                        o.input_args.extend(["-framerate", &sf.ff(), "-i", &path].map(String::from));
                    }
                    _ => {
                        let src_in = match layer {
                            Layer::Video { src_in, .. } | Layer::Broll { src_in, .. } => {
                                src_in.map(|t| t.seconds(fps, m.facts.duration)).unwrap_or(0.0)
                            }
                            _ => 0.0,
                        };
                        if m.info
                            .as_ref()
                            .and_then(|i| i.video.as_ref())
                            .map(|v| v.needs_libvpx_decoder)
                            .unwrap_or(false)
                        {
                            o.input_args.extend(["-c:v", "libvpx-vp9"].map(String::from));
                        }
                        o.input_args.extend(["-ss", &f(src_in), "-t", &f(len + 0.1), "-i", &path].map(String::from));
                    }
                }
                let (srcw, srch) = (m.facts.width.max(2), m.facts.height.max(2));
                let fit = look.fit.unwrap_or(if is_broll && look.scale.is_none() { Fit::Cover } else { Fit::Contain });
                let full_frame = is_broll && look.scale.is_none();
                let mut ch: Vec<String> = vec![format!("fps={}", fps.ff()), "setpts=PTS-STARTPTS".into()];
                let (ow, oh);
                if full_frame {
                    ow = w;
                    oh = h;
                    match fit {
                        Fit::Cover => ch.push(format!("scale={w}:{h}:force_original_aspect_ratio=increase:force_divisible_by=2,crop={w}:{h}")),
                        Fit::Contain => ch.push(format!("scale={w}:{h}:force_original_aspect_ratio=decrease:force_divisible_by=2,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black@0")),
                        Fit::Stretch => ch.push(format!("scale={w}:{h}")),
                    }
                } else {
                    let default_scale = match layer {
                        Layer::Image { .. } => 0.25,
                        Layer::Video { .. } if m.facts.alpha => 1.0,
                        _ => 0.5,
                    };
                    let sc = look.scale.unwrap_or(default_scale);
                    ow = ((w as f64 * sc).round() as u32).max(2) & !1;
                    oh = ((ow as f64 * srch as f64 / srcw as f64).round() as u32).max(2) & !1;
                    ch.push(format!("scale={ow}:{oh}:flags=lanczos"));
                }
                ch.push(format!(
                    "scale=w=iw:h=ih:{}:out_color_matrix=bt709:out_range=tv,format=yuva444p",
                    crate::segment::color_in(m)
                ));
                if kb {
                    ch.push(format!(
                        "scale=w='{ow}+2*round({ow}*0.12*t/{}/2)':h='{oh}+2*round({oh}*0.12*t/{}/2)':eval=frame,crop={ow}:{oh}",
                        f(len.max(0.1)),
                        f(len.max(0.1))
                    ));
                }
                if let Some(op) = look.opacity {
                    if op < 1.0 {
                        ch.push(format!("colorchannelmixer=aa={}", f(op)));
                    }
                }
                lines.push(format!("[{idx}:v]{}[{lab}a]", ch.join(",")));
                let mut cur_l = format!("{lab}a");
                // PiP styling: rounded corners, border, shadow
                let radius = look.radius.unwrap_or(0.0) * k;
                let border = look.border.unwrap_or(0.0) * k;
                if !full_frame && (radius > 0.0 || border > 0.0 || look.shadow) {
                    if radius > 0.0 {
                        let mp = work.join(format!("mask_{li}.png"));
                        o.files.push((mp.clone(), rounded_mask_png(ow, oh, radius)));
                        let mi = *next_input;
                        *next_input += 1;
                        o.input_args.extend(
                            ["-loop", "1", "-framerate", &fps.ff(), "-t", &f(len + 0.1), "-i", &mp.to_string_lossy()]
                                .map(String::from),
                        );
                        lines.push(format!("[{mi}:v]format=gray,setpts=PTS-STARTPTS[{lab}m]"));
                        lines.push(format!("[{cur_l}][{lab}m]alphamerge[{lab}r]"));
                        cur_l = format!("{lab}r");
                    }
                    if border > 0.0 || look.shadow {
                        let bc = rgb_of(look.border_color.as_deref().unwrap_or("#FFFFFF"));
                        let (png, _, _, off) = frame_png(ow, oh, radius, border, bc, look.shadow);
                        let fp = work.join(format!("frame_{li}.png"));
                        o.files.push((fp.clone(), png));
                        let fi = *next_input;
                        *next_input += 1;
                        o.input_args.extend(
                            ["-loop", "1", "-framerate", &fps.ff(), "-t", &f(len + 0.1), "-i", &fp.to_string_lossy()]
                                .map(String::from),
                        );
                        lines.push(format!("[{fi}:v]format=yuva444p,setpts=PTS-STARTPTS[{lab}f]"));
                        lines.push(format!("[{lab}f][{cur_l}]overlay={off}:{off}:format=auto:shortest=1[{lab}b]"));
                        cur_l = format!("{lab}b");
                    }
                }
                if let Some(rot) = look.rotation {
                    if rot.abs() > 1e-6 {
                        lines.push(format!(
                            "[{cur_l}]rotate=a={}*PI/180:c=none:ow='rotw({}*PI/180)':oh='roth({}*PI/180)'[{lab}rot]",
                            f(rot),
                            f(rot),
                            f(rot)
                        ));
                        cur_l = format!("{lab}rot");
                    }
                }
                let an = anim_chain(look, len, ow);
                let mut fin = vec![];
                fin.extend(an);
                fin.push(format!("setpts=PTS+{}/TB", f(s)));
                lines.push(format!("[{cur_l}]{}[{lab}o]", fin.join(",")));
                let default_pos = match layer {
                    Layer::Image { .. } => "top_right",
                    _ => "center",
                };
                let (x0, y0) = if full_frame {
                    ("0".to_string(), "0".to_string())
                } else {
                    position_exprs(look.position.as_ref(), safe, default_pos)
                };
                let (dx, dy) = slide_offsets(look, s, e);
                push_overlay(lines, cur, &format!("{lab}o"), &format!("{x0}{dx}"), &format!("{y0}{dy}"), s, e);
            }
            Layer::Presenter {
                asset, src_in, shape, position, scale, border, border_color, shadow, layouts, ..
            } if media.get(asset).is_some() => {
                use cutline_core::model::{PresenterLayout, PresenterShape};
                let m = media.get(asset).unwrap();
                let (srcw, srch) = (m.facts.width.max(2) as f64, m.facts.height.max(2) as f64);
                let (fx, fy) = extras.presenter_focus.get(&tlay.index).copied().unwrap_or((0.5, 0.4));
                // layout over time: bubble until the first key, then each key's layout
                let body = tl.secs(tl.body_end - tl.origin);
                let mut segs: Vec<(f64, f64, PresenterLayout)> = vec![(s, e, PresenterLayout::Bubble)];
                for k in layouts {
                    let at = tl.from_edit(k.at.seconds(fps, body)).clamp(s, e);
                    if let Some(last) = segs.last_mut() {
                        last.1 = at;
                    }
                    segs.push((at, e, k.layout));
                }
                segs.retain(|g| g.1 - g.0 > 1e-6 && g.2 != PresenterLayout::ScreenFull);
                if segs.is_empty() {
                    continue;
                }
                let idx = *next_input;
                *next_input += 1;
                let from = src_in.map(|t| t.seconds(fps, m.facts.duration)).unwrap_or(0.0);
                o.input_args
                    .extend(["-ss", &f(from), "-t", &f(len + 0.1), "-i", &m.path.to_string_lossy()].map(String::from));
                let outs: String = (0..segs.len()).map(|i| format!("[{lab}s{i}]")).collect();
                lines.push(format!("[{idx}:v]fps={},setpts=PTS-STARTPTS,split={}{outs}", fps.ff(), segs.len()));
                let even = |v: f64| ((v.round() as u32).max(2)) & !1;
                for (i, (ss, se, layout)) in segs.iter().enumerate() {
                    let l2 = format!("{lab}p{i}");
                    // size, corner radius and place of the picture in this layout
                    let (ow, oh, radius, x0, y0, framed) = match layout {
                        PresenterLayout::PresenterFull => (w, h, 0.0, "0".to_string(), "0".to_string(), false),
                        PresenterLayout::SideBySide => {
                            let (ow, oh) = (even(0.32 * w as f64), even(safe.3.min(0.9 * h as f64)));
                            let x = (safe.0 + safe.2 - ow as f64).round();
                            let y = ((h as f64 - oh as f64) / 2.0).round();
                            (ow, oh, 0.035 * w.min(h) as f64, f(x), f(y), true)
                        }
                        _ => {
                            let sc = scale.unwrap_or(if h > w { 0.3 } else { 0.18 });
                            let side = even(w as f64 * sc);
                            let r = if shape.unwrap_or_default() == PresenterShape::Circle {
                                side as f64 / 2.0
                            } else {
                                0.14 * side as f64
                            };
                            let (x, y) = position_exprs(position.as_ref(), safe, "bottom_right");
                            (side, side, r, x, y, true)
                        }
                    };
                    // the largest crop of that shape, centred on the face
                    let aspect = ow as f64 / oh as f64;
                    let (cw, ch) = if srcw / srch > aspect { (srch * aspect, srch) } else { (srcw, srcw / aspect) };
                    let (cw, ch) = (even(cw).min(srcw as u32 & !1), even(ch).min(srch as u32 & !1));
                    let cx = (fx * srcw - cw as f64 / 2.0).clamp(0.0, srcw - cw as f64).round();
                    let cy = (fy * srch - ch as f64 / 2.0).clamp(0.0, srch - ch as f64).round();
                    lines.push(format!(
                        "[{lab}s{i}]crop={cw}:{ch}:{cx}:{cy},scale={ow}:{oh}:flags=lanczos,scale=w=iw:h=ih:{}:out_color_matrix=bt709:out_range=tv,format=yuva444p[{l2}a]",
                        crate::segment::color_in(m)
                    ));
                    let mut cur_l = format!("{l2}a");
                    let png_input = |o: &mut OverlayOut,
                                     next_input: &mut usize,
                                     name: String,
                                     bytes: Vec<u8>|
                     -> usize {
                        let p = work.join(name);
                        o.files.push((p.clone(), bytes));
                        let i = *next_input;
                        *next_input += 1;
                        o.input_args.extend(
                            ["-loop", "1", "-framerate", &fps.ff(), "-t", &f(len + 0.1), "-i", &p.to_string_lossy()]
                                .map(String::from),
                        );
                        i
                    };
                    let mut off = 0u32;
                    if radius > 0.0 {
                        let mi = png_input(
                            &mut o,
                            next_input,
                            format!("pmask_{li}_{i}.png"),
                            rounded_mask_png(ow, oh, radius),
                        );
                        lines.push(format!("[{mi}:v]format=gray,setpts=PTS-STARTPTS[{l2}m]"));
                        lines.push(format!("[{cur_l}][{l2}m]alphamerge[{l2}r]"));
                        cur_l = format!("{l2}r");
                    }
                    let bw = border.unwrap_or(4.0) * k;
                    let sh = shadow.unwrap_or(true);
                    if framed && (bw > 0.0 || sh) {
                        let bc = rgb_of(border_color.as_deref().unwrap_or("#FFFFFF"));
                        let (png, _, _, pad) = frame_png(ow, oh, radius, bw, bc, sh);
                        off = pad;
                        let fi = png_input(&mut o, next_input, format!("pframe_{li}_{i}.png"), png);
                        lines.push(format!("[{fi}:v]format=yuva444p,setpts=PTS-STARTPTS[{l2}f]"));
                        lines.push(format!("[{l2}f][{cur_l}]overlay={off}:{off}:format=auto:shortest=1[{l2}b]"));
                        cur_l = format!("{l2}b");
                    }
                    // a short fade where layouts change hands
                    let fd = 0.2f64.min((se - ss) / 2.0);
                    lines.push(format!(
                        "[{cur_l}]fade=t=in:st={}:d={}:alpha=1,fade=t=out:st={}:d={}:alpha=1,setpts=PTS+{}/TB[{l2}o]",
                        f(ss - s),
                        f(fd),
                        f(se - s - fd),
                        f(fd),
                        f(s)
                    ));
                    // the frame picture is larger than the camera by its padding
                    let (px, py) = if off > 0 { (format!("{x0}-{off}"), format!("{y0}-{off}")) } else { (x0, y0) };
                    push_overlay(lines, cur, &format!("{l2}o"), &px, &py, *ss, *se);
                }
            }
            Layer::Html { look, .. } | Layer::Text { look, .. } => {
                let Some(mz) = extras.html_renders.get(&tlay.index) else {
                    if matches!(layer, Layer::Html { .. }) {
                        tracing::warn!("html layer {} has no render; skipped", tlay.index);
                    }
                    continue; // ASS text templates are burned in below
                };
                let idx = *next_input;
                *next_input += 1;
                o.input_args.extend(["-i".to_string(), mz.to_string_lossy().to_string()]);
                let sc = look.scale.unwrap_or(1.0);
                let ow = ((w as f64 * sc).round() as u32).max(2) & !1;
                let mut ch = vec![
                    format!("fps={}", fps.ff()),
                    "setpts=PTS-STARTPTS".into(),
                    format!("scale={ow}:-2"),
                    "format=yuva444p".into(),
                ];
                if let Some(op) = look.opacity {
                    ch.push(format!("colorchannelmixer=aa={}", f(op)));
                }
                ch.extend(anim_chain(look, len, ow));
                ch.push(format!("setpts=PTS+{}/TB", f(s)));
                lines.push(format!("[{idx}:v]{}[{lab}o]", ch.join(",")));
                let (x0, y0) = position_exprs(look.position.as_ref(), safe, "center");
                let (dx, dy) = slide_offsets(look, s, e);
                push_overlay(lines, cur, &format!("{lab}o"), &format!("{x0}{dx}"), &format!("{y0}{dy}"), s, e);
            }
            Layer::ProgressBar { style, edge, height, .. } => {
                let col = match style.as_deref() {
                    None | Some("brand") => extras.accent.clone(),
                    Some(c) => ff_color(c),
                };
                let bh = ((height.unwrap_or(if tl.target.is_vertical() { 12 } else { 8 }) as f64 * k).round() as u32)
                    .max(2)
                    & !1;
                let y = if edge.as_deref() == Some("bottom") { format!("{}", h - bh) } else { "0".into() };
                let total = tl.duration();
                lines.push(format!(
                    "color=c={col}:s={w}x{bh}:r={}:d={},format=yuva444p,setpts=PTS+{}/TB[{lab}o]",
                    fps.ff(),
                    f(len),
                    f(s)
                ));
                push_overlay(lines, cur, &format!("{lab}o"), &format!("-W+W*t/{}", f(total)), &y, s, e);
            }
            Layer::Mask { rect, mode, color, strength, text, text_color, text_size, .. } => {
                mask_lines(
                    lines,
                    cur,
                    &lab,
                    MaskSpec {
                        rect: *rect,
                        mode: mode.unwrap_or_default(),
                        color: color.as_deref(),
                        strength: strength.unwrap_or(1.0),
                        text: text.as_deref(),
                        text_color: text_color.as_deref(),
                        text_size: text_size.map(|v| v * k),
                    },
                    (w, h),
                    (s, e),
                    extras.fonts_dir.as_deref(),
                );
            }
            _ => {}
        }
    }
    // brand logo watermark: over footage and overlays, under text and captions
    if let Some(wm) = extras.watermark.as_ref().filter(|m| m.end > m.start) {
        let idx = *next_input;
        *next_input += 1;
        let len = wm.end - wm.start;
        o.input_args.extend(
            ["-loop", "1", "-framerate", &fps.ff(), "-t", &f(len + 0.1), "-i", &wm.path.to_string_lossy()]
                .map(String::from),
        );
        let ow = ((w as f64 * wm.scale).round() as u32).max(2) & !1;
        lines.push(format!(
            "[{idx}:v]fps={},setpts=PTS-STARTPTS,scale={ow}:-2:flags=lanczos,format=yuva444p,colorchannelmixer=aa={},setpts=PTS+{}/TB[wm]",
            fps.ff(),
            f(wm.opacity.clamp(0.0, 1.0)),
            f(wm.start)
        ));
        let pos = Position::Preset(wm.position.clone());
        let (x0, y0) = position_exprs(Some(&pos), safe, "top_right");
        push_overlay(lines, cur, "wm", &x0, &y0, wm.start, wm.end);
    }
    // annotations (highlight, spotlight, arrow, keys, cursor): drawn in Rust, composited here
    for (i, an) in extras.annot.iter().enumerate() {
        let idx = *next_input;
        *next_input += 1;
        o.input_args.extend(["-i".to_string(), an.path.to_string_lossy().to_string()]);
        let lab = format!("an{i}");
        lines.push(format!("[{idx}:v]fps={},format=yuva444p,setpts=PTS-STARTPTS+{}/TB[{lab}]", fps.ff(), f(an.start)));
        push_overlay(lines, cur, &lab, "0", "0", an.start, an.end);
    }
    // template images (emoji pops)
    for (i, im) in extras.extra_images.iter().enumerate() {
        let idx = *next_input;
        *next_input += 1;
        let len = im.end - im.start;
        o.input_args.extend(
            ["-loop", "1", "-framerate", &fps.ff(), "-t", &f(len + 0.1), "-i", &im.path.to_string_lossy()]
                .map(String::from),
        );
        let ow = ((im.w * k).round() as u32).max(2) & !1;
        let look = Visual {
            enter: im.pop.then_some(Anim::Pop),
            exit: Some(Anim::Fade),
            anim_duration: Some(0.25),
            ..Default::default()
        };
        let mut ch = vec![
            format!("fps={}", fps.ff()),
            "setpts=PTS-STARTPTS".into(),
            format!("scale={ow}:-2"),
            "format=yuva444p".into(),
        ];
        ch.extend(anim_chain(&look, len, ow));
        ch.push(format!("setpts=PTS+{}/TB", f(im.start)));
        let lab = format!("em{i}");
        lines.push(format!("[{idx}:v]{}[{lab}]", ch.join(",")));
        push_overlay(
            lines,
            cur,
            &lab,
            &format!("{}-w/2", f(im.cx * k)),
            &format!("{}-h/2", f(im.cy * k)),
            im.start,
            im.end,
        );
    }
    // ASS text/captions on top; libass only sees the bundled fonts (fontconfig isolated)
    for (i, (name, content)) in extras.ass_docs.iter().enumerate() {
        let path = work.join(name);
        o.scripts.push((path.clone(), content.clone()));
        let fonts = extras.fonts_dir.as_ref().map(|d| format!(":fontsdir={}", filter_path(d))).unwrap_or_default();
        let out = format!("ass{i}");
        lines.push(format!(
            "[{cur}]subtitles=filename={}{fonts}:original_size={}x{}[{out}]",
            filter_path(&path),
            full.width,
            full.height
        ));
        *cur = out;
    }
    let _ = p;
    Ok(o)
}
