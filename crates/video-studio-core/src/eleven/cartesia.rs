//! A small blocking client for the Cartesia REST API: the other choice for the voice-over
//! (Settings › Voice). Only speech and the list of voices; music, sound effects and transcripts
//! stay with ElevenLabs. The key is sent as a bearer token and appears nowhere else.

use std::time::Duration;

use serde_json::{json, Value};

use super::client::{Generated, VoiceInfo};
use super::voices::VoiceGender;
use crate::{Error, Result};

const DEFAULT_BASE: &str = "https://api.cartesia.ai";
/// The API version these requests are written for.
const API_VERSION: &str = "2026-08-14";
/// The model used when neither the brand nor the command names one.
pub const DEFAULT_MODEL: &str = "sonic-3.6";
const MAX_BODY: u64 = 300 * 1024 * 1024;
/// Pages of 100 voices read for one listing.
const VOICE_PAGES: usize = 3;

pub struct Client {
    key: String,
    base: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(key: &str, base_url: Option<&str>) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(600)))
            .build();
        Client {
            key: key.to_string(),
            base: base_url.unwrap_or(DEFAULT_BASE).trim_end_matches('/').to_string(),
            agent: config.into(),
        }
    }

    fn fail(status: u16, body: &[u8]) -> Error {
        let text = String::from_utf8_lossy(body);
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v["message"]
                    .as_str()
                    .or(v["error"]["message"].as_str())
                    .or(v["error"].as_str())
                    .or(v["title"].as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| text.chars().take(300).collect());
        let hint = match status {
            401 | 403 => " (the API key was rejected)",
            402 | 429 => " (quota or rate limit)",
            _ => "",
        };
        Error::Http(format!("Cartesia returned {status}{hint}: {}", message.trim()))
    }

    fn finish(mut resp: ureq::http::Response<ureq::Body>) -> Result<Generated> {
        let status = resp.status().as_u16();
        let header = |resp: &ureq::http::Response<ureq::Body>, name: &str| {
            resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
        };
        let content_type = header(&resp, "content-type");
        let request_id = header(&resp, "x-request-id").or_else(|| header(&resp, "request-id"));
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(|e| Error::Http(format!("reading the Cartesia response failed: {e}")))?;
        if !(200..300).contains(&status) {
            return Err(Self::fail(status, &bytes));
        }
        Ok(Generated { bytes, status, content_type, request_id, credits: None })
    }

    fn get(&self, path: &str) -> Result<Generated> {
        let resp = self
            .agent
            .get(format!("{}{}", self.base, path))
            .header("Authorization", &format!("Bearer {}", self.key))
            .header("Cartesia-Version", API_VERSION)
            .call()
            .map_err(|e| Error::Http(format!("couldn't reach Cartesia: {e}")))?;
        Self::finish(resp)
    }

    /// Speech as MP3 (44.1 kHz, 128 kbit/s, like the ElevenLabs files). Of `settings` only
    /// `speed` (0.6–1.5), `volume` (0.5–2) and `emotion` are Cartesia's; anything else in it,
    /// such as a brand's ElevenLabs settings, is left out.
    pub fn tts(&self, voice: &str, text: &str, model: Option<&str>, settings: Option<&Value>) -> Result<Generated> {
        let mut body = json!({
            "model_id": model.unwrap_or(DEFAULT_MODEL),
            "transcript": text,
            "voice": {"id": voice},
            "output_format": {"container": "mp3", "sample_rate": 44100, "bit_rate": 128000},
        });
        if let Some(config) = generation_config(settings) {
            body["generation_config"] = config;
        }
        let resp = self
            .agent
            .post(format!("{}/tts/bytes", self.base))
            .header("Authorization", &format!("Bearer {}", self.key))
            .header("Cartesia-Version", API_VERSION)
            .send_json(&body)
            .map_err(|e| Error::Http(format!("couldn't reach Cartesia: {e}")))?;
        Self::finish(resp)
    }

    /// The voices the key can use, in the shape the ElevenLabs list has, so both are ranked and
    /// printed the same way. `gender` narrows the list on Cartesia's side.
    pub fn voices(&self, search: Option<&str>, gender: VoiceGender) -> Result<Vec<VoiceInfo>> {
        let mut out = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..VOICE_PAGES {
            let mut path = "/voices?limit=100".to_string();
            if let Some(s) = search.filter(|s| !s.trim().is_empty()) {
                path.push_str("&q=");
                path.push_str(&super::client::urlencode(s.trim()));
            }
            match gender {
                VoiceGender::Female => path.push_str("&gender=feminine"),
                VoiceGender::Male => path.push_str("&gender=masculine"),
                VoiceGender::Any => {}
            }
            if let Some(a) = &after {
                path.push_str("&starting_after=");
                path.push_str(&super::client::urlencode(a));
            }
            let r = self.get(&path)?;
            let v: Value = serde_json::from_slice(&r.bytes)
                .map_err(|e| Error::Http(format!("unexpected voices response: {e}")))?;
            // `{"data": [...], "has_more": …}`; older versions answered with a bare array.
            let page = v["data"].as_array().or(v.as_array()).cloned().unwrap_or_default();
            out.extend(page.iter().filter_map(voice_info));
            after = page.last().and_then(|x| x["id"].as_str()).map(str::to_string);
            if v["has_more"].as_bool() != Some(true) || after.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// Whether the key is accepted: asks for one voice.
    pub fn test(&self) -> Result<()> {
        self.get("/voices?limit=1").map(|_| ())
    }
}

/// `speed`, `volume` and `emotion` out of a settings object, when it has any of them.
fn generation_config(settings: Option<&Value>) -> Option<Value> {
    let s = settings?.as_object()?;
    let mut config = serde_json::Map::new();
    if let Some(speed) = s.get("speed").and_then(Value::as_f64) {
        config.insert("speed".into(), json!(speed.clamp(0.6, 1.5)));
    }
    if let Some(volume) = s.get("volume").and_then(Value::as_f64) {
        config.insert("volume".into(), json!(volume.clamp(0.5, 2.0)));
    }
    if let Some(emotion) = s.get("emotion").and_then(Value::as_str).filter(|e| !e.trim().is_empty()) {
        config.insert("emotion".into(), json!(emotion.trim()));
    }
    (!config.is_empty()).then_some(Value::Object(config))
}

fn voice_info(v: &Value) -> Option<VoiceInfo> {
    let mut labels = std::collections::BTreeMap::new();
    match v["gender"].as_str() {
        Some("feminine") => labels.insert("gender".to_string(), "female".to_string()),
        Some("masculine") => labels.insert("gender".to_string(), "male".to_string()),
        Some("gender_neutral") => labels.insert("gender".to_string(), "neutral".to_string()),
        _ => None,
    };
    if let Some(l) = v["language"].as_str().filter(|l| !l.is_empty()) {
        labels.insert("language".to_string(), l.to_string());
    }
    Some(VoiceInfo {
        voice_id: v["id"].as_str()?.to_string(),
        name: v["name"].as_str().unwrap_or("").to_string(),
        category: Some(if v["is_owner"].as_bool() == Some(true) { "yours" } else { "library" }.to_string()),
        description: v["description"].as_str().filter(|d| !d.trim().is_empty()).map(str::to_string),
        labels,
        created_at_unix: v["created_at"].as_str().and_then(crate::timeutil::parse).map(|t| t.timestamp()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cartesia_settings_are_sent() {
        assert_eq!(generation_config(None), None);
        // a brand's ElevenLabs settings carry nothing Cartesia takes but the speed
        let eleven = json!({"stability": 0.5, "similarity_boost": 0.8, "style": 0.2, "speed": 1.03});
        assert_eq!(generation_config(Some(&eleven)), Some(json!({"speed": 1.03})));
        let own = json!({"speed": 3.0, "emotion": " excited ", "volume": 1.2});
        assert_eq!(generation_config(Some(&own)), Some(json!({"speed": 1.5, "emotion": "excited", "volume": 1.2})));
        assert_eq!(generation_config(Some(&json!({"stability": 0.5}))), None);
    }

    #[test]
    fn voices_take_the_shape_of_the_other_list() {
        let v = voice_info(&json!({
            "id": "abc", "name": "Skylar", "description": "Approachable American female.",
            "gender": "feminine", "language": "en", "is_owner": false,
            "created_at": "2026-03-31T17:37:05.961874Z"
        }))
        .unwrap();
        assert_eq!((v.voice_id.as_str(), v.name.as_str()), ("abc", "Skylar"));
        assert_eq!(super::super::voices::gender_of(&v).as_deref(), Some("female"));
        assert_eq!(v.category.as_deref(), Some("library"));
        assert!(v.created_at_unix.unwrap() > 1_770_000_000);
        assert!(voice_info(&json!({"name": "no id"})).is_none());
    }
}
