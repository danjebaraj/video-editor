//! Energy-based voice activity detection (fallback when no transcript is available).

/// Detect speech spans in a mono buffer at `rate` Hz.
/// 20 ms frames; threshold = max(abs_floor, noise_floor + 15 dB); 150 ms hangover; spans
/// shorter than 80 ms dropped.
pub fn detect(samples: &[f32], rate: u32) -> Vec<(f64, f64)> {
    let hop = (rate as usize / 50).max(1);
    let frames: Vec<f64> = samples
        .chunks(hop)
        .map(|c| {
            let e = c.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / c.len().max(1) as f64;
            10.0 * e.max(1e-12).log10()
        })
        .collect();
    if frames.is_empty() {
        return vec![];
    }
    let mut sorted = frames.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor = sorted[sorted.len() / 10];
    // A loud floor means signal is (almost) always present: fall back to an absolute gate.
    let thr = if floor > -45.0 { -45.0 } else { (floor + 15.0).max(-50.0) };
    let fd = hop as f64 / rate as f64;
    let hang = (0.15 / fd).round() as usize;
    let mut spans = vec![];
    let mut start: Option<usize> = None;
    let mut last_on = 0usize;
    for (i, &db) in frames.iter().enumerate() {
        if db > thr {
            if start.is_none() {
                start = Some(i);
            }
            last_on = i;
        } else if let Some(s) = start {
            if i > last_on + hang {
                spans.push((s as f64 * fd, (last_on + 1) as f64 * fd));
                start = None;
            }
        }
    }
    if let Some(s) = start {
        spans.push((s as f64 * fd, (last_on + 1) as f64 * fd));
    }
    spans.retain(|(a, b)| b - a >= 0.08);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_bursts() {
        let rate = 16000;
        let mut s = vec![0.0f32; rate * 4];
        for (a, b) in [(0.5, 1.5), (2.5, 3.0)] {
            let (lo, hi) = ((a * rate as f64) as usize, (b * rate as f64) as usize);
            for (i, v) in s.iter_mut().enumerate().take(hi).skip(lo) {
                *v = 0.3 * ((i as f32) * 0.2).sin();
            }
        }
        let v = detect(&s, rate as u32);
        assert_eq!(v.len(), 2, "{v:?}");
        assert!((v[0].0 - 0.5).abs() < 0.03 && (v[0].1 - 1.5).abs() < 0.03);
        assert!((v[1].0 - 2.5).abs() < 0.03 && (v[1].1 - 3.0).abs() < 0.03);
    }
}
