//! The ElevenLabs helper behind `video-studio eleven …` (spec §14).
//!
//! Claude calls the helper through its restricted Bash permission. The helper reads the API key
//! from [`Secrets`], never prints it, and only runs inside a run: its context (run folder, budget,
//! log path) comes from a file named by `VIDEO_STUDIO_RUN_CONTEXT`, which the app writes outside
//! the folders Claude can read. Every call is quoted first, checked against the run's credit cap,
//! served from the cache when an identical request was made before, and logged.

mod client;

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub use client::{Client, Subscription, VoiceInfo};

use crate::claude::broker::append_line;
use crate::claude::Mode;
use crate::platform::{Secrets, ELEVEN_KEY};
use crate::{paths, timeutil, Error, Result};

pub const CONTEXT_ENV: &str = "VIDEO_STUDIO_RUN_CONTEXT";
pub const BUNDLED_PRICING: &str = include_str!("../../data/eleven-pricing.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pricing {
    pub tts_credits_per_char: BTreeMap<String, f64>,
    pub sfx_credits_per_second: f64,
    pub music_credits_per_second: f64,
    pub scribe_credits_per_minute: f64,
}

impl Pricing {
    pub fn bundled() -> Self {
        serde_json::from_str(BUNDLED_PRICING).expect("bundled eleven-pricing.json is valid")
    }
    pub fn tts_rate(&self, model: Option<&str>) -> f64 {
        model.and_then(|m| self.tts_credits_per_char.get(m)).or_else(|| self.tts_credits_per_char.get("default")).copied().unwrap_or(1.0)
    }
}

/// What the app tells the helper about the run it is part of. Written to
/// `<library>/run-contexts/<run id>/context.json`; Claude can't read that folder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelperContext {
    pub run_id: String,
    pub run_folder: Utf8PathBuf,
    pub mode: Mode,
    /// `<run folder>/logs`
    pub logs_dir: Utf8PathBuf,
    /// The real cutline binary the `cutline` proxy runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutline: Option<Utf8PathBuf>,
    pub credit_cap: u64,
    pub cache_dir: Utf8PathBuf,
    #[serde(default)]
    pub cache_limit_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_voice_settings: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// API base URL override (the fake server in tests).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Test key used instead of the secret store, only ever set by tests with a fake server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_api_key: Option<String>,
}

impl HelperContext {
    /// Loads the context named by `VIDEO_STUDIO_RUN_CONTEXT`. Without it the helper refuses to
    /// run, so it can't be used outside a run.
    pub fn from_env() -> Result<Self> {
        let path = std::env::var(CONTEXT_ENV).ok().filter(|p| !p.is_empty()).ok_or_else(|| Error::Denied("This command only works inside a Syncfusion Video Studio run.".into()))?;
        let ctx: HelperContext = paths::read_json(Utf8Path::new(&path)).map_err(|_| Error::Denied("This command only works inside a Syncfusion Video Studio run (the run context is missing).".into()))?;
        if !ctx.run_folder.is_dir() {
            return Err(Error::Denied("The run this command belongs to no longer exists.".into()));
        }
        Ok(ctx)
    }

    pub fn audio_dir(&self) -> Utf8PathBuf {
        self.run_folder.join("audio")
    }
    pub fn log_path(&self) -> Utf8PathBuf {
        self.logs_dir.join("eleven.ndjson")
    }

    fn api_key(&self, secrets: &dyn Secrets) -> Result<String> {
        if let Some(k) = self.test_api_key.as_deref().filter(|k| !k.is_empty()) {
            return Ok(k.to_string());
        }
        match secrets.get(ELEVEN_KEY)? {
            Some(k) if !k.trim().is_empty() => Ok(k.trim().to_string()),
            _ => Err(Error::Environment("No ElevenLabs API key is set. Ask the user to add one in Settings › ElevenLabs; until then, use media files for voice and music.".into())),
        }
    }

    fn client(&self, secrets: &dyn Secrets) -> Result<Client> {
        Ok(Client::new(&self.api_key(secrets)?, self.base_url.as_deref()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Tts { voice: String, text: String, model: Option<String>, settings: Option<Value> },
    Music { prompt: String, length_s: f64 },
    Sfx { prompt: String, length_s: f64 },
    Scribe { input: Utf8PathBuf },
}

impl Request {
    pub fn command(&self) -> &'static str {
        match self {
            Request::Tts { .. } => "tts",
            Request::Music { .. } => "music",
            Request::Sfx { .. } => "sfx",
            Request::Scribe { .. } => "scribe",
        }
    }

    fn extension(&self) -> &'static str {
        match self {
            Request::Scribe { .. } => "json",
            _ => "mp3",
        }
    }

    /// The request as logged and hashed: everything that changes the result, and never the key.
    pub fn describe(&self) -> Value {
        match self {
            Request::Tts { voice, text, model, settings } => json!({"command": "tts", "voice": voice, "model": model, "settings": settings, "text": text}),
            Request::Music { prompt, length_s } => json!({"command": "music", "prompt": prompt, "length_s": length_s}),
            Request::Sfx { prompt, length_s } => json!({"command": "sfx", "prompt": prompt, "length_s": length_s}),
            Request::Scribe { input } => {
                let hash = crate::media::fingerprint(input).map(|f| f.quick_hash).unwrap_or_default();
                json!({"command": "scribe", "input": input, "content": hash})
            }
        }
    }

    /// Cache key: a hash of the endpoint, parameters and text.
    pub fn cache_key(&self) -> String {
        let mut d = self.describe();
        // The scribe input's location doesn't matter, only its content.
        if let (Request::Scribe { .. }, Some(o)) = (self, d.as_object_mut()) {
            o.remove("input");
        }
        let digest = Sha256::digest(d.to_string().as_bytes());
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Estimated credits for a request, without calling the API.
pub fn quote(req: &Request, pricing: &Pricing, audio_seconds: Option<f64>) -> u64 {
    let credits = match req {
        Request::Tts { text, model, .. } => text.chars().count() as f64 * pricing.tts_rate(model.as_deref()),
        Request::Music { length_s, .. } => length_s * pricing.music_credits_per_second,
        Request::Sfx { length_s, .. } => length_s * pricing.sfx_credits_per_second,
        Request::Scribe { .. } => audio_seconds.unwrap_or(60.0) / 60.0 * pricing.scribe_credits_per_minute,
    };
    credits.ceil().max(0.0) as u64
}

/// Credits this run has spent so far: the sum over the call log.
pub fn spent(log: &Utf8Path) -> u64 {
    std::fs::read_to_string(log)
        .map(|t| t.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ok"].as_bool() == Some(true)).filter_map(|v| v["credits"].as_u64()).sum())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CallResult {
    pub out: Utf8PathBuf,
    pub credits: u64,
    pub cached: bool,
    pub bytes: u64,
    pub spent: u64,
    pub cap: u64,
}

/// Where an `--out` value lands: relative paths go under `<run folder>/audio/`, and nothing may
/// leave the run folder.
pub fn resolve_out(ctx: &HelperContext, out: &str, extension: &str) -> Result<Utf8PathBuf> {
    let given = Utf8Path::new(out);
    let mut p = if given.is_absolute() {
        paths::normalize(given)
    } else if given.starts_with("audio") {
        paths::normalize(&ctx.run_folder.join(given))
    } else {
        paths::normalize(&ctx.audio_dir().join(given))
    };
    if p.extension().is_none() {
        p.set_extension(extension);
    }
    if !paths::is_inside(&p, &ctx.run_folder) {
        return Err(Error::Denied(format!("--out must be inside the run folder ({}); audio normally goes in audio/.", ctx.run_folder)));
    }
    Ok(p)
}

fn with_lock<T>(ctx: &HelperContext, f: impl FnOnce() -> Result<T>) -> Result<T> {
    use fs2::FileExt;
    std::fs::create_dir_all(&ctx.logs_dir)?;
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(ctx.logs_dir.join(".eleven.lock"))?;
    lock.lock_exclusive()?;
    let r = f();
    let _ = FileExt::unlock(&lock);
    r
}

fn log_call(ctx: &HelperContext, req: &Request, out: Option<&Utf8Path>, quoted: u64, credits: u64, cached: bool, bytes: u64, error: Option<&str>, summary: Value) {
    let entry = json!({
        "ts": timeutil::now_rfc3339(),
        "command": req.command(),
        "request": req.describe(),
        "quoted": quoted,
        "credits": credits,
        "cache": if cached { "hit" } else { "miss" },
        "out": out,
        "bytes": bytes,
        "ok": error.is_none(),
        "error": error,
        "response": summary,
    });
    append_line(&ctx.log_path(), &entry.to_string());
}

/// Runs one generation request: quote, budget check, cache lookup, API call, log.
pub fn execute(ctx: &HelperContext, req: &Request, out: &str, secrets: &dyn Secrets) -> Result<CallResult> {
    if ctx.mode != Mode::Production {
        return Err(Error::Denied("Audio isn't generated in a dry run. Use `video-studio eleven quote …` to estimate credits.".into()));
    }
    let dest = resolve_out(ctx, out, req.extension())?;
    let pricing = Pricing::bundled();
    if let Request::Scribe { input } = req {
        if !input.is_file() {
            return Err(Error::NotFound(format!("{input} doesn't exist")));
        }
    }
    if let Request::Tts { text, voice, .. } = req {
        if text.trim().is_empty() {
            return Err(Error::other("The text to voice is empty."));
        }
        if voice.trim().is_empty() {
            return Err(Error::other("No voice was given and the brand preset has no default voice. Pass --voice <id> (see `video-studio eleven voices`)."));
        }
    }
    let quoted = quote(req, &pricing, None);
    let key = req.cache_key();
    let cache_file = ctx.cache_dir.join(format!("{key}.{}", req.extension()));

    with_lock(ctx, || {
        // A cache hit costs nothing and never touches the API.
        if cache_file.is_file() {
            if let Some(dir) = dest.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::copy(&cache_file, &dest)?;
            let _ = filetime_touch(&cache_file);
            let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
            log_call(ctx, req, Some(&dest), quoted, 0, true, bytes, None, json!({"source": "cache"}));
            let total = spent(&ctx.log_path());
            return Ok(CallResult { out: dest.clone(), credits: 0, cached: true, bytes, spent: total, cap: ctx.credit_cap });
        }
        let so_far = spent(&ctx.log_path());
        if so_far + quoted > ctx.credit_cap {
            let msg = format!(
                "This call needs about {quoted} credits, but the run has used {so_far} of its {} credit cap ({} left). Shorten the text or prompt, reuse audio already in audio/, or ask the user to raise the cap in the run config.",
                ctx.credit_cap,
                ctx.credit_cap.saturating_sub(so_far)
            );
            log_call(ctx, req, None, quoted, 0, false, 0, Some(&msg), Value::Null);
            return Err(Error::Budget(msg));
        }
        let client = ctx.client(secrets)?;
        let result = match req {
            Request::Tts { voice, text, model, settings } => client.tts(voice, text, model.as_deref().or(ctx.default_model.as_deref()), settings.as_ref()),
            Request::Music { prompt, length_s } => client.music(prompt, *length_s),
            Request::Sfx { prompt, length_s } => client.sfx(prompt, *length_s),
            Request::Scribe { input } => client.scribe(input),
        };
        match result {
            Err(e) => {
                log_call(ctx, req, None, quoted, 0, false, 0, Some(&e.to_string()), Value::Null);
                Err(e)
            }
            Ok(resp) => {
                // The API's own figure when it reports one, else the quote.
                let credits = resp.credits.unwrap_or(quoted);
                paths::write_atomic(&dest, &resp.bytes)?;
                paths::write_atomic(&cache_file, &resp.bytes)?;
                let bytes = resp.bytes.len() as u64;
                log_call(ctx, req, Some(&dest), quoted, credits, false, bytes, None, json!({"status": resp.status, "content_type": resp.content_type, "request_id": resp.request_id}));
                if ctx.cache_limit_bytes > 0 {
                    prune_cache(&ctx.cache_dir, ctx.cache_limit_bytes);
                }
                Ok(CallResult { out: dest.clone(), credits, cached: false, bytes, spent: so_far + credits, cap: ctx.credit_cap })
            }
        }
    })
}

fn filetime_touch(path: &Utf8Path) -> std::io::Result<()> {
    // Rewriting the modification time marks the entry as recently used for pruning.
    let f = std::fs::OpenOptions::new().append(true).open(path)?;
    f.set_modified(std::time::SystemTime::now())
}

/// Total size of the cache in bytes.
pub fn cache_size(dir: &Utf8Path) -> u64 {
    std::fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()).unwrap_or(0)
}

/// Removes the least recently used cache entries until the cache fits in `limit` bytes.
pub fn prune_cache(dir: &Utf8Path, limit: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = rd.flatten().filter_map(|e| e.metadata().ok().filter(|m| m.is_file()).map(|m| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), e.path()))).collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, size, path) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
}

/// Settings › ElevenLabs › Clear.
pub fn clear_cache(dir: &Utf8Path) -> Result<()> {
    if dir.is_dir() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

/// The shared cache folder in the app library.
pub fn cache_dir(library: &Utf8Path) -> Utf8PathBuf {
    library.join("cache/eleven")
}

/// Settings › ElevenLabs › Test: checks the key against the API and returns the remaining credits.
pub fn test_key(key: &str, base_url: Option<&str>) -> Result<Subscription> {
    Client::new(key, base_url).subscription()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::MemorySecrets;

    fn ctx(dir: &Utf8Path, mode: Mode, cap: u64) -> HelperContext {
        let run = dir.join("run");
        std::fs::create_dir_all(run.join("logs")).unwrap();
        HelperContext { run_id: "run_1".into(), run_folder: run.clone(), mode, logs_dir: run.join("logs"), cutline: None, credit_cap: cap, cache_dir: dir.join("cache"), cache_limit_bytes: 0, default_voice: None, default_voice_settings: None, default_model: None, base_url: Some("http://127.0.0.1:9".into()), test_api_key: None }
    }

    fn tts(text: &str) -> Request {
        Request::Tts { voice: "v1".into(), text: text.into(), model: None, settings: None }
    }

    #[test]
    fn quotes() {
        let p = Pricing::bundled();
        assert_eq!(quote(&tts("Hello world"), &p, None), 11);
        assert_eq!(quote(&Request::Tts { voice: "v".into(), text: "Hello world".into(), model: Some("eleven_flash_v2_5".into()), settings: None }, &p, None), 6);
        assert_eq!(quote(&Request::Sfx { prompt: "whoosh".into(), length_s: 2.0 }, &p, None), 80);
        assert_eq!(quote(&Request::Music { prompt: "calm".into(), length_s: 30.0 }, &p, None), 900);
        assert_eq!(quote(&Request::Scribe { input: "a.mp3".into() }, &p, Some(90.0)), 150);
    }

    #[test]
    fn cache_keys_follow_the_request() {
        let a = tts("Hello");
        assert_eq!(a.cache_key(), tts("Hello").cache_key());
        assert_ne!(a.cache_key(), tts("Hello!").cache_key());
        assert_ne!(a.cache_key(), Request::Tts { voice: "v2".into(), text: "Hello".into(), model: None, settings: None }.cache_key());
        assert_ne!(a.cache_key(), Request::Tts { voice: "v1".into(), text: "Hello".into(), model: None, settings: Some(json!({"stability": 0.3})) }.cache_key());
        assert_eq!(a.cache_key().len(), 64);
    }

    #[test]
    fn out_paths_stay_in_the_run_folder() {
        let d = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(d.path()).unwrap();
        let c = ctx(dir, Mode::Production, 100);
        assert_eq!(resolve_out(&c, "vo/scene1", "mp3").unwrap(), c.run_folder.join("audio/vo/scene1.mp3"));
        assert_eq!(resolve_out(&c, "audio/music.mp3", "mp3").unwrap(), c.run_folder.join("audio/music.mp3"));
        assert_eq!(resolve_out(&c, c.run_folder.join("work/x.mp3").as_str(), "mp3").unwrap(), c.run_folder.join("work/x.mp3"));
        assert!(matches!(resolve_out(&c, "../../escape.mp3", "mp3"), Err(Error::Denied(_))));
        assert!(matches!(resolve_out(&c, "/tmp/x.mp3", "mp3"), Err(Error::Denied(_))));
    }

    #[test]
    fn refuses_outside_a_run_and_in_dry_runs() {
        let d = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(d.path()).unwrap();
        let secrets = MemorySecrets::with(ELEVEN_KEY, "k");
        let dry = ctx(dir, Mode::DryRun, 1000);
        assert!(matches!(execute(&dry, &tts("Hi"), "a.mp3", &secrets), Err(Error::Denied(m)) if m.contains("dry run")));
        // no context file named in the environment
        std::env::remove_var(CONTEXT_ENV);
        assert!(matches!(HelperContext::from_env(), Err(Error::Denied(m)) if m.contains("only works inside")));
    }

    #[test]
    fn cap_is_enforced_before_the_api_is_called() {
        let d = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(d.path()).unwrap();
        let c = ctx(dir, Mode::Production, 10);
        let secrets = MemorySecrets::with(ELEVEN_KEY, "k");
        // 26 characters against a cap of 10: refused with a message Claude can act on, and logged
        let err = execute(&c, &tts("This sentence is too long."), "a.mp3", &secrets).unwrap_err();
        assert!(matches!(&err, Error::Budget(m) if m.contains("about 26 credits") && m.contains("Shorten the text")), "{err}");
        let log: Vec<Value> = std::fs::read_to_string(c.log_path()).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(log.len(), 1);
        assert_eq!((log[0]["ok"].as_bool(), log[0]["credits"].as_u64(), log[0]["quoted"].as_u64()), (Some(false), Some(0), Some(26)));
        assert!(!log[0].to_string().contains("\"k\""), "the key is never logged");
        assert_eq!(spent(&c.log_path()), 0);
    }

    #[test]
    fn cache_hits_cost_nothing_and_need_no_key() {
        let d = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(d.path()).unwrap();
        let c = ctx(dir, Mode::Production, 5);
        let req = tts("A line that was voiced in an earlier run");
        std::fs::create_dir_all(&c.cache_dir).unwrap();
        std::fs::write(c.cache_dir.join(format!("{}.mp3", req.cache_key())), b"cached-audio").unwrap();
        // no key at all, and a cap below the quote: the cache still serves it
        let r = execute(&c, &req, "vo.mp3", &MemorySecrets::default()).unwrap();
        assert!(r.cached && r.credits == 0 && r.spent == 0);
        assert_eq!(std::fs::read(c.run_folder.join("audio/vo.mp3")).unwrap(), b"cached-audio");
        let log: Value = serde_json::from_str(std::fs::read_to_string(c.log_path()).unwrap().lines().next().unwrap()).unwrap();
        assert_eq!((log["cache"].as_str(), log["credits"].as_u64(), log["ok"].as_bool()), (Some("hit"), Some(0), Some(true)));
    }

    #[test]
    fn missing_key_is_reported_plainly() {
        let d = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(d.path()).unwrap();
        let c = ctx(dir, Mode::Production, 1000);
        let err = execute(&c, &tts("Hello"), "a.mp3", &MemorySecrets::default()).unwrap_err();
        assert!(matches!(&err, Error::Environment(m) if m.contains("No ElevenLabs API key")), "{err}");
        assert!(matches!(execute(&c, &tts("  "), "a.mp3", &MemorySecrets::default()), Err(Error::Other(m)) if m.contains("empty")));
        assert!(matches!(execute(&c, &Request::Tts { voice: "".into(), text: "Hi".into(), model: None, settings: None }, "a.mp3", &MemorySecrets::default()), Err(Error::Other(m)) if m.contains("No voice")));
    }

    #[test]
    fn pruning_removes_least_recently_used_first() {
        let d = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(d.path()).unwrap();
        for (i, name) in ["old", "mid", "new"].iter().enumerate() {
            let p = dir.join(format!("{name}.mp3"));
            std::fs::write(&p, vec![0u8; 100]).unwrap();
            let f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + i as u64 * 100)).unwrap();
        }
        assert_eq!(cache_size(dir), 300);
        prune_cache(dir, 150);
        assert!(!dir.join("old.mp3").exists() && !dir.join("mid.mp3").exists() && dir.join("new.mp3").exists());
        clear_cache(dir).unwrap();
        assert_eq!(cache_size(dir), 0);
    }
}
