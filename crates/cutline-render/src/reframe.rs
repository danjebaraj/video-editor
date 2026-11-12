//! Per-clip reframe: track_face / manual / split → geometry overrides (sendcmd crop paths).

use crate::pipeline::Loaded;
use crate::segment::ClipExtras;
use crate::Geometry;
use anyhow::Result;
use cutline_core::diag::Diagnostic;
use cutline_core::model::{Project, ReframeMode};
use cutline_core::timeline::{TClip, Timeline};
use cutline_media::cache::Cache;
use cutline_reframe::track::{SmoothParams, Subject};
use cutline_reframe::{Analysis, CropPath, TrackOptions};

pub const ANALYSIS_RATE: f64 = 6.0;
pub const ANALYSIS_WIDTH: u32 = 640;

fn cache(l: &Loaded) -> Cache {
    Cache::new(crate::cutline_dir(&l.dir).join("cache").join("reframe"))
}

/// Face analysis for a clip's source range (cached by fingerprint + range + detector).
pub fn analysis(l: &Loaded, c: &TClip) -> Result<Option<Analysis>> {
    let Some(m) = l.media.get(&c.asset) else { return Ok(None) };
    let Some(mut det) = cutline_reframe::default_detector() else { return Ok(None) };
    let key = cutline_media::hash_key(&[
        m.fingerprint.as_bytes(),
        format!("{:.4}-{:.4}-{ANALYSIS_RATE}-{ANALYSIS_WIDTH}", c.src_in, c.src_out).as_bytes(),
        det.id().as_bytes(),
        b"faces-v1",
    ]);
    let cc = cache(l);
    if let Some(a) = cc.get_json::<Analysis>(&key) {
        return Ok(Some(a));
    }
    let tc = cutline_deps::toolchain()?;
    let a = cutline_reframe::analyze(tc, &m.path, c.src_in, c.src_out, det.as_mut(), ANALYSIS_RATE, ANALYSIS_WIDTH)?;
    cc.put_json(&key, &a)?;
    Ok(Some(a))
}

fn write_cmd(l: &Loaded, path: &CropPath) -> Result<std::path::PathBuf> {
    let text = path.sendcmd();
    let key = cutline_media::hash_key(&[text.as_bytes(), b"sendcmd-v1"]);
    let p = cache(l).root.join(format!("{key}.cmd"));
    if !p.exists() {
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, text)?;
    }
    Ok(p)
}

/// Fill `x` for clips whose reframe needs analysis or keyframes.
pub fn apply(
    l: &Loaded,
    p: &Project,
    tl: &Timeline,
    g: &Geometry,
    c: &TClip,
    x: &mut ClipExtras,
    warnings: &mut Vec<Diagnostic>,
) -> Result<Option<CropPath>> {
    let rf = c.clip.reframe.clone().or_else(|| p.default_reframe.clone()).unwrap_or_default();
    let Some(m) = l.media.get(&c.asset) else { return Ok(None) };
    let (sw, sh) = (m.facts.width, m.facts.height);
    if sw == 0 || sh == 0 {
        return Ok(None);
    }
    let aspect = g.width as f64 / g.height as f64;
    let fps = tl.fps.as_f64();
    let n = c.frames as usize;
    let fallback = |x: &mut ClipExtras, warnings: &mut Vec<Diagnostic>, why: &str| {
        let fb = rf.fallback.unwrap_or(ReframeMode::FitBlur);
        x.mode_override = Some(fb);
        warnings.push(
            Diagnostic::warning(
                "W_REFRAME_NO_FACES",
                format!("/main/{}/reframe", c.main_index),
                format!("clip `{}`: {why}; using {fb:?}", c.id),
            )
            .hint("check the footage, choose `manual` keys, or another reframe mode"),
        );
    };
    let path = match rf.mode {
        ReframeMode::TrackFace => {
            let Some(an) = analysis(l, c)? else {
                fallback(x, warnings, &format!("face detector unavailable ({})", cutline_reframe::detector_status()));
                return Ok(None);
            };
            let bias = rf.bias.map(|b| (b.x, b.y)).unwrap_or((0.5, 0.38));
            let o = TrackOptions {
                subject: Subject::parse(rf.subject.as_deref(), rf.subject_index),
                bias,
                smooth: SmoothParams { max_speed: rf.max_pan_speed.unwrap_or(0.6), ..Default::default() },
                speed: c.speed,
            };
            match cutline_reframe::track_path(&an, n, fps, sw, sh, aspect, &o) {
                Some(p) => p,
                None => {
                    fallback(x, warnings, "no faces found");
                    return Ok(None);
                }
            }
        }
        ReframeMode::Manual => {
            let keys: Vec<(f64, f64, f64, f64)> = rf.keys.iter().map(|k| (k.t, k.x, k.y, k.zoom)).collect();
            cutline_reframe::manual_path(&keys, n, fps, sw, sh, aspect)
        }
        ReframeMode::Split => {
            let regions = if rf.regions.len() == 2 {
                Some([
                    (rf.regions[0].x, rf.regions[0].y, rf.regions[0].w, rf.regions[0].h),
                    (rf.regions[1].x, rf.regions[1].y, rf.regions[1].w, rf.regions[1].h),
                ])
            } else {
                analysis(l, c)?.and_then(|an| cutline_reframe::split_regions_from_faces(&an, sw, sh, g.width, g.height))
            };
            match regions {
                Some(r) => {
                    x.geometry_override = Some(cutline_reframe::split_geometry(r, sw, sh, g.width, g.height));
                    return Ok(None);
                }
                None => {
                    fallback(x, warnings, "split needs two faces or two `regions`");
                    return Ok(None);
                }
            }
        }
        _ => return Ok(None),
    };
    let cmd = write_cmd(l, &path)?;
    x.geometry_override = Some(path.geometry(&crate::escape::filter_path(&cmd), g.width, g.height));
    x.deps.push(cmd);
    Ok(Some(path))
}
