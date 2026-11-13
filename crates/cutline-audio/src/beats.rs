//! Beat tracking for snapping cuts to music.
//!
//! 1. Onset strength: half-wave rectified spectral flux of the log-magnitude spectrum
//!    (1024-point frames, 512 hop at 22.05 kHz ≈ 23 ms), with the local mean removed.
//! 2. Tempo: autocorrelation of the onset envelope over 60–200 BPM, weighted by a log-normal
//!    prior around 120 BPM (as in librosa), refined by parabolic interpolation.
//! 3. Beats: Ellis' dynamic-programming tracker: each frame's score is its onset strength plus
//!    the best predecessor score penalised by how far the gap is from one beat period
//!    (`-tightness · ln(gap/period)²`), traced back from the best-scoring frame near the end.
//! 4. Grid: when the tracked beats are steady (most music made for video is), they are replaced
//!    by an exact grid `phase + k·period` fitted against a fine (≈3 ms) onset envelope, so
//!    beat times do not wobble by the 23 ms analysis hop and do not drift over the track.
//! 5. Bars: assuming 4 beats to the bar, the beat-of-four that is loudest and where the
//!    spectrum changes most (chords and bass notes change on the "one") starts the bars.

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
    /// Times of the first beat of each bar (a subset of `times`), assuming 4 beats to the bar.
    #[serde(default)]
    pub downbeats: Vec<f64>,
    /// How clearly the bar starts stand out, 0–1. Below `BAR_CONFIDENT` the bars are a guess.
    #[serde(default)]
    pub bar_confidence: f64,
    /// The tempo is constant and `times` is an exact grid.
    #[serde(default)]
    pub steady: bool,
}

/// `bar_confidence` from which downbeats are trusted (for snapping preference).
pub const BAR_CONFIDENT: f64 = 0.25;
/// Beats per bar assumed for downbeats.
pub const BEATS_PER_BAR: usize = 4;

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

/// Fine onset envelope: half-wave rectified rise of log energy, 64-sample hop (≈2.9 ms),
/// lightly smoothed. Returns (envelope, seconds per hop).
fn fine_onsets(samples: &[f32]) -> (Vec<f64>, f64) {
    const H: usize = 64;
    const W: usize = 256;
    if samples.len() < W + H {
        return (vec![], H as f64 / RATE as f64);
    }
    let n = (samples.len() - W) / H;
    let e: Vec<f64> = (0..n)
        .map(|i| (1e-9 + samples[i * H..i * H + W].iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>()).ln())
        .collect();
    // rise over ~6 ms, placed at the middle of the later window
    let mut on = vec![0.0; n];
    for i in 2..n {
        on[i] = (e[i] - e[i - 2]).max(0.0);
    }
    let sm: Vec<f64> = (0..n)
        .map(|i| 0.25 * on[i.saturating_sub(1)] + 0.5 * on[i] + 0.25 * on[(i + 1).min(n - 1)])
        .collect();
    (sm, H as f64 / RATE as f64)
}

/// Exact beat grid (phase, period) for steadily tracked beats, or `None` when the tempo moves.
fn fit_grid(samples: &[f32], raw: &[f64]) -> Option<(f64, f64)> {
    if raw.len() < 8 {
        return None;
    }
    let mut gaps: Vec<f64> = raw.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = gaps[gaps.len() / 2];
    let near = gaps.iter().filter(|g| (*g / med - 1.0).abs() < 0.08).count();
    if (near as f64) < 0.85 * gaps.len() as f64 {
        return None;
    }
    // least squares t = a + p·k over the tracked beats
    let n = raw.len() as f64;
    let (sk, st) = ((n - 1.0) * n / 2.0, raw.iter().sum::<f64>());
    let skk: f64 = (0..raw.len()).map(|k| (k * k) as f64).sum();
    let skt: f64 = raw.iter().enumerate().map(|(k, t)| k as f64 * t).sum();
    let p0 = (n * skt - sk * st) / (n * skk - sk * sk);
    let a0 = (st - p0 * sk) / n;
    if (p0 / med - 1.0).abs() > 0.03 {
        return None;
    }
    let (on, hop) = fine_onsets(samples);
    if on.is_empty() {
        return Some((a0, p0));
    }
    // window centre of envelope index i: the energy window is 256 samples, the rise spans two hops
    let at = |t: f64| -> f64 {
        let x = (t - 128.0 / RATE as f64) / hop;
        if x < 0.0 || x + 1.0 >= on.len() as f64 {
            return 0.0;
        }
        let (i, fr) = (x.floor() as usize, x - x.floor());
        on[i] * (1.0 - fr) + on[i + 1] * fr
    };
    let dur = samples.len() as f64 / RATE as f64;
    let score = |a: f64, p: f64| -> f64 {
        let mut k = ((0.0 - a) / p).ceil();
        let mut s = 0.0;
        loop {
            let t = a + k * p;
            if t >= dur {
                break;
            }
            s += at(t);
            k += 1.0;
        }
        s
    };
    // coarse-to-fine search around the fit: period ±0.6 %, phase ±45 ms (about the track middle,
    // so a period change does not swing the phase)
    let mid = (dur / 2.0 / p0).round();
    let (mut bp, mut bc) = (p0, a0 + mid * p0);
    let (mut dp, mut dc) = (p0 * 0.006, 0.045);
    for _ in 0..4 {
        let mut best = (f64::MIN, bp, bc);
        for i in -12..=12 {
            let p = bp + dp * i as f64 / 12.0;
            for j in -15..=15 {
                let c = bc + dc * j as f64 / 15.0;
                let v = score(c - mid * p, p);
                if v > best.0 {
                    best = (v, p, c);
                }
            }
        }
        bp = best.1;
        bc = best.2;
        dp /= 6.0;
        dc /= 6.0;
    }
    Some((bc - mid * bp, bp))
}

/// Index (0..BEATS_PER_BAR) of the grid beats that start bars, with a 0–1 confidence.
fn bar_phase(samples: &[f32], times: &[f64]) -> (usize, f64) {
    const N: usize = 2048;
    if times.len() < BEATS_PER_BAR * 4 {
        return (0, 0.0);
    }
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
    let window: Vec<f32> =
        (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos()).collect();
    // log spectrum 40 Hz – 2 kHz of the ~93 ms after each beat, and its loudness
    let (lo, hi) = (40 * N / RATE as usize, 2000 * N / RATE as usize);
    let mut spec: Vec<Option<Vec<f32>>> = vec![];
    let mut loud: Vec<f64> = vec![];
    let mut buf = vec![Complex::new(0.0f32, 0.0); N];
    for t in times {
        let i0 = (t * RATE as f64) as usize;
        if i0 + N > samples.len() {
            spec.push(None);
            loud.push(f64::NAN);
            continue;
        }
        let seg = &samples[i0..i0 + N];
        for (b, (x, w)) in buf.iter_mut().zip(seg.iter().zip(&window)) {
            *b = Complex::new(x * w, 0.0);
        }
        fft.process(&mut buf);
        let m: Vec<f32> = buf[lo..hi].iter().map(|c| (1.0 + 50.0 * c.norm()).ln()).collect();
        loud.push((1e-9 + seg.iter().map(|x| (*x as f64).powi(2)).sum::<f64>()).ln());
        spec.push(Some(m));
    }
    let mut change = vec![f64::NAN; times.len()];
    for k in 1..times.len() {
        if let (Some(a), Some(b)) = (&spec[k - 1], &spec[k]) {
            change[k] = a.iter().zip(b).map(|(x, y)| (x - y).abs() as f64).sum();
        }
    }
    let z = |v: &[f64]| -> Vec<f64> {
        let ok: Vec<f64> = v.iter().cloned().filter(|x| x.is_finite()).collect();
        if ok.len() < 2 {
            return vec![0.0; v.len()];
        }
        let m = ok.iter().sum::<f64>() / ok.len() as f64;
        let sd = (ok.iter().map(|x| (x - m).powi(2)).sum::<f64>() / ok.len() as f64).sqrt().max(1e-9);
        v.iter().map(|x| if x.is_finite() { (x - m) / sd } else { 0.0 }).collect()
    };
    let (zl, zc) = (z(&loud), z(&change));
    // Vote bar by bar: within each group of four beats, the loudest one and the one with the
    // biggest change each get a vote. Real bar starts win far more than their quarter share.
    let mut votes = [0.0f64; BEATS_PER_BAR];
    let groups = times.len() / BEATS_PER_BAR;
    for g in 0..groups {
        for feat in [&zl, &zc] {
            let k0 = g * BEATS_PER_BAR;
            let j = (0..BEATS_PER_BAR).max_by(|a, b| feat[k0 + a].partial_cmp(&feat[k0 + b]).unwrap()).unwrap();
            votes[j] += 1.0;
        }
    }
    let total = (2 * groups).max(1) as f64;
    let best = (0..BEATS_PER_BAR).max_by(|a, b| votes[*a].partial_cmp(&votes[*b]).unwrap()).unwrap_or(0);
    let second = (0..BEATS_PER_BAR).filter(|j| *j != best).map(|j| votes[j]).fold(0.0, f64::max);
    // share of votes ahead of the runner-up, scaled so a 35-point lead is full confidence
    (best, (((votes[best] - second) / total) / 0.35).clamp(0.0, 1.0))
}

/// Tempo, beats and bars of mono `samples` at `RATE`. `None` for audio without a steady pulse.
pub fn detect(samples: &[f32]) -> Option<Beats> {
    let env = onset_envelope(samples);
    let mut bpm = tempo(&env)?;
    let mut times = track(&env, bpm);
    if times.len() < 4 {
        return None;
    }
    let mut steady = false;
    if let Some((a, p)) = fit_grid(samples, &times) {
        let dur = samples.len() as f64 / RATE as f64;
        // a beat a few ms before the first sample is the beat at 0, not a missing one
        let mut k = ((-0.02 - a) / p).ceil();
        let mut grid = vec![];
        while a + k * p < dur - 0.05 {
            grid.push((a + k * p).max(0.0));
            k += 1.0;
        }
        if grid.len() >= 4 {
            times = grid;
            bpm = 60.0 / p;
            steady = true;
        }
    }
    let (phase, bar_confidence) = bar_phase(samples, &times);
    let downbeats = times.iter().skip(phase).step_by(BEATS_PER_BAR).cloned().collect();
    Some(Beats { bpm, times, downbeats, bar_confidence, steady })
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
    fn steady_music_gets_an_exact_grid() {
        for (bpm, offset) in [(110.0, 0.31), (128.0, 0.07)] {
            let (s, truth) = groove(bpm, offset, 30.0);
            let b = detect(&s).expect("beats");
            assert!(b.steady);
            assert!((b.bpm - bpm).abs() < 0.05, "tempo {} vs {bpm}", b.bpm);
            for t in &b.times {
                let d = truth.iter().map(|x| (x - t).abs()).fold(f64::MAX, f64::min);
                if *t > 0.5 && *t < 29.0 {
                    assert!(d < 0.006, "beat at {t:.4} is {:.1} ms off ({bpm} BPM)", d * 1000.0);
                }
            }
        }
    }

    #[test]
    fn accented_beat_starts_the_bar() {
        // every fourth beat (starting with the third) is louder and brings a new bass note
        let bpm = 100.0;
        let period = 60.0 / bpm;
        let (mut s, truth) = groove(bpm, 0.2, 30.0);
        let notes = [55.0f32, 65.4, 73.4, 49.0];
        for (k, t) in truth.iter().enumerate() {
            if k % 4 != 2 {
                continue;
            }
            let hz = notes[(k / 4) % notes.len()];
            let i0 = (t * RATE as f64) as usize;
            for i in 0..(4.0 * period * RATE as f64) as usize {
                if let Some(v) = s.get_mut(i0 + i) {
                    let tt = i as f32 / RATE as f32;
                    *v += 0.35 * (2.0 * std::f32::consts::PI * hz * tt).sin() * (-tt * 1.2).exp();
                }
            }
        }
        let b = detect(&s).expect("beats");
        assert!(b.bar_confidence >= BAR_CONFIDENT, "confidence {}", b.bar_confidence);
        for d in b.downbeats.iter().filter(|d| **d > 1.0 && **d < 28.0) {
            let k = ((d - 0.2) / period).round() as i64;
            assert_eq!(k.rem_euclid(4), 2, "downbeat at {d:.3} is beat {k}");
        }
        // identical beats: no bar is claimed
        let (plain, _) = groove(bpm, 0.2, 30.0);
        assert!(detect(&plain).unwrap().bar_confidence < BAR_CONFIDENT);
    }

    #[test]
    fn silence_has_no_beats() {
        assert_eq!(detect(&vec![0.0f32; RATE as usize * 10]), None);
    }
}
