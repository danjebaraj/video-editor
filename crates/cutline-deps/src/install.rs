//! Resolving upstream sources and installing tools into the tools dir.
//!
//! Layout: `<tools>/ffmpeg/<version>/{ffmpeg,ffprobe}` with `<tools>/ffmpeg/current` (and
//! `previous`) holding the active version name. Plain files are used instead of symlinks so the
//! layout works on Windows.

use crate::download::{download, extract, get_text, make_executable, parse_sha256_sidecar, resolve_url};
use crate::lockfile::{LockEntry, Lockfile};
use crate::paths::{exe_name, platform_id, tools_dir};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A downloadable artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Logical name: `ffmpeg` or `ffprobe` (archive containing that binary).
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

/// A resolved upstream release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub source: String,
}

/// Resolve the latest ffmpeg release for this platform.
///
/// `CUTLINE_FFMPEG_MANIFEST_URL` points at a JSON [`Release`] (used by tests with a mock server
/// and by anyone hosting an internal mirror).
pub fn resolve_latest_ffmpeg() -> Result<Release> {
    if let Ok(u) = std::env::var("CUTLINE_FFMPEG_MANIFEST_URL") {
        let mut r: Release = serde_json::from_str(&get_text(&u)?).context("parsing ffmpeg manifest")?;
        r.source = format!("manifest:{u}");
        return Ok(r);
    }
    let plat = platform_id();
    let (os, arch) = plat.split_once('-').unwrap_or(("", ""));
    match os {
        "macos" | "linux" => {
            let arch = match arch {
                "arm64" => "arm64",
                "x86_64" => "amd64",
                a => bail!("unsupported architecture {a}"),
            };
            resolve_martin_riedl(os, arch)
        }
        "windows" => resolve_btbn(),
        o => bail!("no ffmpeg source for OS {o}; install ffmpeg on PATH"),
    }
}

/// Martin Riedl static builds (release channel). Version is encoded in the redirect target
/// (`/download/macos/arm64/<ts>_<version>/ffmpeg.zip`) and sha256 sidecars are published.
fn resolve_martin_riedl(os: &str, arch: &str) -> Result<Release> {
    let mut artifacts = vec![];
    let mut version = String::new();
    for name in ["ffmpeg", "ffprobe"] {
        let u = format!("https://ffmpeg.martin-riedl.de/redirect/latest/{os}/{arch}/release/{name}.zip");
        let fin = resolve_url(&u)?;
        let seg = fin.rsplit('/').nth(1).unwrap_or_default();
        version = seg.split_once('_').map(|(_, v)| v.to_string()).unwrap_or_else(|| seg.to_string());
        let sha = parse_sha256_sidecar(&get_text(&format!("{fin}.sha256"))?);
        artifacts.push(Artifact { name: name.into(), url: fin, sha256: sha });
    }
    Ok(Release { version, artifacts, source: "martin-riedl".into() })
}

/// BtbN GitHub builds: prefer the newest `ffmpeg-nX.Y-latest-win64-gpl-X.Y.zip` release branch.
fn resolve_btbn() -> Result<Release> {
    let j: serde_json::Value =
        serde_json::from_str(&get_text("https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/tags/latest")?)?;
    let assets = j["assets"].as_array().ok_or_else(|| anyhow!("no assets"))?;
    let mut best: Option<((u32, u32), String, String)> = None;
    let mut sums_url = None;
    for a in assets {
        let name = a["name"].as_str().unwrap_or_default();
        let url = a["browser_download_url"].as_str().unwrap_or_default().to_string();
        if name == "checksums.sha256" {
            sums_url = Some(url.clone());
        }
        if let Some(rest) = name.strip_prefix("ffmpeg-n") {
            if name.contains("-latest-win64-gpl-") && name.ends_with(".zip") && !name.contains("shared") {
                let ver = rest.split('-').next().unwrap_or_default().to_string();
                let pv = crate::tools::parse_version(&ver);
                if best.as_ref().map(|b| pv > b.0).unwrap_or(true) {
                    best = Some((pv, ver, url));
                }
            }
        }
    }
    let (_, ver, url) = best.ok_or_else(|| anyhow!("no win64 gpl release build found"))?;
    let fname = url.rsplit('/').next().unwrap_or_default().to_string();
    let sha = sums_url
        .and_then(|u| get_text(&u).ok())
        .and_then(|s| s.lines().find(|l| l.ends_with(&fname)).and_then(parse_sha256_sidecar));
    // One archive contains both binaries.
    Ok(Release {
        version: ver,
        artifacts: vec![Artifact { name: "ffmpeg+ffprobe".into(), url, sha256: sha }],
        source: "btbn".into(),
    })
}

fn find_file(root: &Path, file: &str) -> Option<PathBuf> {
    let rd = std::fs::read_dir(root).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, file) {
                return Some(f);
            }
        } else if p.file_name().and_then(|n| n.to_str()) == Some(file) {
            return Some(p);
        }
    }
    None
}

fn now_rfc3339() -> String {
    // Avoid a chrono dependency: seconds since epoch is unambiguous and sortable.
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("unix:{s}")
}

/// Download + extract a release into `<tools>/ffmpeg/<version>` WITHOUT activating it.
/// Returns the directory holding the binaries.
pub fn stage_ffmpeg(rel: &Release) -> Result<PathBuf> {
    let base = tools_dir().join("ffmpeg");
    let dl = base.join(".download");
    let dir = base.join(sanitize(&rel.version));
    let work = base.join(format!(".staging-{}", sanitize(&rel.version)));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    for a in &rel.artifacts {
        let fname = a.url.rsplit('/').next().unwrap_or("archive.zip");
        let fname = if fname.contains('.') { fname.to_string() } else { format!("{fname}.zip") };
        let arch = dl.join(format!("{}-{}", sanitize(&a.name), fname));
        let sha = download(&a.url, &arch, a.sha256.as_deref())?;
        if a.sha256.is_none() {
            tracing::warn!(url = a.url, sha, "upstream publishes no sha256; recording computed hash");
        }
        extract(&arch, &work.join(sanitize(&a.name)))?;
        let _ = std::fs::remove_file(&arch);
    }
    std::fs::create_dir_all(&dir)?;
    for b in ["ffmpeg", "ffprobe"] {
        let n = exe_name(b);
        let src = find_file(&work, &n).ok_or_else(|| anyhow!("{n} not found in downloaded archive"))?;
        let dst = dir.join(&n);
        std::fs::copy(&src, &dst)?;
        make_executable(&dst)?;
    }
    let _ = std::fs::remove_dir_all(&work);
    // sanity: it runs
    let out = std::process::Command::new(dir.join(exe_name("ffmpeg"))).arg("-version").output()?;
    if !out.status.success() {
        bail!("staged ffmpeg does not run");
    }
    Ok(dir)
}

pub fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// Mark a staged version as current (keeping the old current as `previous`).
pub fn promote_ffmpeg(version: &str) -> Result<()> {
    let base = tools_dir().join("ffmpeg");
    let v = sanitize(version);
    if !base.join(&v).join(exe_name("ffmpeg")).exists() {
        bail!("version {version} is not staged");
    }
    if let Ok(cur) = std::fs::read_to_string(base.join("current")) {
        if cur.trim() != v {
            std::fs::write(base.join("previous"), cur.trim())?;
        }
    }
    std::fs::write(base.join("current"), &v)?;
    Ok(())
}

/// Switch back to the previously active version.
pub fn rollback_ffmpeg() -> Result<String> {
    let base = tools_dir().join("ffmpeg");
    let prev = std::fs::read_to_string(base.join("previous")).context("no previous ffmpeg version to roll back to")?;
    let cur = std::fs::read_to_string(base.join("current")).unwrap_or_default();
    std::fs::write(base.join("current"), prev.trim())?;
    if !cur.trim().is_empty() {
        std::fs::write(base.join("previous"), cur.trim())?;
    }
    Ok(prev.trim().to_string())
}

pub fn current_ffmpeg_version() -> Option<String> {
    std::fs::read_to_string(tools_dir().join("ffmpeg").join("current")).ok().map(|s| s.trim().to_string())
}

/// Record a release in the lockfile.
pub fn record(lock: &mut Lockfile, rel: &Release, selftest: Option<&str>) {
    for a in &rel.artifacts {
        lock.upsert(LockEntry {
            name: a.name.clone(),
            version: rel.version.clone(),
            platform: platform_id(),
            url: a.url.clone(),
            sha256: a.sha256.clone().unwrap_or_default(),
            installed_at: Some(now_rfc3339()),
            selftest: selftest.map(str::to_string),
            license: Some("GPL-2.0-or-later (static build)".into()),
        });
    }
}

/// Release pinned in the lockfile for this platform, if complete.
pub fn pinned_ffmpeg(lock: &Lockfile) -> Option<Release> {
    let plat = platform_id();
    let ents: Vec<&LockEntry> = lock
        .tools
        .iter()
        .filter(|t| t.platform == plat && (t.name == "ffmpeg" || t.name == "ffprobe" || t.name == "ffmpeg+ffprobe"))
        .collect();
    let combined = ents.iter().any(|e| e.name == "ffmpeg+ffprobe");
    let complete = combined || (ents.iter().any(|e| e.name == "ffmpeg") && ents.iter().any(|e| e.name == "ffprobe"));
    if !complete || ents.iter().any(|e| e.sha256.is_empty()) {
        return None;
    }
    Some(Release {
        version: ents[0].version.clone(),
        artifacts: ents
            .iter()
            .map(|e| Artifact { name: e.name.clone(), url: e.url.clone(), sha256: Some(e.sha256.clone()) })
            .collect(),
        source: "lockfile".into(),
    })
}

/// Download a single file (model etc.) to `<tools>/<sub>/<file>`, verifying sha when known.
pub fn install_file(sub: &str, file: &str, url: &str, sha256: Option<&str>) -> Result<(PathBuf, String)> {
    let dest = tools_dir().join(sub).join(file);
    if dest.exists() {
        if let Some(e) = sha256 {
            if crate::download::sha256_file(&dest)?.eq_ignore_ascii_case(e) {
                return Ok((dest, e.to_string()));
            }
        }
    }
    let tmp = dest.with_extension("part");
    let got = download(url, &tmp, sha256)?;
    std::fs::rename(&tmp, &dest)?;
    Ok((dest, got))
}
