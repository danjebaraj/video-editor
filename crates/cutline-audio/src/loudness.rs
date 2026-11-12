//! Two-pass `loudnorm` helpers.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Measured {
    pub input_i: f64,
    pub input_tp: f64,
    pub input_lra: f64,
    pub input_thresh: f64,
    pub target_offset: f64,
}

fn num(v: &serde_json::Value) -> Option<f64> {
    v.as_str().and_then(|s| s.trim().parse().ok()).or_else(|| v.as_f64())
}

/// Parse the JSON block `loudnorm=print_format=json` prints at the end of stderr.
pub fn parse_loudnorm_json(stderr: &str) -> Option<Measured> {
    let start = stderr.rfind('{')?;
    let end = stderr[start..].find('}')? + start + 1;
    let j: serde_json::Value = serde_json::from_str(&stderr[start..end]).ok()?;
    let i = num(&j["input_i"])?;
    if !i.is_finite() {
        return None;
    }
    Some(Measured {
        input_i: i,
        input_tp: num(&j["input_tp"]).unwrap_or(-99.0),
        input_lra: num(&j["input_lra"]).unwrap_or(0.0),
        input_thresh: num(&j["input_thresh"]).unwrap_or(-70.0),
        target_offset: num(&j["target_offset"]).unwrap_or(0.0),
    })
}

/// Pass 1 filter.
pub fn pass1(i: f64, tp: f64, lra: f64) -> String {
    format!("loudnorm=I={i}:TP={tp}:LRA={lra}:print_format=json")
}

/// Pass 2 filter (linear mode where possible), resampled back to 48 kHz.
pub fn pass2(i: f64, tp: f64, lra: f64, m: &Measured) -> String {
    format!(
        "loudnorm=I={i}:TP={tp}:LRA={lra}:measured_I={:.2}:measured_TP={:.2}:measured_LRA={:.2}:measured_thresh={:.2}:offset={:.2}:linear=true:print_format=json,aresample=48000",
        m.input_i, m.input_tp, m.input_lra, m.input_thresh, m.target_offset
    )
}

/// Pass 2 shown in dry-run output (values unknown until pass 1 runs).
pub fn pass2_template(i: f64, tp: f64, lra: f64) -> String {
    format!("loudnorm=I={i}:TP={tp}:LRA={lra}:measured_I=<I>:measured_TP=<TP>:measured_LRA=<LRA>:measured_thresh=<thresh>:offset=<offset>:linear=true:print_format=json,aresample=48000")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse() {
        let s = r#"[Parsed_loudnorm_0 @ 0x1]
{
	"input_i" : "-27.61",
	"input_tp" : "-4.47",
	"input_lra" : "18.06",
	"input_thresh" : "-39.20",
	"output_i" : "-16.58",
	"output_tp" : "-1.50",
	"output_lra" : "14.78",
	"output_thresh" : "-27.71",
	"normalization_type" : "dynamic",
	"target_offset" : "0.58"
}"#;
        let m = parse_loudnorm_json(s).unwrap();
        assert_eq!(m.input_i, -27.61);
        assert_eq!(m.target_offset, 0.58);
        assert!(pass2(-14.0, -1.0, 11.0, &m).contains("measured_I=-27.61"));
        assert!(parse_loudnorm_json(r#"{"input_i" : "-inf"}"#).is_none());
    }
}
