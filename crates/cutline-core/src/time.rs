//! Time model: seconds (`12.4`), timecodes (`"01:02.500"`, `"00:01:02.500"`), frames
//! (`"f:372"`) and the `"end"` keyword. Timeline positions are integer frames at the timeline fps.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// A user-supplied time value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeValue {
    Seconds(f64),
    Frames(i64),
    End,
}

impl TimeValue {
    /// Seconds at the given fps; `End` resolves to `end`.
    pub fn seconds(&self, fps: Fps, end: f64) -> f64 {
        match *self {
            TimeValue::Seconds(s) => s,
            TimeValue::Frames(f) => f as f64 / fps.as_f64(),
            TimeValue::End => end,
        }
    }
    /// Frames at `fps`, rounded to nearest (cuts snap to output frames).
    pub fn frames(&self, fps: Fps, end_frames: i64) -> i64 {
        match *self {
            TimeValue::Seconds(s) => fps.secs_to_frames(s),
            TimeValue::Frames(f) => f,
            TimeValue::End => end_frames,
        }
    }
    pub fn is_end(&self) -> bool {
        matches!(self, TimeValue::End)
    }
}

pub fn parse_time(s: &str) -> Result<TimeValue, String> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("end") {
        return Ok(TimeValue::End);
    }
    if let Some(f) = t.strip_prefix("f:") {
        return f.trim().parse::<i64>().map(TimeValue::Frames).map_err(|_| format!("invalid frame count `{s}`"));
    }
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() > 3 {
        return Err(format!("invalid timecode `{s}`"));
    }
    let mut secs = 0.0;
    for (i, p) in parts.iter().enumerate() {
        let v: f64 = p
            .parse()
            .map_err(|_| format!("invalid time `{s}` (use seconds, \"mm:ss.mmm\", \"hh:mm:ss.mmm\" or \"f:N\")"))?;
        if v < 0.0 || (i > 0 && v >= 60.0) {
            return Err(format!("invalid timecode component in `{s}`"));
        }
        secs = secs * 60.0 + v;
    }
    if !secs.is_finite() || secs < 0.0 {
        return Err(format!("negative or invalid time `{s}`"));
    }
    Ok(TimeValue::Seconds(secs))
}

impl<'de> Deserialize<'de> for TimeValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        match v {
            serde_json::Value::Number(n) => {
                let f = n.as_f64().ok_or_else(|| serde::de::Error::custom("invalid number"))?;
                if f < 0.0 {
                    return Err(serde::de::Error::custom("time must not be negative"));
                }
                Ok(TimeValue::Seconds(f))
            }
            serde_json::Value::String(s) => parse_time(&s).map_err(serde::de::Error::custom),
            _ => Err(serde::de::Error::custom("time must be a number of seconds or a string")),
        }
    }
}

impl Serialize for TimeValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            TimeValue::Seconds(v) => s.serialize_f64(round6(*v)),
            TimeValue::Frames(f) => s.serialize_str(&format!("f:{f}")),
            TimeValue::End => s.serialize_str("end"),
        }
    }
}

impl JsonSchema for TimeValue {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TimeValue".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Seconds (number), timecode string \"mm:ss.mmm\" / \"hh:mm:ss.mmm\", frames \"f:N\", or \"end\".",
            "oneOf": [
                {"type": "number", "minimum": 0},
                {"type": "string", "pattern": "^(end|f:\\d+|(\\d+:){0,2}\\d+(\\.\\d+)?)$"}
            ]
        })
    }
}

pub fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// Frame rate as a rational.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fps {
    pub num: i64,
    pub den: i64,
}

impl Fps {
    pub const fn new(num: i64, den: i64) -> Self {
        Self { num, den }
    }
    pub fn as_f64(&self) -> f64 {
        self.num as f64 / self.den as f64
    }
    pub fn secs_to_frames(&self, s: f64) -> i64 {
        (s * self.num as f64 / self.den as f64 + 1e-9).round() as i64
    }
    pub fn frames_to_secs(&self, f: i64) -> f64 {
        f as f64 * self.den as f64 / self.num as f64
    }
    /// ffmpeg rational string (`30`, `30000/1001`).
    pub fn ff(&self) -> String {
        if self.den == 1 {
            self.num.to_string()
        } else {
            format!("{}/{}", self.num, self.den)
        }
    }
    /// From a float, recognizing NTSC rates.
    pub fn from_f64(f: f64) -> Option<Self> {
        if !(f.is_finite() && f > 0.0 && f <= 240.0) {
            return None;
        }
        for (n, d) in [(24000, 1001), (30000, 1001), (60000, 1001)] {
            if (f - n as f64 / d as f64).abs() < 0.005 {
                return Some(Fps::new(n, d));
            }
        }
        if (f - f.round()).abs() < 1e-6 {
            return Some(Fps::new(f.round() as i64, 1));
        }
        Some(Fps::new((f * 1000.0).round() as i64, 1000))
    }
    pub fn parse(s: &str) -> Option<Self> {
        if let Some((a, b)) = s.split_once('/') {
            let (n, d): (i64, i64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            if d <= 0 || n <= 0 {
                return None;
            }
            return Fps::from_f64(n as f64 / d as f64).map(|f| if f.den == 1000 { Fps::new(n, d) } else { f });
        }
        Fps::from_f64(s.trim().parse().ok()?)
    }
}

impl fmt::Display for Fps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.ff())
    }
}

impl<'de> Deserialize<'de> for Fps {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        let r = match &v {
            serde_json::Value::Number(n) => n.as_f64().and_then(Fps::from_f64),
            serde_json::Value::String(s) => Fps::parse(s),
            _ => None,
        };
        r.ok_or_else(|| serde::de::Error::custom(format!("invalid fps {v}")))
    }
}

impl Serialize for Fps {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.den == 1 {
            s.serialize_i64(self.num)
        } else {
            s.serialize_str(&self.ff())
        }
    }
}

impl JsonSchema for Fps {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Fps".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Frames per second: number (30, 29.97) or rational string \"30000/1001\".",
            "oneOf": [{"type": "number", "exclusiveMinimum": 0}, {"type": "string"}]
        })
    }
}

/// `HH:MM:SS.mmm` (or `MM:SS.mmm` when under an hour).
pub fn format_tc(secs: f64) -> String {
    let ms = (secs * 1000.0).round() as i64;
    let (h, m, s, r) = (ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000);
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}.{r:03}")
    } else {
        format!("{m:02}:{s:02}.{r:03}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(parse_time("12.4").unwrap(), TimeValue::Seconds(12.4));
        assert_eq!(parse_time("01:02.500").unwrap(), TimeValue::Seconds(62.5));
        assert_eq!(parse_time("00:01:02.500").unwrap(), TimeValue::Seconds(62.5));
        assert_eq!(parse_time("1:00:00").unwrap(), TimeValue::Seconds(3600.0));
        assert_eq!(parse_time("f:372").unwrap(), TimeValue::Frames(372));
        assert_eq!(parse_time("end").unwrap(), TimeValue::End);
        assert!(parse_time("abc").is_err());
        assert!(parse_time("1:2:3:4").is_err());
        assert!(parse_time("-3").is_err());
    }

    #[test]
    fn json_numbers_and_strings() {
        let v: Vec<TimeValue> = serde_json::from_str(r#"[1.5, "00:02.000", "f:10", "end"]"#).unwrap();
        assert_eq!(v, vec![TimeValue::Seconds(1.5), TimeValue::Seconds(2.0), TimeValue::Frames(10), TimeValue::End]);
        assert!(serde_json::from_str::<TimeValue>("-1").is_err());
    }

    #[test]
    fn fps_math() {
        let f = Fps::parse("30000/1001").unwrap();
        assert_eq!(f, Fps::new(30000, 1001));
        assert_eq!(Fps::from_f64(29.97).unwrap(), Fps::new(30000, 1001));
        assert_eq!(Fps::from_f64(30.0).unwrap(), Fps::new(30, 1));
        let f30 = Fps::new(30, 1);
        assert_eq!(f30.secs_to_frames(12.4), 372);
        assert_eq!(TimeValue::Frames(372).seconds(f30, 0.0), 12.4);
        // nearest-frame snapping
        assert_eq!(f30.secs_to_frames(0.016), 0);
        assert_eq!(f30.secs_to_frames(0.017), 1);
    }

    #[test]
    fn tc_format() {
        assert_eq!(format_tc(62.5), "01:02.500");
        assert_eq!(format_tc(3723.25), "01:02:03.250");
    }
}
