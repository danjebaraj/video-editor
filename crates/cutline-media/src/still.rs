//! HEIC/HEIF photos as ordinary stills: each is decoded once (primary image, tiles composed,
//! upright, 8-bit sRGB) into a cached PNG, so every renderer path that loops an image input
//! works on it unchanged.

use crate::cache::Cache;
use anyhow::{Context, Result};
use cutline_deps::Toolchain;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Bumped when the decode changes, so old stills are not reused.
const KEY: &[u8] = b"heic-still-v1";

/// The hint shown with every `E_IMAGE_DECODE`.
pub const DECODE_HINT: &str = "Export the photo as JPEG (Photos › File › Export) and use that file instead.";

static DECODES: AtomicUsize = AtomicUsize::new(0);

/// ffmpeg decodes run by this process (tests use it to prove cache hits).
pub fn decode_count() -> usize {
    DECODES.load(Ordering::SeqCst)
}

/// A photo that could not be decoded; `reason` is ffmpeg's.
#[derive(Debug, Clone)]
pub struct DecodeError {
    pub path: PathBuf,
    pub reason: String,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "E_IMAGE_DECODE: can't decode {}: {}", self.path.display(), self.reason)
    }
}

impl std::error::Error for DecodeError {}

/// Where the decoded still of a file with this fingerprint lives (it may not exist yet).
pub fn cached_path(fingerprint: &str, cache: &Cache) -> PathBuf {
    cache.path(&crate::hash_key(&[fingerprint.as_bytes(), KEY]), "png")
}

/// The ffmpeg arguments that decode `input` into `out` (after the toolchain's own flags).
/// ffmpeg ≥ 7.1 composes a HEIF tile grid into one picture (through an internal filter graph,
/// so no `-vf` may be added) and applies its rotation and mirroring. `-pix_fmt` gives 8-bit
/// RGB, with alpha only when the decoded picture has it.
pub fn decode_args(input: &Path, out: &Path, alpha: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["-i".into(), input.to_string_lossy().into()];
    a.extend(["-map_metadata", "-1", "-frames:v", "1", "-pix_fmt"].map(String::from));
    a.push(if alpha { "rgba" } else { "rgb24" }.into());
    a.extend(["-update", "1", "-f", "image2"].map(String::from));
    a.push(out.to_string_lossy().into());
    a
}

/// Decode a HEIC/HEIF into an 8-bit PNG in `cache` (keyed by the file's fingerprint) and return
/// its path. A second call for an unchanged file runs no ffmpeg.
pub fn decode_still(tc: &Toolchain, path: &Path, cache: &Cache) -> Result<PathBuf> {
    let fp = crate::fingerprint(path).with_context(|| format!("reading {}", path.display()))?;
    decode_with_fingerprint(tc, path, &fp, false, cache)
}

pub(crate) fn decode_with_fingerprint(
    tc: &Toolchain,
    path: &Path,
    fp: &str,
    alpha: bool,
    cache: &Cache,
) -> Result<PathBuf> {
    let out = cached_path(fp, cache);
    if out.is_file() {
        return Ok(out);
    }
    std::fs::create_dir_all(out.parent().unwrap())?;
    let tmp = out.with_extension("tmp.png");
    DECODES.fetch_add(1, Ordering::SeqCst);
    let r = tc.ffmpeg_cmd().args(decode_args(path, &tmp, alpha)).output().context("running ffmpeg")?;
    if !r.status.success() || !tmp.is_file() {
        let _ = std::fs::remove_file(&tmp);
        let reason = ffmpeg_reason(&String::from_utf8_lossy(&r.stderr));
        return Err(DecodeError { path: path.to_path_buf(), reason }.into());
    }
    std::fs::rename(&tmp, &out)?;
    Ok(out)
}

/// The useful line of an ffmpeg error log: the first one naming bad data or a missing
/// decoder, else the last line. `[component @ 0x…]` prefixes are dropped.
pub fn ffmpeg_reason(stderr: &str) -> String {
    let lines: Vec<String> = stderr
        .lines()
        .map(|l| {
            let mut l = l.trim();
            while let Some(rest) = l.strip_prefix('[').and_then(|r| r.split_once("] ")).map(|(_, r)| r) {
                l = rest.trim();
            }
            l.to_string()
        })
        .filter(|l| !l.is_empty())
        .collect();
    let telling = ["Invalid data", "Decoding error", "not supported", "No such file", "Unsupported", "could not find"];
    lines
        .iter()
        .find(|l| telling.iter().any(|t| l.contains(t)))
        .or(lines.last())
        .cloned()
        .unwrap_or_else(|| "ffmpeg could not read the file".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_prefers_the_telling_line() {
        let log = "[in#0/mov @ 0x1] stream 18, offset 0x29c1: partial file\n\
                   [hevc @ 0x2] Invalid NAL unit size (456 > 7).\n\
                   [vist#0:17/hevc @ 0x3] [dec:hevc @ 0x4] Decoding error: Invalid data found when processing input\n\
                   [vist#0:17/hevc @ 0x3] [dec:hevc @ 0x4] Terminating thread with return code -1";
        assert_eq!(ffmpeg_reason(log), "Decoding error: Invalid data found when processing input");
        assert_eq!(ffmpeg_reason("\n  \n"), "ffmpeg could not read the file");
        assert_eq!(ffmpeg_reason("something odd\nlast words\n"), "last words");
    }

    #[test]
    fn args_keep_one_upright_frame() {
        let a = decode_args(Path::new("in.heic"), Path::new("out.png"), false);
        assert!(a.windows(2).any(|w| w[0] == "-pix_fmt" && w[1] == "rgb24"));
        assert!(!a.iter().any(|x| x == "-vf"), "a tile grid takes no simple filter");
        assert_eq!(a.first().map(String::as_str), Some("-i"));
        assert!(a.windows(2).any(|w| w[0] == "-frames:v" && w[1] == "1"));
        assert_eq!(a.last().map(String::as_str), Some("out.png"));
    }
}
