//! Brand file: fonts, colors, logo, caption defaults, SFX aliases, LUT, watermark.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BrandFonts {
    pub title: String,
    pub body: String,
    pub caption: String,
}
impl Default for BrandFonts {
    fn default() -> Self {
        Self { title: "Anton".into(), body: "Inter".into(), caption: "Poppins ExtraBold".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BrandColors {
    pub primary: String,
    pub accent: String,
    pub text: String,
    pub stroke: String,
    pub background: String,
}
impl Default for BrandColors {
    fn default() -> Self {
        Self {
            primary: "#FFFFFF".into(),
            accent: "#FFC400".into(),
            text: "#FFFFFF".into(),
            stroke: "#000000".into(),
            background: "#101014".into(),
        }
    }
}

/// Watermark look. Times are seconds on the edit (bumpers excluded).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Watermark {
    pub opacity: f64,
    /// Logo width as a fraction of the frame width.
    pub scale: f64,
    pub from: Option<f64>,
    pub to: Option<f64>,
}
impl Default for Watermark {
    fn default() -> Self {
        Self { opacity: 0.7, scale: 0.08, from: None, to: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Brand {
    pub name: String,
    pub fonts: BrandFonts,
    pub colors: BrandColors,
    pub logo: Option<String>,
    pub caption_style: Option<String>,
    pub lower_third: Option<String>,
    pub intro: Option<String>,
    pub outro: Option<String>,
    /// alias → path (relative to the brand file) or null.
    pub sfx: BTreeMap<String, Option<String>>,
    pub lut: Option<String>,
    pub watermark_position: Option<String>,
    /// How the logo is drawn as a watermark (when `logo` is set).
    pub watermark: Option<Watermark>,
    /// Seconds of fade between a bumper (`intro`/`outro`) and the edit (default 0.4; 0 = cut).
    pub bumper_fade: Option<f64>,
    /// Caption corrections applied after transcription (names, product terms).
    pub dictionary: BTreeMap<String, String>,
    /// Directory the brand file lives in (for relative paths).
    #[serde(skip)]
    pub dir: PathBuf,
}

impl Brand {
    /// Load `project.brand` (relative to the project dir), `default`, or the bundled default.
    pub fn load(spec: Option<&str>, project_dir: &Path, assets_dir: &Path) -> anyhow::Result<Brand> {
        let path = match spec {
            None | Some("default") => {
                let p = project_dir.join("brand").join("default.json");
                if spec.is_some() || !p.exists() {
                    assets_dir.join("brand").join("default.json")
                } else {
                    p
                }
            }
            Some(s) => project_dir.join(s),
        };
        let text = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("brand file {}: {e}", path.display()))?;
        let mut b: Brand = json5::from_str(&text).map_err(|e| anyhow::anyhow!("brand file {}: {e}", path.display()))?;
        b.dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok(b)
    }

    /// A brand file path (`logo`, `intro`, `outro`), relative to the brand file.
    pub fn file(&self, rel: &Option<String>) -> Option<PathBuf> {
        rel.as_ref().filter(|s| !s.is_empty()).map(|s| self.dir.join(s))
    }

    /// Resolved SFX alias files that exist.
    pub fn sfx_files(&self) -> BTreeMap<String, PathBuf> {
        self.sfx
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|p| (k.clone(), self.dir.join(p))))
            .filter(|(_, p)| p.exists())
            .collect()
    }
}
