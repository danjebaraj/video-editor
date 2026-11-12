//! Bundled font database (family names as libass sees them) + measurement handles.

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct FontEntry {
    /// Names libass can match (legacy family name ID 1, typographic family ID 16, full name).
    pub names: Vec<String>,
    pub bold: bool,
    pub italic: bool,
    pub path: PathBuf,
    pub font: FontArc,
    /// hhea height / (OS/2 winAscent + winDescent): converts an ASS font size (libass sizes by
    /// the win metrics, like VSFilter) into an ab_glyph pixel scale.
    pub win_ratio: f64,
}

impl std::fmt::Debug for FontEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontEntry")
            .field("names", &self.names)
            .field("bold", &self.bold)
            .field("path", &self.path)
            .finish()
    }
}

#[derive(Debug, Clone, Default)]
pub struct FontDb {
    pub dir: PathBuf,
    pub fonts: Vec<FontEntry>,
}

impl FontDb {
    pub fn load(dir: &Path) -> Result<Self> {
        let mut fonts = vec![];
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .with_context(|| format!("fonts dir {}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("ttf") | Some("otf")))
            .collect();
        files.sort();
        for p in files {
            let data = std::fs::read(&p)?;
            let face = ttf_parser::Face::parse(&data, 0).with_context(|| format!("parsing {}", p.display()))?;
            let mut names = vec![];
            for n in face.names() {
                if matches!(n.name_id, 1 | 4 | 16) {
                    if let Some(s) = n.to_string() {
                        if !names.contains(&s) {
                            names.push(s);
                        }
                    }
                }
            }
            let hhea_h = (face.ascender() as f64 - face.descender() as f64).max(1.0);
            let win_h = face
                .tables()
                .os2
                .map(|o| o.windows_ascender() as f64 + o.windows_descender().unsigned_abs() as f64)
                .filter(|v| *v > 0.0)
                .unwrap_or(hhea_h);
            let win_ratio = hhea_h / win_h;
            let bold = face.is_bold() || face.weight().to_number() >= 700;
            let italic = face.is_italic();
            let font = FontArc::try_from_vec(data).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
            fonts.push(FontEntry { names, bold, italic, path: p, font, win_ratio });
        }
        Ok(Self { dir: dir.to_path_buf(), fonts })
    }

    /// Bundled default fonts dir.
    pub fn bundled() -> Result<Self> {
        Self::load(&cutline_deps::paths::assets_dir().join("fonts"))
    }

    /// Family names (deduplicated) for validation.
    pub fn families(&self) -> Vec<String> {
        let mut v: Vec<String> = self.fonts.iter().flat_map(|f| f.names.iter().cloned()).collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn has(&self, family: &str) -> bool {
        self.fonts.iter().any(|f| f.names.iter().any(|n| n.eq_ignore_ascii_case(family)))
    }

    /// Best match for (family, bold). Falls back to the first font.
    pub fn get(&self, family: &str, bold: bool) -> Option<&FontEntry> {
        let cands: Vec<&FontEntry> =
            self.fonts.iter().filter(|f| f.names.iter().any(|n| n.eq_ignore_ascii_case(family))).collect();
        cands.iter().find(|f| f.bold == bold).or(cands.first()).copied().or(self.fonts.first())
    }
}

/// Text measurement consistent with libass sizing: the ASS font size equals the font's
/// ascender − descender in pixels (which is what `ab_glyph::PxScale` height means).
pub struct Measure<'a> {
    pub font: &'a FontEntry,
    pub size: f64,
    /// Letter spacing (ASS `\fsp`) in px.
    pub spacing: f64,
}

impl Measure<'_> {
    fn scale(&self) -> PxScale {
        PxScale::from((self.size * self.font.win_ratio) as f32)
    }
    pub fn width(&self, text: &str) -> f64 {
        let sf = self.font.font.as_scaled(self.scale());
        let mut w = 0.0f64;
        let mut prev = None;
        let mut n = 0;
        for ch in text.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                w += sf.kern(p, id) as f64;
            }
            w += sf.h_advance(id) as f64;
            prev = Some(id);
            n += 1;
        }
        w + self.spacing * (n.max(1) - 1) as f64
    }
    /// Line height in px (libass: ascender − descender at this size).
    pub fn line_height(&self) -> f64 {
        // libass line height = the font size (win ascent + descent at that size)
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_fonts_load_and_measure() {
        let db = FontDb::bundled().unwrap();
        assert!(db.has("Anton"));
        assert!(db.has("Poppins ExtraBold"));
        let f = db.get("Anton", false).unwrap();
        let m = Measure { font: f, size: 100.0, spacing: 0.0 };
        let w1 = m.width("HELLO");
        let w2 = m.width("HELLO HELLO");
        assert!(w1 > 100.0 && w2 > 2.0 * w1);
        assert!((m.line_height() - 100.0).abs() < 1.0, "{}", m.line_height());
        assert!(f.win_ratio > 0.5 && f.win_ratio <= 1.5);
    }
}
