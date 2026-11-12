//! Media probing, fingerprinting and on-disk caches.

pub mod cache;
pub mod probe;

pub use probe::{probe, MediaInfo};

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Cheap but robust content fingerprint: size + sha256 of the first and last MiB.
/// (mtime is deliberately excluded so copies of the same file share caches.)
pub fn fingerprint(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut h = Sha256::new();
    h.update(len.to_le_bytes());
    let chunk = 1 << 20;
    let mut buf = vec![0u8; chunk.min(len as usize)];
    f.read_exact(&mut buf)?;
    h.update(&buf);
    if len > chunk as u64 {
        let start = len.saturating_sub(chunk as u64).max(chunk as u64);
        f.seek(SeekFrom::Start(start))?;
        buf.clear();
        f.read_to_end(&mut buf)?;
        h.update(&buf);
    }
    Ok(hex::encode(&h.finalize()[..16]))
}

/// Fingerprint a path that may be an image-sequence pattern or a directory (hashes all members).
pub fn fingerprint_any(path: &Path) -> Result<String> {
    let s = path.to_string_lossy();
    if s.contains('%') {
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut names: Vec<_> = std::fs::read_dir(dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        names.sort();
        let mut h = Sha256::new();
        for n in names {
            h.update(fingerprint(&n)?.as_bytes());
        }
        return Ok(hex::encode(&h.finalize()[..16]));
    }
    fingerprint(path)
}

/// Stable short hash of arbitrary bytes (used for cache keys).
pub fn hash_key(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    hex::encode(&h.finalize()[..16])
}
