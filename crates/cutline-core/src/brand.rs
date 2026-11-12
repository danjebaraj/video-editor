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

    /// Resolved SFX alias files that exist.
    pub fn sfx_files(&self) -> BTreeMap<String, PathBuf> {
        self.sfx
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|p| (k.clone(), self.dir.join(p))))
            .filter(|(_, p)| p.exists())
            .collect()
    }
}
