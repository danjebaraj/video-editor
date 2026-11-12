//! YuNet (OpenCV Zoo `face_detection_yunet_2023mar.onnx`) through ONNX Runtime.
//!
//! Input `input` [1,3,H,W] float32 BGR 0–255 (H, W multiples of 32). Outputs per stride s ∈
//! {8,16,32}: `cls_s` [1,N,1], `obj_s` [1,N,1], `bbox_s` [1,N,4], `kps_s` [1,N,10] with
//! N = (H/s)·(W/s). Decoding follows OpenCV's FaceDetectorYN (anchor-free, 2023mar).

use crate::{Detector, Face};
use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> = std::sync::OnceLock::new();

/// One ONNX Runtime environment per process (creating a second one fails).
fn env() -> Result<&'static ort::environment::Environment> {
    match ENV.get_or_init(|| ort::init().with_name("cutline").build().map_err(|e| format!("onnxruntime init: {e}"))) {
        Ok(e) => Ok(e),
        Err(e) => Err(anyhow::anyhow!("{e}")),
    }
}

pub struct YuNet {
    session: Session,
    pub score_threshold: f32,
    pub nms_threshold: f32,
    /// Model input size (fixed for 2023mar: 640×640); frames are letterboxed into it.
    pub input_w: usize,
    pub input_h: usize,
}

impl YuNet {
    pub fn load(path: &Path) -> Result<Self> {
        let session = Session::builder(env()?)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .commit_from_file(path)
            .map_err(|e| anyhow::anyhow!("loading {}: {e}", path.display()))?;
        let (mut w, mut h) = (640usize, 640usize);
        if let Some(i) = session.inputs().first() {
            if let ort::value::ValueType::Tensor { shape, .. } = i.dtype() {
                if shape.len() == 4 && shape[2] > 0 && shape[3] > 0 {
                    h = shape[2] as usize;
                    w = shape[3] as usize;
                }
            }
        }
        Ok(Self { session, score_threshold: 0.6, nms_threshold: 0.3, input_w: w, input_h: h })
    }

    /// Input/output names and shapes (debugging).
    pub fn describe(&self) -> String {
        let f = |o: &ort::value::Outlet| format!("{}: {:?}", o.name(), o.dtype());
        format!(
            "inputs: {}\noutputs: {}",
            self.session.inputs().iter().map(f).collect::<Vec<_>>().join("; "),
            self.session.outputs().iter().map(f).collect::<Vec<_>>().join("; ")
        )
    }
}

fn iou(a: &Face, b: &Face) -> f64 {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.w).min(b.x + b.w);
    let y2 = (a.y + a.h).min(b.y + b.h);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    inter / (a.w * a.h + b.w * b.h - inter).max(1e-12)
}

pub fn nms(mut faces: Vec<Face>, thr: f64) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    let mut keep: Vec<Face> = vec![];
    for f in faces {
        if keep.iter().all(|k| iou(k, &f) <= thr) {
            keep.push(f);
        }
    }
    keep
}

impl Detector for YuNet {
    fn id(&self) -> String {
        format!("yunet-2023mar-{}x{}", self.input_w, self.input_h)
    }

    fn detect(&mut self, rgb: &[u8], w: u32, h: u32) -> Result<Vec<Face>> {
        let (iw, ih) = (self.input_w, self.input_h);
        // letterbox: scale to fit, pad bottom/right
        let s = (iw as f64 / w as f64).min(ih as f64 / h as f64);
        let (sw, sh) = (((w as f64 * s) as usize).max(1), ((h as f64 * s) as usize).max(1));
        let mut data = vec![0f32; 3 * iw * ih];
        for y in 0..sh {
            let sy = ((y as f64 + 0.5) / s) as usize;
            for x in 0..sw {
                let sx = ((x as f64 + 0.5) / s) as usize;
                let i = (sy.min(h as usize - 1) * w as usize + sx.min(w as usize - 1)) * 3;
                // BGR planes
                data[y * iw + x] = rgb[i + 2] as f32;
                data[iw * ih + y * iw + x] = rgb[i + 1] as f32;
                data[2 * iw * ih + y * iw + x] = rgb[i] as f32;
            }
        }
        let input = Tensor::from_array(([1usize, 3, ih, iw], data)).map_err(|e| anyhow::anyhow!("{e}"))?;
        let outputs =
            self.session.run(ort::inputs!["input" => input]).map_err(|e| anyhow::anyhow!("yunet inference: {e}"))?;
        let mut faces = vec![];
        for stride in [8usize, 16, 32] {
            let get = |n: &str| -> Result<Vec<f32>> {
                let v = outputs.get(n).with_context(|| format!("missing output {n}"))?;
                let (_, d) = v.try_extract_tensor::<f32>().map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(d.to_vec())
            };
            let cls = get(&format!("cls_{stride}"))?;
            let obj = get(&format!("obj_{stride}"))?;
            let bbox = get(&format!("bbox_{stride}"))?;
            let cols = iw / stride;
            let rows = ih / stride;
            for r in 0..rows {
                for c in 0..cols {
                    let idx = r * cols + c;
                    let score = (cls[idx].clamp(0.0, 1.0) * obj[idx].clamp(0.0, 1.0)).sqrt();
                    if score < self.score_threshold {
                        continue;
                    }
                    let cx = (c as f32 + bbox[idx * 4]) * stride as f32;
                    let cy = (r as f32 + bbox[idx * 4 + 1]) * stride as f32;
                    let bw = bbox[idx * 4 + 2].exp() * stride as f32;
                    let bh = bbox[idx * 4 + 3].exp() * stride as f32;
                    let (x0, y0) = (cx - bw / 2.0, cy - bh / 2.0);
                    // back to normalized source coordinates
                    let fx = x0 as f64 / s / w as f64;
                    let fy = y0 as f64 / s / h as f64;
                    faces.push(Face {
                        x: fx,
                        y: fy,
                        w: bw as f64 / s / w as f64,
                        h: bh as f64 / s / h as f64,
                        score: score as f64,
                    });
                }
            }
        }
        Ok(nms(faces, self.nms_threshold as f64))
    }
}
