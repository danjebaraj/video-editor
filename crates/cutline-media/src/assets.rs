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
    pub path: PathBuf,
    pub fingerprint: String,
    pub info: Option<MediaInfo>,
    pub facts: MediaFacts,
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
        let key = crate::hash_key(&[fp.as_bytes(), format!("{fps_hint:?}").as_bytes(), b"probe-v1"]);
        let info: MediaInfo = match cache.get_json(&key) {
            Some(i) => i,
            None => {
                let i = probe(tc, &path, fps_hint)?;
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
        if kind == AssetKind::ImageSequence {
            let n = std::fs::read_dir(path.parent().unwrap()).map(|r| r.count()).unwrap_or(0);
            let f = a.fps.unwrap_or(Fps::new(30, 1));
            facts.duration = n as f64 / f.as_f64();
            facts.fps = Some(f);
        }
        out.assets.insert(id.clone(), AssetMedia { id: id.clone(), path, fingerprint: fp, info: Some(info), facts });
    }
    Ok(out)
}
