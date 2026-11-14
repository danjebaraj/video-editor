//! Timestamps: RFC 3339 UTC in files, local time for run folder names.

use chrono::{DateTime, Local, SecondsFormat, Utc};

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn system_time_rfc3339(t: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(t).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// `yyyy-mm-dd-hhmm` in local time (spec §9.2 run folder names).
pub fn run_stamp() -> String {
    Local::now().format("%Y-%m-%d-%H%M").to_string()
}

pub fn parse(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

/// Seconds between two RFC 3339 stamps, if both parse.
pub fn seconds_between(a: &str, b: &str) -> Option<f64> {
    Some((parse(b)? - parse(a)?).num_milliseconds() as f64 / 1000.0)
}
