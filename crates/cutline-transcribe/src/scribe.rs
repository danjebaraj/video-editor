//! ElevenLabs Scribe (speech-to-text) JSON import. Tolerant of field-name variants and extra
//! fields: words may live at the top level, under `transcripts[]` or `channels[]`; fields may be
//! `text`/`word`, `start`/`start_time`, `end`/`end_time`, `speaker_id`/`speaker`; entries with
//! `type: "spacing"` are skipped; `audio_event` entries are skipped unless asked for.

use crate::model::{Transcript, Word};
use anyhow::{bail, Result};
use serde_json::Value;

fn num(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn collect_words<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    if let Some(ws) = v.get("words").and_then(|w| w.as_array()) {
        out.extend(ws.iter());
        return;
    }
    for key in ["transcripts", "channels", "results", "transcript"] {
        match v.get(key) {
            Some(Value::Array(a)) => {
                for x in a {
                    collect_words(x, out);
                }
            }
            Some(o @ Value::Object(_)) => collect_words(o, out),
            _ => {}
        }
    }
}

pub fn import(text: &str, asset: &str) -> Result<Transcript> {
    let v: Value = serde_json::from_str(text)?;
    let mut raw = vec![];
    collect_words(&v, &mut raw);
    if raw.is_empty() {
        bail!("no `words` array found in Scribe JSON");
    }
    let mut words = vec![];
    for r in raw {
        let ty = r.get("type").and_then(|t| t.as_str()).unwrap_or("word");
        if ty != "word" {
            continue;
        }
        let w = r.get("text").or_else(|| r.get("word")).and_then(|t| t.as_str()).unwrap_or("").trim();
        if w.is_empty() {
            continue;
        }
        let start = r.get("start").or_else(|| r.get("start_time")).and_then(num);
        let end = r.get("end").or_else(|| r.get("end_time")).and_then(num);
        let (Some(start), Some(end)) = (start, end) else { continue };
        let conf = r
            .get("logprob")
            .and_then(num)
            .map(|lp| lp.exp().clamp(0.0, 1.0))
            .or_else(|| r.get("confidence").and_then(num))
            .unwrap_or(1.0);
        let speaker = r
            .get("speaker_id")
            .or_else(|| r.get("speaker"))
            .and_then(|s| s.as_str().map(str::to_string).or_else(|| s.as_i64().map(|i| format!("speaker_{i}"))));
        words.push(Word { w: w.to_string(), start, end: end.max(start), conf, speaker });
    }
    words.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    let lang = v.get("language_code").or_else(|| v.get("language")).and_then(|l| l.as_str()).unwrap_or("").to_string();
    let mut t = Transcript { asset: asset.into(), lang, model: "elevenlabs-scribe".into(), words, segments: vec![] };
    t.rebuild_segments();
    Ok(t)
}

/// Load a transcript file: Scribe JSON or cutline's own format.
pub fn load_any(text: &str, asset: &str) -> Result<Transcript> {
    if let Ok(mut t) = serde_json::from_str::<Transcript>(text) {
        if !t.words.is_empty() && text.contains("\"w\"") {
            if t.segments.is_empty() {
                t.rebuild_segments();
            }
            if t.asset.is_empty() {
                t.asset = asset.into();
            }
            return Ok(t);
        }
    }
    import(text, asset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_shape() {
        let s = include_str!("../tests/fixtures/scribe_sample.json");
        let t = import(s, "vo").unwrap();
        assert_eq!(t.lang, "en");
        assert_eq!(t.words[0].w, "Your");
        assert_eq!(t.words.len(), 9, "{:?}", t.words);
        assert!(t.words.iter().all(|w| w.w != " "));
        assert_eq!(t.words[0].speaker.as_deref(), Some("speaker_0"));
        assert!(t.words.windows(2).all(|p| p[0].start <= p[1].start));
        assert_eq!(t.segments.len(), 2);
    }

    #[test]
    fn tolerant_variants() {
        let s = r#"{"transcripts": [{"words": [
            {"word": "Hi", "start_time": "0.1", "end_time": 0.3, "speaker": 1, "extra": {"x": 1}},
            {"text": "(laughs)", "start": 0.3, "end": 0.6, "type": "audio_event"},
            {"text": "there", "start": 0.6, "end": 0.9, "confidence": 0.8}
        ]}], "unknown_top_level": true}"#;
        let t = import(s, "a").unwrap();
        assert_eq!(t.words.len(), 2);
        assert_eq!(t.words[0].start, 0.1);
        assert_eq!(t.words[0].speaker.as_deref(), Some("speaker_1"));
        assert_eq!(t.words[1].conf, 0.8);
        assert!(import(r#"{"text": "no words"}"#, "a").is_err());
    }

    #[test]
    fn own_format_roundtrip() {
        let t = Transcript {
            asset: "a".into(),
            lang: "en".into(),
            model: "m".into(),
            words: vec![crate::model::Word { w: "x".into(), start: 0.0, end: 1.0, conf: 1.0, speaker: None }],
            segments: vec![],
        };
        let s = serde_json::to_string(&t).unwrap();
        let back = load_any(&s, "a").unwrap();
        assert_eq!(back.words, t.words);
    }
}
