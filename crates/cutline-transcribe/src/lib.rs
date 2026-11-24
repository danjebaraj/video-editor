//! whisper.cpp integration (sidecar), Scribe import, transcript cache, word mapping.

pub mod align;
pub mod check;
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
/// (when allowed and available). Cached by asset fingerprint + model. Whisper works in a folder
/// under `work_root` (the project's `.cutline/tmp`: a sandboxed run may write nothing else),
/// removed afterwards.
pub fn get(
    tc: &cutline_deps::Toolchain,
    asset_id: &str,
    media: &Path,
    fingerprint: &str,
    transcript_file: Option<&Path>,
    model: &str,
    lang: Option<&str>,
    cache: &Cache,
    work_root: &Path,
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
    let work = work_root.join(format!("whisper-{}-{}", std::process::id(), &key[..12.min(key.len())]));
    let res = whisper::transcribe(tc, media, model, lang, &work);
    let _ = std::fs::remove_dir_all(&work);
    let mut t = res?;
    t.asset = asset_id.into();
    cache.store_json(&key, &t);
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
    cache.store_json(&key, t);
    Ok(())
}
