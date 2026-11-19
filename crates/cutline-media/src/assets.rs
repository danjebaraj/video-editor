//! Probe every asset of a project (cached by fingerprint) and expose `MediaLookup`.

use crate::cache::Cache;
use crate::probe::{probe, MediaInfo};
use anyhow::Result;
use cutline_core::model::{AssetKind, Project};
use cutline_core::time::Fps;
use cutline_core::timeline::{default_kind_from_ext, MediaFacts, MediaLookup};
use cutline_deps::Toolchain;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct AssetMedia {
    pub id: String,
    /// The file the renderers read: the asset itself, or the decoded still of a HEIC/HEIF.
    pub path: PathBuf,
    /// The user's file when `path` is a decoded still of it (HEIC/HEIF).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
    pub fingerprint: String,
    pub info: Option<MediaInfo>,
    pub facts: MediaFacts,
}

impl AssetMedia {
    /// A variable-rate video that stores far fewer frames than it shows: a screen recording
    /// that only writes a frame when the screen changes. Its frame rate says nothing about its
    /// timing, a seek can land where no frame follows, and the frame on screen at a given time
    /// is the last one stored before it.
    pub fn sparse(&self) -> bool {
        self.info
            .as_ref()
            .and_then(|i| i.video.as_ref())
            .map(|v| v.vfr && v.fps_f64 > 0.0 && v.fps_f64 < 15.0)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProjectMedia {
    pub assets: BTreeMap<String, AssetMedia>,
}

impl MediaLookup for ProjectMedia {
    fn facts(&self, id: &str) -> Option<MediaFacts> {
        self.assets.get(id).map(|a| a.facts.clone())
    }
}

impl ProjectMedia {
    pub fn get(&self, id: &str) -> Option<&AssetMedia> {
        self.assets.get(id)
    }
}

pub fn facts_from_info(i: &MediaInfo, kind: AssetKind) -> MediaFacts {
    let v = i.video.as_ref();
    MediaFacts {
        exists: true,
        kind,
        duration: i.duration,
        has_video: v.is_some(),
        has_audio: i.audio.is_some(),
        alpha: v.map(|v| v.alpha).unwrap_or(false),
        width: v.map(|v| v.display_width).unwrap_or(0),
        height: v.map(|v| v.display_height).unwrap_or(0),
        fps: v.and_then(|v| Fps::parse(&v.fps)),
        rotation: v.map(|v| v.rotation).unwrap_or(0),
    }
}

/// Resolve an asset path relative to the project dir.
pub fn asset_path(dir: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        dir.join(pb)
    }
}

/// Where a track's sound ends, when it is followed by at least half a second of silence (a
/// quarter second of it is kept for the last note's tail). `None`: it plays to the end.
fn audible_end(tc: &Toolchain, path: &Path, duration: f64) -> Option<f64> {
    let run = |af: &str| -> Option<String> {
        // not `ffmpeg_cmd()`: both filters report through the log, which that one silences
        let out = std::process::Command::new(&tc.ffmpeg)
            .args(["-hide_banner", "-nostdin", "-loglevel", "info", "-i"])
            .arg(path)
            .args(["-vn", "-af", af, "-f", "null", "-"])
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let num =
        |line: &str, key: &str| -> Option<f64> { line.split(key).nth(1)?.split_whitespace().next()?.parse().ok() };
    // "Silence" is relative to the track: a last chord that has decayed 28 dB below the
    // track's average level is no longer heard under a voice or at the end of a video.
    let mean = run("volumedetect")?.lines().find_map(|l| num(l, "mean_volume:"))?;
    let floor = (mean - 28.0).clamp(-60.0, -35.0);
    let log = run(&format!("silencedetect=noise={floor:.1}dB:d=0.5"))?;
    // the last silence counts only when nothing audible follows it
    let start = log.lines().filter_map(|l| num(l, "silence_start:")).next_back()?;
    let ended = log.lines().filter_map(|l| num(l, "silence_end:")).next_back();
    let open = ended.map(|e| e < start || e >= duration - 0.05).unwrap_or(true);
    (open && start > 1.0 && duration - start >= 0.5).then(|| crate::round3(start + 0.25))
}

/// ffmpeg composes HEIF tile grids from 7.1 on; older builds decode one 512×512 tile.
pub const HEIF_MIN_FFMPEG: (u32, u32) = (7, 1);

/// An asset photo that could not be decoded (`E_IMAGE_DECODE` at `/assets/<id>`).
#[derive(Debug, Clone)]
pub struct AssetDecodeError {
    pub id: String,
    pub path: PathBuf,
    pub reason: String,
}

impl AssetDecodeError {
    fn new(id: &str, path: &Path, reason: String) -> Self {
        Self { id: id.to_string(), path: path.to_path_buf(), reason }
    }
}

impl std::fmt::Display for AssetDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "E_IMAGE_DECODE: asset `{}` ({}) can't be decoded: {}", self.id, self.path.display(), self.reason)
    }
}

impl std::error::Error for AssetDecodeError {}

/// Decoded stills sit next to the probe cache (`.cutline/cache/still`).
fn still_cache(probe_cache: &Cache) -> Cache {
    Cache::new(probe_cache.root.with_file_name("still"))
}

fn decode_heif(tc: &Toolchain, path: &Path, fp: &str, alpha: bool, cache: &Cache) -> Result<PathBuf> {
    let v = cutline_deps::tools::parse_version(&tc.version);
    if v < HEIF_MIN_FFMPEG && !crate::still::cached_path(fp, cache).is_file() {
        anyhow::bail!(
            "ffmpeg {} can't compose HEIF tile grids (7.1 or later needed; run `cutline deps install ffmpeg`)",
            tc.version
        );
    }
    crate::still::decode_with_fingerprint(tc, path, fp, alpha, cache)
}

fn decode_reason(e: &anyhow::Error) -> String {
    match e.downcast_ref::<crate::still::DecodeError>() {
        Some(d) => d.reason.clone(),
        None => format!("{e:#}"),
    }
}

/// Probe all assets. Missing files produce `exists: false` facts (validation reports them).
pub fn probe_project(tc: &Toolchain, p: &Project, dir: &Path, cache: &Cache) -> Result<ProjectMedia> {
    let mut out = ProjectMedia::default();
    for (id, a) in &p.assets {
        let path = asset_path(dir, &a.path);
        let ext_kind = default_kind_from_ext(&a.path);
        let declared = a.kind.unwrap_or(ext_kind);
        let is_seq = a.path.contains('%');
        let exists = if is_seq { path.parent().map(|d| d.is_dir()).unwrap_or(false) } else { path.exists() };
        if !exists {
            out.assets.insert(
                id.clone(),
                AssetMedia {
                    id: id.clone(),
                    path,
                    source: None,
                    fingerprint: String::new(),
                    info: None,
                    facts: MediaFacts {
                        exists: false,
                        kind: declared,
                        duration: 0.0,
                        has_video: false,
                        has_audio: false,
                        alpha: false,
                        width: 0,
                        height: 0,
                        fps: None,
                        rotation: 0,
                    },
                },
            );
            continue;
        }
        let fp = crate::fingerprint_any(&path)?;
        if matches!(declared, AssetKind::Html | AssetKind::Lottie | AssetKind::Svg) {
            let dur = a.duration.map(|t| t.seconds(Fps::new(30, 1), 0.0)).unwrap_or(3.0);
            out.assets.insert(
                id.clone(),
                AssetMedia {
                    id: id.clone(),
                    path,
                    source: None,
                    fingerprint: fp,
                    info: None,
                    facts: MediaFacts {
                        exists: true,
                        kind: declared,
                        duration: dur,
                        has_video: true,
                        has_audio: false,
                        alpha: a.alpha.unwrap_or(true),
                        width: a.width.unwrap_or(0),
                        height: a.height.unwrap_or(0),
                        fps: a.fps,
                        rotation: 0,
                    },
                },
            );
            continue;
        }
        let fps_hint = a.fps.map(|f| f.as_f64());
        let heif = crate::probe::is_heif(&path);
        // HEIC/HEIF probes changed (the whole tile grid, an image): earlier ones said "video"
        let version: &[u8] = if heif { b"probe-heif-v1" } else { b"probe-v1" };
        let key = crate::hash_key(&[fp.as_bytes(), format!("{fps_hint:?}").as_bytes(), version]);
        let info: MediaInfo = match cache.get_json(&key) {
            Some(i) => i,
            None => {
                let i = match probe(tc, &path, fps_hint) {
                    Ok(i) => i,
                    Err(e) if heif => return Err(AssetDecodeError::new(id, &path, format!("{e:#}")).into()),
                    Err(e) => return Err(e),
                };
                let _ = cache.put_json(&key, &i);
                i
            }
        };
        let kind = match (a.kind, info.kind.as_str()) {
            (Some(k), _) => k,
            (None, "image") => AssetKind::Image,
            (None, "audio") => AssetKind::Audio,
            (None, "image_sequence") => AssetKind::ImageSequence,
            _ => AssetKind::Video,
        };
        let mut facts = facts_from_info(&info, kind);
        if kind == AssetKind::Audio && a.role == Some(cutline_core::model::Role::Music) {
            // Generated music often ends in seconds of silence. The track ends where its sound
            // does, so `align: "end"` and `fade_out` land on the music rather than on nothing.
            let key = crate::hash_key(&[fp.as_bytes(), b"audible-end-v3"]);
            let end: Option<f64> = match cache.get_json::<Option<f64>>(&key) {
                Some(e) => e,
                None => {
                    let e = audible_end(tc, &path, info.duration);
                    let _ = cache.put_json(&key, &e);
                    e
                }
            };
            if let Some(e) = end {
                facts.duration = e;
            }
        }
        if kind == AssetKind::ImageSequence {
            let n = std::fs::read_dir(path.parent().unwrap()).map(|r| r.count()).unwrap_or(0);
            let f = a.fps.unwrap_or(Fps::new(30, 1));
            facts.duration = n as f64 / f.as_f64();
            facts.fps = Some(f);
        }
        // HEIC/HEIF photos are rendered from a decoded still; the fingerprint stays the photo's
        let (path, source) = if heif && kind == AssetKind::Image {
            let alpha = info.video.as_ref().is_some_and(|v| v.alpha);
            let still = decode_heif(tc, &path, &fp, alpha, &still_cache(cache))
                .map_err(|e| AssetDecodeError::new(id, &path, decode_reason(&e)))?;
            (still, Some(path))
        } else {
            (path, None)
        };
        out.assets
            .insert(id.clone(), AssetMedia { id: id.clone(), path, source, fingerprint: fp, info: Some(info), facts });
    }
    Ok(out)
}
