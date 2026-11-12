//! Tool discovery (tools dir → PATH fallback), version parsing and capability probing.

use crate::paths::{exe_name, tools_dir};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolSource {
    Env,
    ToolsDir,
    Path,
}

/// A resolved ffmpeg + ffprobe pair.
#[derive(Debug, Clone, Serialize)]
pub struct Toolchain {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub version: String,
    pub source: ToolSource,
}

/// Parsed `(major, minor)`; snapshot builds (`N-12345-g...`, `git-...`) count as newest.
pub fn parse_version(v: &str) -> (u32, u32) {
    let v = v.trim();
    if v.starts_with('N') || v.starts_with("git") {
        return (99, 0);
    }
    let v = v.trim_start_matches('n');
    let mut it = v.split(|c: char| !c.is_ascii_digit());
    let maj = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let min = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (maj, min)
}

impl Toolchain {
    /// True when ffmpeg supports `-/filter_complex <file>` (ffmpeg ≥ 7.1).
    pub fn supports_slash_filter_option(&self) -> bool {
        parse_version(&self.version) >= (7, 1)
    }

    /// Arguments that load a filter_complex script from a file.
    pub fn filter_complex_file_args(&self, path: &Path) -> Vec<String> {
        let p = path.to_string_lossy().to_string();
        if self.supports_slash_filter_option() {
            vec!["-/filter_complex".into(), p]
        } else {
            vec!["-filter_complex_script".into(), p]
        }
    }

    pub fn ffmpeg_cmd(&self) -> Command {
        let mut c = Command::new(&self.ffmpeg);
        c.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]);
        c
    }

    pub fn ffprobe_cmd(&self) -> Command {
        let mut c = Command::new(&self.ffprobe);
        c.args(["-hide_banner", "-loglevel", "error"]);
        c
    }
}

fn read_version(ffmpeg: &Path) -> Result<String> {
    let out = Command::new(ffmpeg)
        .arg("-version")
        .output()
        .with_context(|| format!("running {}", ffmpeg.display()))?;
    if !out.status.success() {
        bail!("{} -version failed", ffmpeg.display());
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let first = s.lines().next().unwrap_or_default();
    // "ffmpeg version 6.1.1-3ubuntu5 Copyright ..."
    Ok(first
        .split_whitespace()
        .nth(2)
        .unwrap_or("unknown")
        .to_string())
}

/// Directory where the active installed ffmpeg version lives, if any.
pub fn installed_ffmpeg_dir() -> Option<PathBuf> {
    let base = tools_dir().join("ffmpeg");
    let cur = std::fs::read_to_string(base.join("current")).ok()?;
    let d = base.join(cur.trim());
    d.join(exe_name("ffmpeg")).exists().then_some(d)
}

/// Discover ffmpeg/ffprobe: `CUTLINE_FFMPEG`/`CUTLINE_FFPROBE` → tools dir → PATH.
pub fn discover() -> Result<Toolchain> {
    if let (Some(f), Some(p)) = (
        std::env::var_os("CUTLINE_FFMPEG"),
        std::env::var_os("CUTLINE_FFPROBE"),
    ) {
        let ffmpeg = PathBuf::from(f);
        return Ok(Toolchain {
            version: read_version(&ffmpeg)?,
            ffmpeg,
            ffprobe: PathBuf::from(p),
            source: ToolSource::Env,
        });
    }
    if let Some(d) = installed_ffmpeg_dir() {
        let ffmpeg = d.join(exe_name("ffmpeg"));
        let ffprobe = d.join(exe_name("ffprobe"));
        if ffprobe.exists() {
            if let Ok(version) = read_version(&ffmpeg) {
                return Ok(Toolchain {
                    ffmpeg,
                    ffprobe,
                    version,
                    source: ToolSource::ToolsDir,
                });
            }
        }
    }
    let ffmpeg =
        which::which("ffmpeg").context("ffmpeg not found (run `cutline deps install ffmpeg`)")?;
    let ffprobe =
        which::which("ffprobe").context("ffprobe not found (run `cutline deps install ffmpeg`)")?;
    Ok(Toolchain {
        version: read_version(&ffmpeg)?,
        ffmpeg,
        ffprobe,
        source: ToolSource::Path,
    })
}

static TOOLCHAIN: OnceLock<Result<Toolchain, String>> = OnceLock::new();

/// Process-wide cached toolchain.
pub fn toolchain() -> Result<&'static Toolchain> {
    match TOOLCHAIN.get_or_init(|| discover().map_err(|e| format!("{e:#}"))) {
        Ok(t) => Ok(t),
        Err(e) => bail!("{e}"),
    }
}

/// Lists of available filters/encoders/decoders.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Capabilities {
    pub filters: BTreeSet<String>,
    pub encoders: BTreeSet<String>,
    pub decoders: BTreeSet<String>,
}

fn list(ffmpeg: &Path, flag: &str, name_col: usize) -> Result<BTreeSet<String>> {
    let out = Command::new(ffmpeg).args(["-hide_banner", flag]).output()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let mut set = BTreeSet::new();
    let mut started = false;
    for line in s.lines() {
        // filter list has no dashed separator: " TSC xfade   VV->V  desc"
        if flag == "-filters" {
            let t: Vec<&str> = line.split_whitespace().collect();
            if t.len() >= 3 && t[2].contains("->") {
                set.insert(t[1].to_string());
            }
            continue;
        }
        if !started {
            // header ends with a line of dashes / " ------"
            if line.trim_start().starts_with("---") || line.trim() == "------" {
                started = true;
            }
            continue;
        }
        if let Some(n) = line.split_whitespace().nth(name_col) {
            set.insert(n.to_string());
        }
    }
    Ok(set)
}

pub fn capabilities(tc: &Toolchain) -> Result<Capabilities> {
    Ok(Capabilities {
        filters: list(&tc.ffmpeg, "-filters", 1)?,
        encoders: list(&tc.ffmpeg, "-encoders", 1)?,
        decoders: list(&tc.ffmpeg, "-decoders", 1)?,
    })
}

pub const REQUIRED_FILTERS: &[&str] = &[
    "xfade",
    "acrossfade",
    "loudnorm",
    "ebur128",
    "afftdn",
    "deesser",
    "acompressor",
    "sidechaincompress",
    "subtitles",
    "lut3d",
    "scdet",
    "blackdetect",
    "freezedetect",
    "silencedetect",
    "rgbashift",
    "colortemperature",
    "amultiply",
    "overlay",
    "scale",
    "crop",
    "pad",
    "fps",
    "atempo",
    "alimiter",
];
pub const OPTIONAL_FILTERS: &[&str] = &["zscale", "vidstabdetect", "vidstabtransform", "arnndn"];
pub const REQUIRED_ENCODERS: &[&str] = &["libx264", "aac", "prores_ks", "png"];
pub const REQUIRED_DECODERS: &[&str] = &["libvpx-vp9", "prores", "hevc", "h264"];
pub const DRAFT_HW_ENCODERS: &[&str] = &["h264_videotoolbox", "h264_nvenc", "h264_qsv", "h264_amf"];

#[derive(Debug, Clone, Serialize)]
pub struct CapCheck {
    pub kind: &'static str,
    pub name: String,
    pub required: bool,
    pub present: bool,
}

pub fn check_capabilities(c: &Capabilities) -> Vec<CapCheck> {
    let mut v = vec![];
    let mut push = |kind, set: &BTreeSet<String>, names: &[&str], required| {
        for n in names {
            v.push(CapCheck {
                kind,
                name: n.to_string(),
                required,
                present: set.contains(*n),
            });
        }
    };
    push("filter", &c.filters, REQUIRED_FILTERS, true);
    push("filter", &c.filters, OPTIONAL_FILTERS, false);
    push("encoder", &c.encoders, REQUIRED_ENCODERS, true);
    push("encoder", &c.encoders, DRAFT_HW_ENCODERS, false);
    push("decoder", &c.decoders, REQUIRED_DECODERS, true);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("6.1.1-3ubuntu5"), (6, 1));
        assert_eq!(parse_version("9.0.2"), (9, 0));
        assert_eq!(parse_version("n7.1"), (7, 1));
        assert_eq!(parse_version("N-122320-g38e89fe502"), (99, 0));
        assert!(parse_version("7.1") >= (7, 1));
        assert!(parse_version("7.0.2") < (7, 1));
    }
}
