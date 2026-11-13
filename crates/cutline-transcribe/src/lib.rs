//! whisper.cpp integration (sidecar), Scribe import, transcript cache, word mapping.

pub mod mapping;
pub mod model;
pub mod scribe;
pub mod text;
pub mod whisper;

pub use model::{Transcript, Word};

use anyhow::Result;
use cutline_media::cache::Cache;
use std::path::Path;

/// Where a transcript came from.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum Source {
    File(String),
    Cache,
    Whisper(String),
}

/// Get the transcript for an asset: explicit file (Scribe or cutline JSON) → cache → whisper
/// (when allowed and available). Cached by asset fingerprint + model.
pub fn get(
    tc: &cutline_deps::Toolchain,
    asset_id: &str,
    media: &Path,
    fingerprint: &str,
    transcript_file: Option<&Path>,
    model: &str,
    lang: Option<&str>,
    cache: &Cache,
    allow_whisper: bool,
) -> Result<Option<(Transcript, Source)>> {
    if let Some(f) = transcript_file {
        let text = std::fs::read_to_string(f)?;
        let t = scribe::load_any(&text, asset_id)?;
        return Ok(Some((t, Source::File(f.display().to_string()))));
    }
    let key = cutline_media::hash_key(&[
        fingerprint.as_bytes(),
        model.as_bytes(),
        lang.unwrap_or("auto").as_bytes(),
        b"transcript-v1",
    ]);
    if let Some(t) = cache.get_json::<Transcript>(&key) {
        return Ok(Some((t, Source::Cache)));
    }
    if !allow_whisper || whisper::available(model).is_err() {
        return Ok(None);
    }
    let work = cache.root.join("work").join(&key);
    let mut t = whisper::transcribe(tc, media, model, lang, &work)?;
    t.asset = asset_id.into();
    cache.put_json(&key, &t)?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(Some((t, Source::Whisper(model.into()))))
}

/// Store an imported transcript in the cache (so later renders find it without the file).
pub fn put_cache(cache: &Cache, fingerprint: &str, model: &str, lang: Option<&str>, t: &Transcript) -> Result<()> {
    let key = cutline_media::hash_key(&[
        fingerprint.as_bytes(),
        model.as_bytes(),
        lang.unwrap_or("auto").as_bytes(),
        b"transcript-v1",
    ]);
    cache.put_json(&key, t)?;
    Ok(())
}
