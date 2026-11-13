//! Auto-reframe: face detection (YuNet via ONNX Runtime behind the `onnx` feature, or an
//! injected detector), tracking, smoothing, scene-cut resets, and a per-frame crop path driven
//! through ffmpeg `sendcmd`.

pub mod track;
#[cfg(feature = "onnx")]
pub mod yunet;

use anyhow::{Context, Result};
use cutline_deps::Toolchain;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use track::{build_tracks, select, smooth, targets_per_frame, FrameDet, SmoothParams, Subject};

/// Face box in normalized source coordinates (0..1).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Face {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub score: f64,
}

impl Face {
    pub fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }
}

pub trait Detector {
    /// Identifier hashed into analysis cache keys.
    fn id(&self) -> String;
    /// Detect faces in an RGB24 frame; returns normalized boxes.
    fn detect(&mut self, rgb: &[u8], w: u32, h: u32) -> Result<Vec<Face>>;
}

/// Detector driven by a closure over the analysis frame index (tests).
pub struct FnDetector<F: FnMut(usize, &[u8], u32, u32) -> Vec<Face>> {
    pub name: String,
    pub f: F,
    n: usize,
}

impl<F: FnMut(usize, &[u8], u32, u32) -> Vec<Face>> FnDetector<F> {
    pub fn new(name: &str, f: F) -> Self {
        Self { name: name.into(), f, n: 0 }
    }
}

impl<F: FnMut(usize, &[u8], u32, u32) -> Vec<Face>> Detector for FnDetector<F> {
    fn id(&self) -> String {
        self.name.clone()
    }
    fn detect(&mut self, rgb: &[u8], w: u32, h: u32) -> Result<Vec<Face>> {
        let r = (self.f)(self.n, rgb, w, h);
        self.n += 1;
        Ok(r)
    }
}

/// The face model path (installed by `cutline deps install face-model`).
pub fn model_path() -> std::path::PathBuf {
    cutline_deps::paths::tools_dir().join("models").join("face_detection_yunet_2023mar.onnx")
}

type Factory = Box<dyn Fn() -> Box<dyn Detector> + Send + Sync>;
static FACTORY: std::sync::Mutex<Option<Factory>> = std::sync::Mutex::new(None);

/// Inject a detector factory (tests, alternative detectors). `None` restores the default.
pub fn set_detector_factory(f: Option<Factory>) {
    *FACTORY.lock().unwrap() = f;
}

/// Built-in detector when compiled with `onnx` and the model is installed (or the injected one).
pub fn default_detector() -> Option<Box<dyn Detector>> {
    if let Some(f) = FACTORY.lock().unwrap().as_ref() {
        return Some(f());
    }
    #[cfg(feature = "onnx")]
    {
        let m = std::env::var_os("CUTLINE_FACE_MODEL").map(std::path::PathBuf::from).unwrap_or_else(model_path);
        if m.exists() {
            match yunet::YuNet::load(&m) {
                Ok(d) => return Some(Box::new(d)),
                Err(e) => tracing::warn!("face model failed to load: {e:#}"),
            }
        }
    }
    None
}

pub fn detector_status() -> &'static str {
    if cfg!(feature = "onnx") {
        if model_path().exists() {
            "yunet"
        } else {
            "model missing"
        }
    } else {
        "not compiled (build with --features onnx)"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Analysis {
    /// Detections at clip-local source times.
    pub dets: Vec<FrameDet>,
    /// Scene cut times (clip-local source seconds).
    pub cuts: Vec<f64>,
    pub rate: f64,
}

/// Decode `[src_in, src_out)` at `rate` fps / `width` px and run the detector on every frame;
/// detect scene cuts with `scdet`.
pub fn analyze(
    tc: &Toolchain,
    path: &Path,
    src_in: f64,
    src_out: f64,
    det: &mut dyn Detector,
    rate: f64,
    width: u32,
) -> Result<Analysis> {
    let dur = src_out - src_in;
    let mut child = Command::new(&tc.ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-ss",
            &format!("{src_in:.6}"),
            "-t",
            &format!("{dur:.6}"),
            "-i",
        ])
        .arg(path)
        .args(["-vf", &format!("fps={rate},scale={width}:-2,format=rgb24"), "-f", "rawvideo", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning ffmpeg for analysis")?;
    // probe the analysis frame height from the source aspect
    let info = cutline_media::probe(tc, path, None)?;
    let v = info.video.context("no video stream")?;
    let h = (((width as f64) * v.display_height as f64 / v.display_width.max(1) as f64 / 2.0).round() * 2.0) as u32;
    let fsz = (width * h * 3) as usize;
    let mut so = child.stdout.take().unwrap();
    let mut dets = vec![];
    let mut i = 0usize;
    loop {
        let mut buf = vec![0u8; fsz];
        if so.read_exact(&mut buf).is_err() {
            break;
        }
        let faces = det.detect(&buf, width, h)?;
        dets.push(FrameDet { t: i as f64 / rate, faces });
        i += 1;
    }
    child.wait()?;
    let cuts = scene_cuts(tc, path, src_in, src_out)?;
    Ok(Analysis { dets, cuts, rate })
}

/// Scene cuts via `scdet` (clip-local seconds).
pub fn scene_cuts(tc: &Toolchain, path: &Path, src_in: f64, src_out: f64) -> Result<Vec<f64>> {
    let o = Command::new(&tc.ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-ss",
            &format!("{src_in:.6}"),
            "-t",
            &format!("{:.6}", src_out - src_in),
            "-i",
        ])
        .arg(path)
        .args(["-vf", "scale=320:-2,scdet=threshold=12", "-an", "-f", "null", "-"])
        .output()?;
    let s = String::from_utf8_lossy(&o.stderr);
    let mut v: Vec<f64> = s
        .lines()
        .filter_map(|l| {
            let p = l.find("lavfi.scd.time")?;
            l[p + 14..]
                .trim_start_matches([':', '=', ' '])
                .split(|c: char| c == ',' || c.is_whitespace())
                .next()?
                .parse()
                .ok()
        })
        .collect();
    v.dedup();
    Ok(v)
}

/// Per-output-frame crop rectangles in source pixels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CropPath {
    pub src_w: u32,
    pub src_h: u32,
    pub cw: Vec<u32>,
    pub ch: Vec<u32>,
    pub x: Vec<u32>,
    pub y: Vec<u32>,
    pub fps: f64,
    /// Manual zoom keys (t, zoom ≥ 1), eased in-out cubic; applied after the fixed-size crop.
    #[serde(default)]
    pub zoom_keys: Vec<(f64, f64)>,
}

impl CropPath {
    pub fn len(&self) -> usize {
        self.x.len()
    }
    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }
    /// Center x (normalized) at frame i.
    pub fn center_x(&self, i: usize) -> f64 {
        (self.x[i] as f64 + self.cw[i] as f64 / 2.0) / self.src_w as f64
    }
    /// `sendcmd` script driving `crop@rf` (only emits changes).
    pub fn sendcmd(&self) -> String {
        let mut s = String::new();
        let mut prev: Option<(u32, u32, u32, u32)> = None;
        for i in 0..self.len() {
            let cur = (self.cw[i], self.ch[i], self.x[i], self.y[i]);
            if prev != Some(cur) {
                let t = i as f64 / self.fps;
                let mut cmds = vec![];
                if prev.map(|p| p.0 != cur.0).unwrap_or(true) {
                    cmds.push(format!("crop@rf w {}", cur.0));
                }
                if prev.map(|p| p.1 != cur.1).unwrap_or(true) {
                    cmds.push(format!("crop@rf h {}", cur.1));
                }
                if prev.map(|p| p.2 != cur.2).unwrap_or(true) {
                    cmds.push(format!("crop@rf x {}", cur.2));
                }
                if prev.map(|p| p.3 != cur.3).unwrap_or(true) {
                    cmds.push(format!("crop@rf y {}", cur.3));
                }
                s.push_str(&format!("{t:.6} {};\n", cmds.join(", ")));
                prev = Some(cur);
            }
        }
        s
    }
    /// Geometry filter chain template (`{in}`, `{out}`, `{cin}` placeholders) for the segment.
    pub fn geometry(&self, sendcmd_path: &str, w: u32, h: u32) -> String {
        let zoom = self.zoom_expr().map(|z| format!(",scale=w='{w}+2*round({w}*(({z})-1)/2)':h='{h}+2*round({h}*(({z})-1)/2)':eval=frame:flags=bicubic,crop={w}:{h}:x='(iw-{w})/2':y='(ih-{h})/2'")).unwrap_or_default();
        format!(
            "[{{in}}]sendcmd=f={sendcmd_path},crop@rf=w={}:h={}:x={}:y={}:exact=1,scale={w}:{h}:flags=lanczos:{{cin}}:out_color_matrix=bt709:out_range=tv{zoom},setsar=1[{{out}}]",
            self.cw[0], self.ch[0], self.x[0], self.y[0]
        )
    }

    /// Piecewise in-out-cubic zoom expression of `t` (None when there is no zoom).
    pub fn zoom_expr(&self) -> Option<String> {
        if self.zoom_keys.iter().all(|(_, z)| (z - 1.0).abs() < 1e-6) {
            return None;
        }
        let k = &self.zoom_keys;
        let f = |v: f64| format!("{v:.6}");
        let mut e = f(k.last()?.1);
        for i in (0..k.len().saturating_sub(1)).rev() {
            let (t0, z0) = k[i];
            let (t1, z1) = k[i + 1];
            let u = format!("clip((t-{})/{},0,1)", f(t0), f((t1 - t0).max(1e-6)));
            let ease = format!("if(lt({u},0.5),4*pow({u},3),1-pow(-2*{u}+2,3)/2)");
            e = format!("if(lt(t,{}),{}+{}*{ease},{e})", f(t1), f(z0), f(z1 - z0));
        }
        Some(format!("if(lt(t,{}),{},{e})", f(k[0].0), f(k[0].1)))
    }
}

fn even(v: f64) -> u32 {
    ((v / 2.0).round() * 2.0).max(2.0) as u32
}

/// Largest crop with the target aspect inside the source.
pub fn crop_size(src_w: u32, src_h: u32, aspect: f64) -> (u32, u32) {
    if src_w as f64 / src_h as f64 > aspect {
        (even(src_h as f64 * aspect).min(src_w), src_h & !1)
    } else {
        (src_w & !1, even(src_w as f64 / aspect).min(src_h))
    }
}

#[derive(Debug, Clone)]
pub struct TrackOptions {
    pub subject: Subject,
    pub bias: (f64, f64),
    pub smooth: SmoothParams,
    /// Source seconds per output frame (speed / fps).
    pub speed: f64,
}

/// Build a face-tracking crop path for `n_frames` output frames. `None` when no faces.
pub fn track_path(
    an: &Analysis,
    n_frames: usize,
    fps: f64,
    src_w: u32,
    src_h: u32,
    aspect: f64,
    o: &TrackOptions,
) -> Option<CropPath> {
    let tracks = build_tracks(&an.dets, 0.12, 1.0);
    let tr = select(&tracks, o.subject)?;
    let (cw, ch) = crop_size(src_w, src_h, aspect);
    let (cwn, chn) = (cw as f64 / src_w as f64, ch as f64 / src_h as f64);
    // detection times are source-local; output frame i shows source time i/fps*speed
    let scaled: Vec<(f64, Face)> = tr.samples.iter().map(|(t, f)| (t / o.speed, *f)).collect();
    let tx = targets_per_frame(&scaled, n_frames, fps, 1.0);
    let ty: Vec<Option<f64>> = {
        let ys: Vec<(f64, Face)> = scaled.iter().map(|(t, f)| (*t, Face { x: f.cy() - f.w / 2.0, ..*f })).collect();
        targets_per_frame(&ys, n_frames, fps, 1.0)
    };
    let cut_frames: Vec<usize> =
        an.cuts.iter().map(|t| (t / o.speed * fps).round() as usize).filter(|i| *i > 0 && *i < n_frames).collect();
    let tx: Vec<Option<f64>> = tx.iter().map(|v| v.map(|x| x + (0.5 - o.bias.0) * cwn)).collect();
    let ty: Vec<Option<f64>> = ty.iter().map(|v| v.map(|y| y + (0.5 - o.bias.1) * chn)).collect();
    let xs = smooth(&tx, fps, &o.smooth, &cut_frames);
    let ys = smooth(
        &ty,
        fps,
        &SmoothParams { max_speed: o.smooth.max_speed * src_w as f64 / src_h as f64, ..o.smooth },
        &cut_frames,
    );
    let px = |c: f64, cn: f64, full: u32, size: u32| -> u32 {
        let c = track::clamp_center(c, cn);
        let v = (c - cn / 2.0) * full as f64;
        (even(v).min(full - size)).min(full.saturating_sub(size))
    };
    Some(CropPath {
        src_w,
        src_h,
        cw: vec![cw; n_frames],
        ch: vec![ch; n_frames],
        x: xs.iter().map(|x| if cw >= src_w { 0 } else { px(*x, cwn, src_w, cw) }).collect(),
        y: ys.iter().map(|y| if ch >= src_h { 0 } else { px(*y, chn, src_h, ch) }).collect(),
        fps,
        zoom_keys: vec![],
    })
}

/// Manual keyframes `(t, x, y, zoom)` (clip-relative timeline seconds, normalized center,
/// zoom ≥ 1 shrinks the crop), eased in-out cubic between keys.
pub fn manual_path(
    keys: &[(f64, f64, f64, f64)],
    n_frames: usize,
    fps: f64,
    src_w: u32,
    src_h: u32,
    aspect: f64,
) -> CropPath {
    let (cw0, ch0) = crop_size(src_w, src_h, aspect);
    let mut ks = keys.to_vec();
    ks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let ease = |u: f64| if u < 0.5 { 4.0 * u * u * u } else { 1.0 - (-2.0 * u + 2.0).powi(3) / 2.0 };
    let at = |t: f64| -> (f64, f64, f64) {
        if ks.is_empty() {
            return (0.5, 0.5, 1.0);
        }
        if t <= ks[0].0 {
            return (ks[0].1, ks[0].2, ks[0].3);
        }
        for w in ks.windows(2) {
            if t <= w[1].0 {
                let u = ease((t - w[0].0) / (w[1].0 - w[0].0).max(1e-9));
                return (
                    w[0].1 + (w[1].1 - w[0].1) * u,
                    w[0].2 + (w[1].2 - w[0].2) * u,
                    w[0].3 + (w[1].3 - w[0].3) * u,
                );
            }
        }
        let l = ks.last().unwrap();
        (l.1, l.2, l.3)
    };
    // fixed crop window (zoom 1) positioned by the key centers; zoom is applied after the crop
    // (changing crop size at runtime stalls ffmpeg's filter graph)
    let mut p = CropPath {
        src_w,
        src_h,
        cw: vec![],
        ch: vec![],
        x: vec![],
        y: vec![],
        fps,
        zoom_keys: ks.iter().map(|k| (k.0, k.3.max(1.0))).collect(),
    };
    for i in 0..n_frames {
        let (cx, cy, _z) = at(i as f64 / fps);
        let (cw, ch) = (cw0, ch0);
        let x = even((cx * src_w as f64 - cw as f64 / 2.0).clamp(0.0, (src_w - cw) as f64));
        let y = even((cy * src_h as f64 - ch as f64 / 2.0).clamp(0.0, (src_h - ch) as f64));
        p.cw.push(cw);
        p.ch.push(ch);
        p.x.push(x.min(src_w - cw));
        p.y.push(y.min(src_h - ch));
    }
    p
}

/// Split layout: two source regions (normalized x,y,w,h) stacked into the target.
pub fn split_geometry(regions: [(f64, f64, f64, f64); 2], src_w: u32, src_h: u32, w: u32, h: u32) -> String {
    let half = (h / 2) & !1;
    let mut parts = vec![];
    for (i, (x, y, rw, rh)) in regions.iter().enumerate() {
        let (cw, ch) = (even(rw * src_w as f64).min(src_w), even(rh * src_h as f64).min(src_h));
        let (cx, cy) = (even(x * src_w as f64).min(src_w - cw), even(y * src_h as f64).min(src_h - ch));
        parts.push(format!(
            "[s{i}]crop={cw}:{ch}:{cx}:{cy},scale={w}:{half}:force_original_aspect_ratio=increase:flags=lanczos:{{cin}}:out_color_matrix=bt709:out_range=tv,crop={w}:{half},setsar=1[p{i}]"
        ));
    }
    format!("[{{in}}]split=2[s0][s1];{};[p0][p1]vstack=inputs=2,scale={w}:{h},setsar=1[{{out}}]", parts.join(";"))
}

/// Split regions centered on the two most persistent faces (left one on top).
pub fn split_regions_from_faces(
    an: &Analysis,
    src_w: u32,
    src_h: u32,
    w: u32,
    h: u32,
) -> Option<[(f64, f64, f64, f64); 2]> {
    let tracks = build_tracks(&an.dets, 0.12, 1.0);
    let mut ts: Vec<&track::Track> = tracks.iter().collect();
    ts.sort_by_key(|t| std::cmp::Reverse(t.samples.len()));
    if ts.len() < 2 || ts[1].samples.len() * 3 < ts[0].samples.len() {
        return None;
    }
    let mut two = [ts[0], ts[1]];
    two.sort_by(|a, b| a.mean(|f| f.cx()).partial_cmp(&b.mean(|f| f.cx())).unwrap());
    let region_aspect = w as f64 / (h as f64 / 2.0);
    let rh = 0.8f64;
    let rw = (rh * src_h as f64 * region_aspect / src_w as f64).min(1.0);
    Some(two.map(|t| {
        let cx = t.mean(|f| f.cx());
        let cy = t.mean(|f| f.cy());
        ((cx - rw / 2.0).clamp(0.0, 1.0 - rw), (cy - rh * 0.42).clamp(0.0, 1.0 - rh), rw, rh)
    }))
}
