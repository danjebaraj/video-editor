//! Beat tracking for snapping cuts to music.
//!
//! 1. Onset strength: half-wave rectified spectral flux of the log-magnitude spectrum
//!    (1024-point frames, 512 hop at 22.05 kHz ≈ 23 ms), with the local mean removed.
//! 2. Tempo: autocorrelation of the onset envelope over 60–200 BPM, weighted by a log-normal
//!    prior around 120 BPM (as in librosa), refined by parabolic interpolation.
//! 3. Beats: Ellis' dynamic-programming tracker: each frame's score is its onset strength plus
//!    the best predecessor score penalised by how far the gap is from one beat period
//!    (`-tightness · ln(gap/period)²`), traced back from the best-scoring frame near the end.

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use serde::{Deserialize, Serialize};

/// Sample rate the tracker expects.
pub const RATE: u32 = 22_050;
const FRAME: usize = 1024;
const HOP: usize = 512;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Beats {
    pub bpm: f64,
    /// Beat times in seconds from the start of the audio.
    pub times: Vec<f64>,
}

fn hop_secs() -> f64 {
    HOP as f64 / RATE as f64
}

/// Onset strength per hop (spectral flux), mean-removed and clipped at 0.
pub fn onset_envelope(samples: &[f32]) -> Vec<f64> {
    if samples.len() < FRAME {
        return vec![];
    }
    let fft = FftPlanner::<f32>::new().plan_fft_forward(FRAME);
    let window: Vec<f32> =
        (0..FRAME).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FRAME as f32).cos()).collect();
    let n = (samples.len() - FRAME) / HOP + 1;
    let mut prev: Vec<f32> = vec![0.0; FRAME / 2];
    let mut env = Vec::with_capacity(n);
    let mut buf = vec![Complex::new(0.0f32, 0.0); FRAME];
    for f in 0..n {
        let s = &samples[f * HOP..f * HOP + FRAME];
        for (b, (x, w)) in buf.iter_mut().zip(s.iter().zip(&window)) {
            *b = Complex::new(x * w, 0.0);
        }
        fft.process(&mut buf);
        let mut flux = 0.0f64;
        for (k, c) in buf[..FRAME / 2].iter().enumerate() {
            let m = (1.0 + 100.0 * c.norm()).ln();
            if f > 0 {
                flux += (m - prev[k]).max(0.0) as f64;
            }
            prev[k] = m;
        }
        env.push(flux);
    }
    // remove the slowly varying level (≈ 0.5 s mean) so loud sections do not dominate
    let w = (0.5 / hop_secs()) as usize;
    let mut out = vec![0.0; env.len()];
    let mut acc = 0.0;
    for i in 0..env.len() {
        acc += env[i];
        if i > w {
            acc -= env[i - w - 1];
        }
        let mean = acc / (i.min(w) + 1) as f64;
        out[i] = (env[i] - mean).max(0.0);
    }
    let peak = out.iter().cloned().fold(0.0, f64::max);
    if peak > 0.0 {
        out.iter_mut().for_each(|v| *v /= peak);
    }
    out
}

/// Tempo (BPM) from the onset envelope.
pub fn tempo(env: &[f64]) -> Option<f64> {
    let fps = 1.0 / hop_secs();
    let (lo, hi) = ((fps * 60.0 / 200.0).floor() as usize, (fps * 60.0 / 60.0).ceil() as usize);
    if env.len() < hi * 2 {
        return None;
    }
    // smooth first: a beat period that is not a whole number of frames otherwise splits its
    // autocorrelation peak between two lags, and twice the period wins (octave error)
    let mut sm = env.to_vec();
    for _ in 0..2 {
        sm = (0..sm.len())
            .map(|i| {
                let a = sm[i.saturating_sub(1)];
                let c = sm[(i + 1).min(sm.len() - 1)];
                0.25 * a + 0.5 * sm[i] + 0.25 * c
            })
            .collect();
    }
    let ac =
        |lag: usize| -> f64 { sm.iter().zip(&sm[lag..]).map(|(a, b)| a * b).sum::<f64>() / (sm.len() - lag) as f64 };
    let prior = |lag: f64| {
        let bpm = 60.0 * fps / lag;
        (-0.5 * ((bpm / 120.0).log2() / 1.0).powi(2)).exp()
    };
    let scores: Vec<(usize, f64)> = (lo..=hi).map(|l| (l, ac(l) * prior(l as f64))).collect();
    let (best, _) = scores.iter().cloned().fold((0, f64::MIN), |a, b| if b.1 > a.1 { b } else { a });
    if best == 0 {
        return None;
    }
    // parabolic refinement around the best lag
    let y = |l: usize| scores.iter().find(|s| s.0 == l).map(|s| s.1);
    let lag = match (y(best - 1), y(best), y(best + 1)) {
        (Some(a), Some(b), Some(c)) if (a - 2.0 * b + c).abs() > 1e-12 => {
            best as f64 + 0.5 * (a - c) / (a - 2.0 * b + c)
        }
        _ => best as f64,
    };
    Some(60.0 * fps / lag)
}

/// Beat times (seconds) given the onset envelope and tempo.
pub fn track(env: &[f64], bpm: f64) -> Vec<f64> {
    if env.is_empty() || bpm <= 0.0 {
        return vec![];
    }
    let period = 60.0 / bpm / hop_secs(); // frames per beat
    let tightness = 100.0;
    let n = env.len();
    let mut score = env.to_vec();
    let mut back: Vec<Option<usize>> = vec![None; n];
    let (lo, hi) = ((period / 2.0).round() as usize, (period * 2.0).round() as usize);
    for t in 0..n {
        let mut best: Option<(f64, usize)> = None;
        for gap in lo.max(1)..=hi {
            if gap > t {
                break;
            }
            let p = t - gap;
            let pen = -tightness * (gap as f64 / period).ln().powi(2);
            let v = score[p] + pen;
            if best.map_or(true, |(b, _)| v > b) {
                best = Some((v, p));
            }
        }
        if let Some((v, p)) = best {
            if v > 0.0 {
                score[t] = env[t] + v;
                back[t] = Some(p);
            }
        }
    }
    // the last beat: the best score within the final beat period
    let tail = (n as f64 - period).max(0.0) as usize;
    let mut t = (tail..n).max_by(|&a, &b| score[a].partial_cmp(&score[b]).unwrap()).unwrap_or(n - 1);
    let mut beats = vec![t];
    while let Some(p) = back[t] {
        beats.push(p);
        t = p;
    }
    beats.reverse();
    beats.into_iter().map(|f| f as f64 * hop_secs() + FRAME as f64 / 2.0 / RATE as f64).collect()
}

/// Tempo and beats of mono `samples` at `RATE`. `None` for audio without a steady pulse.
pub fn detect(samples: &[f32]) -> Option<Beats> {
    let env = onset_envelope(samples);
    let bpm = tempo(&env)?;
    let times = track(&env, bpm);
    (times.len() >= 4).then_some(Beats { bpm, times })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kick on every beat, quieter hats on the off-beats, noise underneath.
    fn groove(bpm: f64, offset: f64, secs: f64) -> (Vec<f32>, Vec<f64>) {
        let n = (secs * RATE as f64) as usize;
        let mut s = vec![0.0f32; n];
        let mut seed = 12345u32;
        for v in s.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.02;
        }
        let period = 60.0 / bpm;
        let mut truth = vec![];
        let mut t = offset;
        while t < secs - 0.3 {
            truth.push(t);
            // kick: 60 Hz body + broadband click; hat: a quieter, short noise tick
            for (at, amp, hz, click) in [(t, 0.9f32, 60.0f32, 0.6f32), (t + period / 2.0, 0.0, 0.0, 0.15)] {
                let i0 = (at * RATE as f64) as usize;
                for k in 0..(0.12 * RATE as f64) as usize {
                    if let Some(v) = s.get_mut(i0 + k) {
                        let tt = k as f32 / RATE as f32;
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                        *v += amp * (2.0 * std::f32::consts::PI * hz * tt).sin() * (-tt * 30.0).exp()
                            + click * noise * (-tt * 400.0).exp();
                    }
                }
            }
            t += period;
        }
        (s, truth)
    }

    fn check(bpm: f64, offset: f64) {
        let (s, truth) = groove(bpm, offset, 20.0);
        let b = detect(&s).expect("beats");
        assert!((b.bpm - bpm).abs() / bpm < 0.02, "tempo {} vs {bpm}", b.bpm);
        // every detected beat (away from the edges) lies within 35 ms of a true beat
        let inner: Vec<f64> = b.times.iter().cloned().filter(|t| *t > 1.0 && *t < 19.0).collect();
        assert!(inner.len() as f64 >= 0.9 * 18.0 * bpm / 60.0, "found {} beats", inner.len());
        for t in inner {
            let d = truth.iter().map(|x| (x - t).abs()).fold(f64::MAX, f64::min);
            assert!(d < 0.035, "beat at {t:.3} is {d:.3}s from the grid ({bpm} BPM)");
        }
    }

    #[test]
    fn finds_tempo_and_beats_with_offbeats_and_noise() {
        check(120.0, 0.25);
        check(95.0, 0.6);
        check(140.0, 0.1);
    }

    #[test]
    fn silence_has_no_beats() {
        assert_eq!(detect(&vec![0.0f32; RATE as usize * 10]), None);
    }
}
