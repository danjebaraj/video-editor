//! Subject selection, path smoothing and crop path generation (pure logic, unit-testable).

use crate::Face;
use serde::{Deserialize, Serialize};

/// Detections at one analysis time (seconds, clip-local source time).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrameDet {
    pub t: f64,
    pub faces: Vec<Face>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    MostPersistent,
    Largest,
    Left,
    Right,
    Index(usize),
}

impl Subject {
    pub fn parse(s: Option<&str>, idx: Option<usize>) -> Self {
        match s {
            Some("largest") => Subject::Largest,
            Some("left") => Subject::Left,
            Some("right") => Subject::Right,
            Some("index") => Subject::Index(idx.unwrap_or(0)),
            _ => Subject::MostPersistent,
        }
    }
}

/// A face track across analysis frames.
#[derive(Debug, Clone)]
pub struct Track {
    pub id: usize,
    /// (time, face) samples.
    pub samples: Vec<(f64, Face)>,
}

impl Track {
    pub fn mean(&self, f: impl Fn(&Face) -> f64) -> f64 {
        self.samples.iter().map(|(_, x)| f(x)).sum::<f64>() / self.samples.len().max(1) as f64
    }
}

/// Greedy association of detections into tracks (nearest center within `max_jump` of frame
/// width, tracks die after `max_gap` seconds without a match).
pub fn build_tracks(dets: &[FrameDet], max_jump: f64, max_gap: f64) -> Vec<Track> {
    let mut tracks: Vec<Track> = vec![];
    for d in dets {
        let mut used = vec![false; d.faces.len()];
        let mut order: Vec<usize> = (0..tracks.len()).collect();
        order.sort_by(|a, b| tracks[*b].samples.len().cmp(&tracks[*a].samples.len()));
        for ti in order {
            let (lt, lf) = *tracks[ti].samples.last().unwrap();
            if d.t - lt > max_gap {
                continue;
            }
            let mut best: Option<(usize, f64)> = None;
            for (fi, f) in d.faces.iter().enumerate() {
                if used[fi] {
                    continue;
                }
                let dist = ((f.cx() - lf.cx()).powi(2) + (f.cy() - lf.cy()).powi(2)).sqrt();
                if dist <= max_jump && best.map(|b| dist < b.1).unwrap_or(true) {
                    best = Some((fi, dist));
                }
            }
            if let Some((fi, _)) = best {
                used[fi] = true;
                tracks[ti].samples.push((d.t, d.faces[fi]));
            }
        }
        for (fi, f) in d.faces.iter().enumerate() {
            if !used[fi] {
                let id = tracks.len();
                tracks.push(Track { id, samples: vec![(d.t, *f)] });
            }
        }
    }
    tracks
}

pub fn select(tracks: &[Track], s: Subject) -> Option<&Track> {
    if tracks.is_empty() {
        return None;
    }
    // ignore tiny spurious tracks unless nothing else exists
    let longest = tracks.iter().map(|t| t.samples.len()).max().unwrap_or(0);
    let cands: Vec<&Track> = tracks.iter().filter(|t| t.samples.len() * 5 >= longest).collect();
    match s {
        Subject::MostPersistent => cands
            .iter()
            .max_by(|a, b| {
                (a.samples.len(), a.mean(|f| f.w * f.h)).partial_cmp(&(b.samples.len(), b.mean(|f| f.w * f.h))).unwrap()
            })
            .copied(),
        Subject::Largest => {
            cands.iter().max_by(|a, b| a.mean(|f| f.w * f.h).partial_cmp(&b.mean(|f| f.w * f.h)).unwrap()).copied()
        }
        Subject::Left => {
            cands.iter().min_by(|a, b| a.mean(|f| f.cx()).partial_cmp(&b.mean(|f| f.cx())).unwrap()).copied()
        }
        Subject::Right => {
            cands.iter().max_by(|a, b| a.mean(|f| f.cx()).partial_cmp(&b.mean(|f| f.cx())).unwrap()).copied()
        }
        Subject::Index(i) => {
            let mut v = cands.clone();
            v.sort_by(|a, b| a.samples[0].1.cx().partial_cmp(&b.samples[0].1.cx()).unwrap());
            v.get(i).copied().or(v.first().copied())
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SmoothParams {
    /// Don't move when the target is within this fraction of the frame width.
    pub dead_zone: f64,
    /// Spring natural frequency (rad/s); critically damped.
    pub omega: f64,
    /// Max pan speed in frame widths per second.
    pub max_speed: f64,
    /// Seconds without detections before drifting to center.
    pub lost_timeout: f64,
    /// Drift-to-center speed (frame widths / s).
    pub drift_speed: f64,
}

impl Default for SmoothParams {
    fn default() -> Self {
        Self { dead_zone: 0.04, omega: 6.0, max_speed: 0.6, lost_timeout: 1.5, drift_speed: 0.08 }
    }
}

/// Smooth a target path (normalized x center per output frame; `None` = no detection) into a
/// camera path. `cuts` are frame indices where the camera snaps (scene cuts).
pub fn smooth(targets: &[Option<f64>], fps: f64, p: &SmoothParams, cuts: &[usize]) -> Vec<f64> {
    let dt = 1.0 / fps;
    let mut out = Vec::with_capacity(targets.len());
    let first = targets.iter().flatten().next().copied().unwrap_or(0.5);
    let (mut x, mut v) = (first, 0.0f64);
    let mut goal = first;
    let mut lost_for = 0.0;
    let mut settled = true;
    for (i, t) in targets.iter().enumerate() {
        if cuts.contains(&i) {
            // reset at scene cuts: snap to the next known target
            let next = targets[i..].iter().flatten().next().copied().unwrap_or(0.5);
            x = next;
            v = 0.0;
            goal = next;
            lost_for = 0.0;
            settled = true;
        }
        match t {
            Some(tx) => {
                lost_for = 0.0;
                // dead zone with hysteresis: start moving only when the subject leaves the zone;
                // once moving, follow until settled close to the target
                if (tx - goal).abs() > p.dead_zone || !settled {
                    goal = *tx;
                }
            }
            None => {
                lost_for += dt;
                if lost_for > p.lost_timeout {
                    // drift toward center slowly
                    let step = p.drift_speed * dt;
                    goal = if (goal - 0.5).abs() <= step { 0.5 } else { goal - step * (goal - 0.5).signum() };
                }
            }
        }
        // critically damped spring toward goal
        let w = p.omega;
        let acc = w * w * (goal - x) - 2.0 * w * v;
        v += acc * dt;
        v = v.clamp(-p.max_speed, p.max_speed);
        x += v * dt;
        settled = (goal - x).abs() < p.dead_zone * 0.25 && v.abs() < 0.02;
        out.push(x);
    }
    out
}

/// Clamp a normalized crop center so the crop (width `cw` as fraction of source) stays inside.
pub fn clamp_center(x: f64, cw: f64) -> f64 {
    x.clamp(cw / 2.0, 1.0 - cw / 2.0)
}

/// Interpolate detection targets (normalized subject x) onto output frames.
/// Samples farther than `max_gap` seconds from any detection are `None`.
pub fn targets_per_frame(track: &[(f64, Face)], n_frames: usize, fps: f64, max_gap: f64) -> Vec<Option<f64>> {
    let mut out = vec![None; n_frames];
    if track.is_empty() {
        return out;
    }
    for (i, slot) in out.iter_mut().enumerate() {
        let t = i as f64 / fps;
        let k = track.partition_point(|(tt, _)| *tt <= t);
        let prev = k.checked_sub(1).map(|j| track[j]);
        let next = track.get(k).copied();
        let v = match (prev, next) {
            (Some((t0, f0)), Some((t1, f1))) if t1 - t0 <= max_gap => {
                let u = (t - t0) / (t1 - t0).max(1e-9);
                Some(f0.cx() + (f1.cx() - f0.cx()) * u)
            }
            (Some((t0, f0)), _) if t - t0 <= max_gap / 2.0 => Some(f0.cx()),
            (_, Some((t1, f1))) if t1 - t <= max_gap / 2.0 => Some(f1.cx()),
            _ => None,
        };
        *slot = v;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn face(cx: f64) -> Face {
        Face { x: cx - 0.05, y: 0.3, w: 0.1, h: 0.15, score: 0.9 }
    }

    #[test]
    fn dead_zone_holds_still() {
        // subject wobbles ±1 % around 0.5: camera must not move
        let t: Vec<Option<f64>> = (0..90).map(|i| Some(0.5 + 0.01 * ((i as f64) * 0.7).sin())).collect();
        let x = smooth(&t, 30.0, &SmoothParams::default(), &[]);
        let spread = x.iter().cloned().fold(f64::MIN, f64::max) - x.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread < 0.005, "camera moved {spread}");
    }

    #[test]
    fn hold_when_lost_then_drift_to_center() {
        let mut t: Vec<Option<f64>> = vec![Some(0.8); 30];
        t.extend(vec![None; 150]);
        let p = SmoothParams::default();
        let x = smooth(&t, 30.0, &p, &[]);
        assert!((x[29] - 0.8).abs() < 0.01);
        // within the timeout: holds
        assert!((x[29 + 40] - 0.8).abs() < 0.01, "{}", x[69]);
        // after the timeout: drifting toward center, monotonically
        assert!(x[179] < 0.75, "{}", x[179]);
        assert!(x[60..].windows(2).all(|w| w[1] <= w[0] + 1e-9));
    }

    #[test]
    fn scene_cut_resets() {
        let mut t: Vec<Option<f64>> = vec![Some(0.2); 30];
        t.extend(vec![Some(0.8); 30]);
        let x = smooth(&t, 30.0, &SmoothParams::default(), &[30]);
        assert!((x[30] - 0.8).abs() < 1e-6, "snap at cut: {}", x[30]);
        let y = smooth(&t, 30.0, &SmoothParams::default(), &[]);
        assert!(y[31] < 0.5, "without a cut the camera pans: {}", y[31]);
    }

    #[test]
    fn max_speed_respected() {
        let mut t: Vec<Option<f64>> = vec![Some(0.1); 10];
        t.extend(vec![Some(0.9); 60]);
        let p = SmoothParams { max_speed: 0.5, ..Default::default() };
        let x = smooth(&t, 30.0, &p, &[]);
        for w in x.windows(2) {
            assert!((w[1] - w[0]).abs() * 30.0 <= 0.5 + 1e-9);
        }
    }

    #[test]
    fn tracks_and_selection() {
        let dets: Vec<FrameDet> = (0..20)
            .map(|i| {
                let mut faces = vec![face(0.3 + 0.002 * i as f64)];
                if i % 2 == 0 {
                    faces.push(face(0.75));
                }
                FrameDet { t: i as f64 / 6.0, faces }
            })
            .collect();
        let tr = build_tracks(&dets, 0.1, 0.6);
        assert_eq!(tr.len(), 2);
        assert!((select(&tr, Subject::MostPersistent).unwrap().mean(|f| f.cx()) - 0.32).abs() < 0.03);
        assert!(select(&tr, Subject::Right).unwrap().mean(|f| f.cx()) > 0.7);
        assert!(select(&tr, Subject::Index(1)).unwrap().mean(|f| f.cx()) > 0.7);
    }
}
