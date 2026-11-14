//! A small blocking client for the ElevenLabs REST API. The key is sent in the `xi-api-key`
//! header and appears nowhere else.

use std::time::Duration;

use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{Error, Result};

const DEFAULT_BASE: &str = "https://api.elevenlabs.io";
const MAX_BODY: u64 = 300 * 1024 * 1024;

pub struct Client {
    key: String,
    base: String,
    agent: ureq::Agent,
}

/// A successful generation response.
pub struct Generated {
    pub bytes: Vec<u8>,
    pub status: u16,
    pub content_type: Option<String>,
    pub request_id: Option<String>,
    /// Credits the API says the call cost, when it reports them.
    pub credits: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Subscription {
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub character_count: u64,
    #[serde(default)]
    pub character_limit: u64,
}

impl Subscription {
    pub fn remaining(&self) -> u64 {
        self.character_limit.saturating_sub(self.character_count)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceInfo {
    pub voice_id: String,
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

impl Client {
    pub fn new(key: &str, base_url: Option<&str>) -> Self {
        let config = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(600))).build();
        Client { key: key.to_string(), base: base_url.unwrap_or(DEFAULT_BASE).trim_end_matches('/').to_string(), agent: config.into() }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn fail(status: u16, body: &[u8]) -> Error {
        // ElevenLabs errors look like {"detail": {"status": "...", "message": "..."}} or {"detail": "..."}.
        let text = String::from_utf8_lossy(body);
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["detail"]["message"].as_str().or(v["detail"].as_str()).or(v["message"].as_str()).map(str::to_string))
            .unwrap_or_else(|| text.chars().take(300).collect());
        let hint = match status {
            401 => " (the API key was rejected)",
            402 | 429 => " (quota or rate limit)",
            _ => "",
        };
        Error::Http(format!("ElevenLabs returned {status}{hint}: {}", message.trim()))
    }

    fn finish(mut resp: ureq::http::Response<ureq::Body>) -> Result<Generated> {
        let status = resp.status().as_u16();
        let header = |resp: &ureq::http::Response<ureq::Body>, name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let content_type = header(&resp, "content-type");
        let request_id = header(&resp, "request-id");
        let credits = header(&resp, "character-cost").and_then(|v| v.parse::<u64>().ok());
        let bytes = resp.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(|e| Error::Http(format!("reading the ElevenLabs response failed: {e}")))?;
        if !(200..300).contains(&status) {
            return Err(Self::fail(status, &bytes));
        }
        Ok(Generated { bytes, status, content_type, request_id, credits })
    }

    fn get(&self, path: &str) -> Result<Generated> {
        let resp = self.agent.get(self.url(path)).header("xi-api-key", &self.key).call().map_err(|e| Error::Http(format!("couldn't reach ElevenLabs: {e}")))?;
        Self::finish(resp)
    }

    fn post_json(&self, path: &str, body: &Value) -> Result<Generated> {
        let resp = self.agent.post(self.url(path)).header("xi-api-key", &self.key).send_json(body).map_err(|e| Error::Http(format!("couldn't reach ElevenLabs: {e}")))?;
        Self::finish(resp)
    }

    /// Account usage: used and total credits for the period.
    pub fn subscription(&self) -> Result<Subscription> {
        let r = self.get("/v1/user/subscription")?;
        serde_json::from_slice(&r.bytes).map_err(|e| Error::Http(format!("unexpected subscription response: {e}")))
    }

    pub fn voices(&self, search: Option<&str>) -> Result<Vec<VoiceInfo>> {
        let mut path = "/v2/voices?page_size=100".to_string();
        if let Some(s) = search.filter(|s| !s.trim().is_empty()) {
            path.push_str("&search=");
            path.push_str(&urlencode(s.trim()));
        }
        let r = self.get(&path)?;
        let v: Value = serde_json::from_slice(&r.bytes).map_err(|e| Error::Http(format!("unexpected voices response: {e}")))?;
        Ok(v["voices"].as_array().map(|a| a.iter().filter_map(|x| serde_json::from_value(x.clone()).ok()).collect()).unwrap_or_default())
    }

    pub fn tts(&self, voice: &str, text: &str, model: Option<&str>, settings: Option<&Value>) -> Result<Generated> {
        let mut body = json!({"text": text, "model_id": model.unwrap_or("eleven_multilingual_v2")});
        if let Some(s) = settings.filter(|s| s.is_object()) {
            body["voice_settings"] = s.clone();
        }
        self.post_json(&format!("/v1/text-to-speech/{}?output_format=mp3_44100_128", urlencode(voice)), &body)
    }

    pub fn music(&self, prompt: &str, length_s: f64) -> Result<Generated> {
        self.post_json("/v1/music?output_format=mp3_44100_128", &json!({"prompt": prompt, "music_length_ms": (length_s * 1000.0).round() as u64}))
    }

    pub fn sfx(&self, prompt: &str, length_s: f64) -> Result<Generated> {
        self.post_json("/v1/sound-generation?output_format=mp3_44100_128", &json!({"text": prompt, "duration_seconds": length_s}))
    }

    /// Scribe speech-to-text with word timestamps; the JSON is what `cutline transcribe` imports.
    pub fn scribe(&self, input: &Utf8Path) -> Result<Generated> {
        let audio = std::fs::read(input)?;
        let boundary = format!("----videostudio{}", crate::ids::new_id("b"));
        let mut body: Vec<u8> = Vec::with_capacity(audio.len() + 512);
        let mut field = |name: &str, value: &str| {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
        };
        field("model_id", "scribe_v1");
        field("timestamps_granularity", "word");
        let name = input.file_name().unwrap_or("audio").replace('"', "");
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        body.extend_from_slice(&audio);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let resp = self
            .agent
            .post(self.url("/v1/speech-to-text"))
            .header("xi-api-key", &self.key)
            .header("content-type", &format!("multipart/form-data; boundary={boundary}"))
            .send(&body[..])
            .map_err(|e| Error::Http(format!("couldn't reach ElevenLabs: {e}")))?;
        Self::finish(resp)
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_come_from_the_response() {
        let e = Client::fail(401, br#"{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}"#).to_string();
        assert_eq!(e, "ElevenLabs returned 401 (the API key was rejected): Invalid API key");
        assert!(Client::fail(429, br#"{"detail":"Too many requests"}"#).to_string().contains("quota or rate limit): Too many requests"));
        assert!(Client::fail(500, b"<html>oops</html>").to_string().contains("oops"));
        assert_eq!(urlencode("a b/ç"), "a%20b%2F%C3%A7");
        assert_eq!(Subscription { tier: "pro".into(), character_count: 400, character_limit: 1000 }.remaining(), 600);
    }
}
