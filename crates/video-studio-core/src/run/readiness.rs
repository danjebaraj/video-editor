//! The dry run's readiness report (spec §11.2): what Claude found unclear, what it assumed, what
//! is missing and what is risky, plus an estimated duration per output. Claude writes it as
//! `readiness.json` in the run folder.

use std::collections::BTreeMap;

use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub text: String,
    /// 1-based line of `spec.md` the finding is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ReadinessReport {
    /// Claude's own verdict: could a production run proceed as the spec stands?
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub ambiguities: Vec<Finding>,
    #[serde(default)]
    pub assumptions: Vec<Finding>,
    #[serde(default)]
    pub missing_media: Vec<Finding>,
    #[serde(default)]
    pub risks: Vec<Finding>,
    /// What the production run will record or generate itself. Not a problem, so it never
    /// counts against the report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub planned: Vec<Finding>,
    /// Output slug → estimated duration in seconds.
    #[serde(default)]
    pub estimated_durations: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

fn findings_from(v: &Value) -> Vec<Finding> {
    let one = |x: &Value| -> Option<Finding> {
        match x {
            Value::String(s) if !s.trim().is_empty() => {
                Some(Finding { text: s.trim().to_string(), spec_line: None, detail: None })
            }
            Value::Object(o) => {
                let text = ["text", "message", "title", "summary", "question", "description"]
                    .iter()
                    .find_map(|k| o.get(*k).and_then(Value::as_str))?
                    .trim()
                    .to_string();
                let line = ["spec_line", "line"]
                    .iter()
                    .find_map(|k| o.get(*k).and_then(Value::as_u64))
                    .map(|n| n as usize)
                    .filter(|n| *n > 0);
                let detail = ["detail", "details", "suggestion", "assumption"]
                    .iter()
                    .find_map(|k| o.get(*k).and_then(Value::as_str))
                    .map(str::to_string);
                (!text.is_empty()).then_some(Finding { text, spec_line: line, detail })
            }
            _ => None,
        }
    };
    match v {
        Value::Array(a) => a.iter().filter_map(one).collect(),
        other => one(other).into_iter().collect(),
    }
}

impl ReadinessReport {
    /// Parses what Claude wrote. Lists may hold plain strings or objects, and unknown keys are
    /// ignored, so a slightly off-schema report still comes through.
    pub fn parse(text: &str) -> Option<ReadinessReport> {
        let v: Value = serde_json::from_str(text.trim()).ok()?;
        let o = v.as_object()?;
        let durations = o
            .get("estimated_durations")
            .or_else(|| o.get("estimated_duration_s"))
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect())
            .unwrap_or_default();
        Some(ReadinessReport {
            ready: o.get("ready").and_then(Value::as_bool).unwrap_or(false),
            ambiguities: o.get("ambiguities").map(findings_from).unwrap_or_default(),
            assumptions: o.get("assumptions").map(findings_from).unwrap_or_default(),
            missing_media: o.get("missing_media").map(findings_from).unwrap_or_default(),
            risks: o.get("risks").or_else(|| o.get("risky_choices")).map(findings_from).unwrap_or_default(),
            planned: o.get("planned").or_else(|| o.get("made_by_the_run")).map(findings_from).unwrap_or_default(),
            estimated_durations: durations,
            notes: o.get("notes").and_then(Value::as_str).map(str::to_string),
        })
    }

    pub fn load(run_folder: &Utf8Path) -> Option<ReadinessReport> {
        Self::parse(&std::fs::read_to_string(run_folder.join("readiness.json")).ok()?)
    }

    /// Every finding as `(kind, title, detail, spec line)` for the validation report.
    pub fn findings(&self) -> Vec<(&'static str, String, Option<String>, Option<usize>)> {
        let mut out = Vec::new();
        for (kind, label, list) in [
            ("ambiguity", "Unclear in the spec", &self.ambiguities),
            ("missing_media", "Missing media", &self.missing_media),
            ("risk", "Risk", &self.risks),
            ("assumption", "Assumed", &self.assumptions),
        ] {
            for f in list {
                out.push((kind, format!("{label}: {}", f.text), f.detail.clone(), f.spec_line));
            }
        }
        out
    }

    pub fn is_clean(&self) -> bool {
        self.ambiguities.is_empty() && self.missing_media.is_empty() && self.risks.is_empty()
    }

    /// The findings as a prompt for "Ask Claude to fix the spec" (spec §11.2).
    pub fn fix_prompt(&self) -> String {
        let mut s = String::from("A dry run of this spec found the points below. Update spec.md so a production run has what it needs. Ask me about anything only I can decide.\n\nThe spec describes the video, not this computer or the project's settings. Some points can't be fixed in spec.md: a missing permission, a voice that isn't chosen, an output that isn't added to the run, an API key. Don't write those into the spec as notes or status; tell me in your reply what I need to change and where. If the spec already carries such notes from an earlier dry run, remove them.\n");
        for (title, list) in [
            ("Unclear", &self.ambiguities),
            ("Missing media", &self.missing_media),
            ("Risks", &self.risks),
            ("Assumptions the dry run made (confirm or correct them in the spec)", &self.assumptions),
        ] {
            if list.is_empty() {
                continue;
            }
            s.push_str(&format!("\n{title}:\n"));
            for f in list {
                let line = f.spec_line.map(|l| format!(" (spec.md line {l})")).unwrap_or_default();
                let detail = f.detail.as_ref().map(|d| format!(" — {d}")).unwrap_or_default();
                s.push_str(&format!("- {}{line}{detail}\n", f.text));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_and_loose_reports() {
        let r = ReadinessReport::parse(r#"{"ready": false,
            "ambiguities": [{"text": "Which logo opens the video?", "spec_line": 12, "detail": "Two logos are attached"}],
            "assumptions": ["Used the horizontal logo"],
            "missing_media": [{"message": "No footage for scene 3", "line": 30}],
            "risky_choices": [{"title": "Voice-over runs 4 s past the last scene"}, "", 7],
            "estimated_durations": {"youtube-1080p": 46.5, "bad": "x"}, "extra": true}"#)
        .unwrap();
        assert!(!r.ready && !r.is_clean());
        assert_eq!(
            r.ambiguities[0],
            Finding {
                text: "Which logo opens the video?".into(),
                spec_line: Some(12),
                detail: Some("Two logos are attached".into())
            }
        );
        assert_eq!(r.assumptions[0].text, "Used the horizontal logo");
        assert_eq!(r.missing_media[0].spec_line, Some(30));
        assert_eq!(r.risks.len(), 1);
        assert_eq!(r.estimated_durations.get("youtube-1080p"), Some(&46.5));
        assert_eq!(r.estimated_durations.len(), 1);
        let f = r.findings();
        assert_eq!(f.len(), 4);
        assert_eq!((f[0].0, f[0].3), ("ambiguity", Some(12)));
        let prompt = r.fix_prompt();
        assert!(prompt.contains("- Which logo opens the video? (spec.md line 12) — Two logos are attached"));
        assert!(prompt.contains("Missing media:") && prompt.contains("Assumptions"));
        assert!(prompt.contains("not this computer or the project's settings"));
        assert!(ReadinessReport::parse("not json").is_none());
        assert!(ReadinessReport::parse("[]").is_none());
        assert!(ReadinessReport::parse(r#"{"ready": true}"#).unwrap().is_clean());
        // What the run makes itself is listed but is not a finding.
        let p =
            ReadinessReport::parse(r#"{"ready": true, "planned": ["Recording captures/scene-2.mov, 22 s"]}"#).unwrap();
        assert!(p.is_clean() && p.findings().is_empty() && p.planned.len() == 1);
        assert!(!p.fix_prompt().contains("scene-2"));
    }
}
