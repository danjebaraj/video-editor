//! Ducking envelope from speech activity.
//!
//! Model (deterministic, easy to visualize): the gain ramps linearly in dB from 0 to
//! `amount_db` over `attack` seconds ending at speech onset (pre-roll, so speech is never
//! masked), holds during speech, and ramps back over `release` seconds after speech ends.
//! Gaps shorter than `attack + release`, or than `hold`, stay ducked.

/// Merge spans (sorted, overlapping or closer than `gap` are joined).
pub fn merge(spans: &[(f64, f64)], gap: f64) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = spans.iter().copied().filter(|(a, b)| b > a).collect();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut out: Vec<(f64, f64)> = vec![];
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 + gap => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Gain in dB at time `t`.
pub fn gain_db_at(t: f64, spans: &[(f64, f64)], amount_db: f64, attack: f64, release: f64) -> f64 {
    let mut g = 0.0f64;
    for &(a, b) in spans {
        let v = if t >= a && t <= b {
            amount_db
        } else if t < a && t >= a - attack {
            amount_db * (1.0 - (a - t) / attack.max(1e-6))
        } else if t > b && t <= b + release {
            amount_db * (1.0 - (t - b) / release.max(1e-6))
        } else {
            0.0
        };
        g = g.min(v); // amount_db is negative
    }
    g
}

/// Linear gain envelope sampled at `rate` Hz over `duration` seconds.
pub fn envelope(
    spans: &[(f64, f64)],
    duration: f64,
    rate: u32,
    amount_db: f64,
    attack: f64,
    release: f64,
    hold: f64,
) -> Vec<f32> {
    let merged = merge(spans, (attack + release).max(hold));
    let n = (duration * rate as f64).ceil() as usize + 1;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            10f64.powf(gain_db_at(t, &merged, amount_db, attack, release) / 20.0) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ramps() {
        let s = [(2.0, 3.0)];
        assert_eq!(gain_db_at(0.0, &s, -12.0, 0.1, 0.4), 0.0);
        assert!((gain_db_at(1.95, &s, -12.0, 0.1, 0.4) + 6.0).abs() < 1e-9);
        assert_eq!(gain_db_at(2.5, &s, -12.0, 0.1, 0.4), -12.0);
        assert!((gain_db_at(3.2, &s, -12.0, 0.1, 0.4) + 6.0).abs() < 1e-9);
        assert_eq!(gain_db_at(3.5, &s, -12.0, 0.1, 0.4), 0.0);
    }
    #[test]
    fn merges_short_gaps() {
        assert_eq!(merge(&[(0.0, 1.0), (1.2, 2.0), (5.0, 6.0)], 0.5), vec![(0.0, 2.0), (5.0, 6.0)]);
        let e = envelope(&[(1.0, 2.0)], 3.0, 100, -12.0, 0.1, 0.4, 0.0);
        assert_eq!(e.len(), 301);
        assert!((e[150] - 10f32.powf(-0.6)).abs() < 1e-6);
        assert_eq!(e[0], 1.0);
    }
    #[test]
    fn stays_ducked_through_a_pause_between_sentences() {
        // two sentences 1.2 s apart: the music stays down with a hold of 1.5 s, and comes up
        // in the pause without one
        let speech = [(1.0, 3.0), (4.2, 6.0)];
        let held = envelope(&speech, 7.0, 100, -12.0, 0.25, 0.8, 1.5);
        let free = envelope(&speech, 7.0, 100, -12.0, 0.1, 0.4, 0.0);
        let down = 10f32.powf(-0.6);
        assert!((held[360] - down).abs() < 1e-6, "{}", held[360]);
        assert!(free[360] > 0.99, "{}", free[360]);
        // a long pause still lets it up
        let apart = envelope(&[(1.0, 2.0), (5.0, 6.0)], 7.0, 100, -12.0, 0.25, 0.8, 1.5);
        assert!(apart[350] > 0.99);
    }
}
