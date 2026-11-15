//! Brand presets (spec §8): stored in the app library, one JSON file each, with assets referenced
//! like media. A project keeps the preset's ID and a snapshot so it stays portable.

use std::io::{Read, Write};

use camino::{Utf8Path, Utf8PathBuf};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::media::{fingerprint, Fingerprint};
use crate::{ids, paths, timeutil, Error, Result};

/// A referenced brand file (logo, font, bumper). Never copied, except into a `.cutbrand` export.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AssetRef {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
}

impl AssetRef {
    pub fn new(path: &Utf8Path) -> Self {
        AssetRef { path: path.to_string(), fingerprint: fingerprint(path).ok() }
    }
    pub fn resolved(&self) -> Utf8PathBuf {
        paths::expand_tilde(&self.path)
    }
    pub fn exists(&self) -> bool {
        self.resolved().is_file()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Logos {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<AssetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizontal: Option<AssetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<AssetRef>,
    /// For light backgrounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<AssetRef>,
    /// For dark backgrounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark: Option<AssetRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Palette {
    pub primary: String,
    pub secondary: String,
    pub accent: String,
    pub background: String,
    pub text: String,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            primary: "#F26B21".into(),
            secondary: "#1F2A44".into(),
            accent: "#2D9CDB".into(),
            background: "#FFFFFF".into(),
            text: "#111827".into(),
        }
    }
}

impl Palette {
    pub fn roles(&self) -> [(&'static str, &str); 5] {
        [
            ("primary", &self.primary),
            ("secondary", &self.secondary),
            ("accent", &self.accent),
            ("background", &self.background),
            ("text", &self.text),
        ]
    }
}

/// A font by installed family name or by file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FontRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<AssetRef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Fonts {
    #[serde(default)]
    pub heading: FontRef,
    #[serde(default)]
    pub body: FontRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Watermark {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<AssetRef>,
    /// `top_left`, `top_right`, `bottom_left` or `bottom_right`.
    pub position: String,
    pub opacity: f64,
}

impl Default for Watermark {
    fn default() -> Self {
        Watermark { asset: None, position: "bottom_right".into(), opacity: 0.6 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Voice {
    /// ElevenLabs voice ID.
    #[serde(default)]
    pub voice_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// ElevenLabs voice settings (stability, similarity_boost, style, speed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, Value>>")]
    pub settings: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BrandPreset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub logos: Logos,
    #[serde(default)]
    pub colors: Palette,
    #[serde(default)]
    pub fonts: Fonts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<AssetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outro: Option<AssetRef>,
    #[serde(default)]
    pub watermark: Watermark,
    #[serde(default)]
    pub lower_third_style: String,
    #[serde(default)]
    pub caption_style: String,
    #[serde(default)]
    pub voice: Voice,
    #[serde(default)]
    pub music_mood: String,
    /// Do's and don'ts in free text, given to Claude verbatim.
    #[serde(default)]
    pub rules: String,
    #[serde(default)]
    pub modified: String,
}

impl BrandPreset {
    pub fn new(name: &str) -> Self {
        BrandPreset {
            id: ids::new_id("brd"),
            name: name.to_string(),
            description: String::new(),
            logos: Logos::default(),
            colors: Palette::default(),
            fonts: Fonts::default(),
            intro: None,
            outro: None,
            watermark: Watermark::default(),
            lower_third_style: String::new(),
            caption_style: String::new(),
            voice: Voice::default(),
            music_mood: String::new(),
            rules: String::new(),
            modified: timeutil::now_rfc3339(),
        }
    }

    /// Every referenced file with a label, for validation and export.
    pub fn assets(&self) -> Vec<(&'static str, &AssetRef)> {
        let mut v = Vec::new();
        let l = &self.logos;
        for (label, a) in [
            ("Primary logo", &l.primary),
            ("Horizontal logo", &l.horizontal),
            ("Icon", &l.icon),
            ("Logo for light backgrounds", &l.light),
            ("Logo for dark backgrounds", &l.dark),
            ("Intro bumper", &self.intro),
            ("Outro bumper", &self.outro),
            ("Watermark", &self.watermark.asset),
            ("Heading font", &self.fonts.heading.file),
            ("Body font", &self.fonts.body.file),
        ] {
            if let Some(a) = a {
                v.push((label, a));
            }
        }
        v
    }

    fn assets_mut(&mut self) -> Vec<(&'static str, &mut AssetRef)> {
        let mut v = Vec::new();
        let l = &mut self.logos;
        for (label, a) in [
            ("primary", &mut l.primary),
            ("horizontal", &mut l.horizontal),
            ("icon", &mut l.icon),
            ("light", &mut l.light),
            ("dark", &mut l.dark),
            ("intro", &mut self.intro),
            ("outro", &mut self.outro),
            ("watermark", &mut self.watermark.asset),
            ("font-heading", &mut self.fonts.heading.file),
            ("font-body", &mut self.fonts.body.file),
        ] {
            if let Some(a) = a {
                v.push((label, a));
            }
        }
        v
    }
}

/// The font families cutline bundles. It draws text only with these.
pub const CUTLINE_FONTS: [&str; 6] = ["Anton", "Bebas Neue", "Inter", "Poppins", "Poppins Bold", "Poppins ExtraBold"];

const CUTLINE_CORNERS: [&str; 4] = ["top_left", "top_right", "bottom_left", "bottom_right"];
const CUTLINE_CAPTION_STYLES: [&str; 5] = ["bold_pop", "karaoke", "word_by_word", "clean_sentence", "boxed"];

/// The preset in the shape cutline reads, with what could not be carried over.
#[derive(Debug, Clone, PartialEq)]
pub struct CutlineBrand {
    /// The brand file for cutline (`brand/default.json` in a run folder).
    pub file: Value,
    /// False when the brand has no watermark, so the edit must say `watermark: false`.
    pub watermark: bool,
    /// Brand settings cutline can't honour, in words for Claude and the user.
    pub notes: Vec<String>,
}

impl BrandPreset {
    /// Translates the preset into cutline's brand file. The two differ: cutline wants font
    /// names from its bundled set, one `logo`, and the watermark corner at the top level.
    /// Paths are absolute, so the file works from any folder.
    pub fn for_cutline(&self) -> CutlineBrand {
        let mut notes = Vec::new();
        let mut file = serde_json::Map::new();
        file.insert("name".into(), Value::String(self.name.clone()));

        let bundled = |f: &FontRef, role: &str, notes: &mut Vec<String>| -> Option<String> {
            let family = f.family.as_deref().map(str::trim).filter(|s| !s.is_empty());
            match family {
                Some(name) => match CUTLINE_FONTS.iter().find(|b| b.eq_ignore_ascii_case(name)) {
                    Some(b) => Some(b.to_string()),
                    None => {
                        notes.push(format!(
                            "The {role} font \"{name}\" isn't one cutline can draw (it has {}); Inter is used instead.",
                            CUTLINE_FONTS[..4].join(", ")
                        ));
                        Some("Inter".into())
                    }
                },
                None if f.file.is_some() => {
                    notes.push(format!(
                        "The {role} font is a font file, which cutline can't load (it has {}); Inter is used instead.",
                        CUTLINE_FONTS[..4].join(", ")
                    ));
                    Some("Inter".into())
                }
                None => None,
            }
        };
        let mut fonts = serde_json::Map::new();
        if let Some(t) = bundled(&self.fonts.heading, "heading", &mut notes) {
            fonts.insert("title".into(), Value::String(t));
        }
        if let Some(b) = bundled(&self.fonts.body, "body", &mut notes) {
            fonts.insert("body".into(), Value::String(b));
        }
        if !fonts.is_empty() {
            file.insert("fonts".into(), Value::Object(fonts));
        }

        let c = &self.colors;
        // Text is outlined in whichever of black and white stands out from it.
        let stroke = match parse_hex(&c.text).map(luminance) {
            Some(l) if l < 0.4 => "#FFFFFF",
            _ => "#000000",
        };
        file.insert(
            "colors".into(),
            serde_json::json!({
                "primary": c.primary, "accent": c.accent, "text": c.text,
                "stroke": stroke, "background": c.background,
            }),
        );

        let usable = |a: &Option<AssetRef>| a.as_ref().filter(|a| a.exists()).map(|a| a.resolved().to_string());
        let l = &self.logos;
        let watermark_asset = usable(&self.watermark.asset);
        let logo = [&l.primary, &l.horizontal, &l.icon, &l.dark, &l.light]
            .into_iter()
            .find_map(usable)
            .or_else(|| watermark_asset.clone());
        if let (Some(w), Some(lg)) = (&watermark_asset, &logo) {
            if w != lg {
                notes.push(
                    "cutline draws the watermark from the brand logo, so the separate watermark image isn't used."
                        .into(),
                );
            }
        }
        let watermark = watermark_asset.is_some() && logo.is_some();
        if let Some(lg) = logo {
            file.insert("logo".into(), Value::String(lg));
        }
        if watermark {
            let corner = self.watermark.position.trim().to_ascii_lowercase().replace([' ', '-'], "_");
            let corner = if CUTLINE_CORNERS.contains(&corner.as_str()) { corner } else { "bottom_right".into() };
            file.insert("watermark_position".into(), Value::String(corner));
            file.insert("watermark".into(), serde_json::json!({"opacity": self.watermark.opacity.clamp(0.0, 1.0)}));
        }
        for (key, asset) in [("intro", &self.intro), ("outro", &self.outro)] {
            if let Some(p) = usable(asset) {
                file.insert(key.into(), Value::String(p));
            }
        }
        // The caption style is free text; it carries over only when it is a cutline style name.
        let style = self.caption_style.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        if CUTLINE_CAPTION_STYLES.contains(&style.as_str()) {
            file.insert("caption_style".into(), Value::String(style));
        }
        CutlineBrand { file: Value::Object(file), watermark, notes }
    }
}

/// The preset a project uses: its library ID and a full copy taken when it was chosen.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BrandRef {
    pub preset_id: String,
    pub snapshot: BrandPreset,
}

impl BrandRef {
    pub fn of(preset: &BrandPreset) -> Self {
        BrandRef { preset_id: preset.id.clone(), snapshot: preset.clone() }
    }
}

pub fn brands_dir(library: &Utf8Path) -> Utf8PathBuf {
    library.join("brands")
}

fn preset_file(library: &Utf8Path, id: &str) -> Utf8PathBuf {
    brands_dir(library).join(format!("{}.json", ids::slug(id).replace('-', "_")))
}

/// All presets in the library, by name.
pub fn list(library: &Utf8Path) -> Vec<BrandPreset> {
    let mut v: Vec<BrandPreset> = std::fs::read_dir(brands_dir(library))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| paths::utf8(e.path()).ok())
                .filter(|p| p.extension() == Some("json"))
                .filter_map(|p| paths::read_json::<BrandPreset>(&p).ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|p| p.name.to_lowercase());
    v
}

pub fn get(library: &Utf8Path, id: &str) -> Option<BrandPreset> {
    paths::read_json::<BrandPreset>(&preset_file(library, id))
        .ok()
        .filter(|p| p.id == id)
        .or_else(|| list(library).into_iter().find(|p| p.id == id))
}

/// Saves a preset to the library, stamping `modified`.
pub fn save(library: &Utf8Path, preset: &mut BrandPreset) -> Result<()> {
    if preset.name.trim().is_empty() {
        return Err(Error::other("a brand preset needs a name"));
    }
    if preset.id.trim().is_empty() {
        preset.id = ids::new_id("brd");
    }
    preset.modified = timeutil::now_rfc3339();
    paths::write_json_atomic(&preset_file(library, &preset.id), preset)
}

pub fn delete(library: &Utf8Path, id: &str) -> Result<()> {
    let f = preset_file(library, id);
    if f.exists() {
        std::fs::remove_file(&f)?;
    }
    let assets = brands_dir(library).join(format!("{}.assets", ids::slug(id).replace('-', "_")));
    if assets.is_dir() {
        std::fs::remove_dir_all(assets)?;
    }
    Ok(())
}

/// True when the library copy differs from, and is newer than, the project's snapshot — the
/// "Update to latest" banner.
pub fn library_is_newer(library: &Utf8Path, brand: &BrandRef) -> bool {
    match get(library, &brand.preset_id) {
        Some(lib) => lib != brand.snapshot && lib.modified > brand.snapshot.modified,
        None => false,
    }
}

/// A first preset so a new installation has something to pick (onboarding).
pub fn ensure_default(library: &Utf8Path) -> Result<BrandPreset> {
    if let Some(first) = list(library).into_iter().next() {
        return Ok(first);
    }
    let mut p = BrandPreset::new("Default");
    p.description = "Neutral starting point. Replace the colors, add logos, and write your brand rules.".into();
    p.caption_style = "Sentence captions, bottom third, high contrast".into();
    p.music_mood = "Light, optimistic, unobtrusive".into();
    p.rules = "Do: keep on-screen text short and readable.\nDon't: stretch, recolor or crowd the logo.".into();
    save(library, &mut p)?;
    Ok(p)
}

// MARK: Colors

/// Parses `#RGB`, `#RRGGBB` or `#RRGGBBAA` (the alpha is ignored) to 0–255 components.
pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().strip_prefix('#')?;
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
    match h.len() {
        3 => Some((v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17)),
        6 | 8 => Some((v(0, 2)?, v(2, 2)?, v(4, 2)?)),
        _ => None,
    }
}

/// WCAG relative luminance.
pub fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    let c = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * c(r) + 0.7152 * c(g) + 0.0722 * c(b)
}

/// WCAG contrast ratio, 1–21.
pub fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Average color of a logo's visible pixels (PNG or JPEG), for the contrast check.
pub fn logo_average_color(path: &Utf8Path) -> Option<(u8, u8, u8)> {
    let img = image::open(path).ok()?;
    let img = if img.width().max(img.height()) > 96 { img.thumbnail(96, 96) } else { img }.to_rgba8();
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for p in img.pixels() {
        if p[3] >= 128 {
            r += p[0] as u64;
            g += p[1] as u64;
            b += p[2] as u64;
            n += 1;
        }
    }
    (n > 0).then(|| ((r / n) as u8, (g / n) as u8, (b / n) as u8))
}

// MARK: Import and export (`.cutbrand`: brand.json plus its assets, zipped)

/// Writes a `.cutbrand` file. Assets are stored under `assets/` and the JSON refers to them by
/// those names.
pub fn export_cutbrand(preset: &BrandPreset, dest: &Utf8Path) -> Result<()> {
    let mut portable = preset.clone();
    let mut files: Vec<(String, Utf8PathBuf)> = Vec::new();
    for (label, a) in portable.assets_mut() {
        let src = a.resolved();
        if !src.is_file() {
            return Err(Error::NotFound(format!("the brand asset {} is missing; relink it before exporting", a.path)));
        }
        let name = format!("assets/{label}-{}", src.file_name().unwrap_or("file"));
        a.path = name.clone();
        files.push((name, src));
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("brand.json", opts).map_err(|e| Error::other(e.to_string()))?;
        zip.write_all(paths::to_pretty_json(&portable)?.as_bytes())?;
        for (name, src) in &files {
            zip.start_file(name.as_str(), opts).map_err(|e| Error::other(e.to_string()))?;
            zip.write_all(&std::fs::read(src)?)?;
        }
        zip.finish().map_err(|e| Error::other(e.to_string()))?;
    }
    paths::write_atomic(dest, buf.get_ref())
}

/// Imports a `.cutbrand` file into the library. Its assets are unpacked next to the preset
/// (`brands/<id>.assets/`). An existing preset with the same ID is replaced when `replace` is
/// true; otherwise the import gets a new ID.
pub fn import_cutbrand(library: &Utf8Path, file: &Utf8Path, replace: bool) -> Result<BrandPreset> {
    let f = std::fs::File::open(file).map_err(|e| Error::Io(std::io::Error::new(e.kind(), format!("{file}: {e}"))))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| Error::other(format!("{file} isn't a brand file: {e}")))?;
    let mut json = String::new();
    zip.by_name("brand.json")
        .map_err(|_| Error::other(format!("{file} has no brand.json")))?
        .read_to_string(&mut json)?;
    let mut preset: BrandPreset =
        serde_json::from_str(&json).map_err(|e| Error::other(format!("{file}: brand.json is invalid: {e}")))?;
    if !replace && get(library, &preset.id).is_some() {
        preset.id = ids::new_id("brd");
    }
    let assets_dir = brands_dir(library).join(format!("{}.assets", ids::slug(&preset.id).replace('-', "_")));
    for (_, a) in preset.assets_mut() {
        // Only plain names under assets/ are accepted (no traversal out of the library).
        let name = a
            .path
            .strip_prefix("assets/")
            .filter(|n| !n.is_empty() && !n.contains(['/', '\\']) && *n != "..")
            .map(str::to_string);
        let Some(name) = name else { continue };
        let mut entry = zip
            .by_name(&format!("assets/{name}"))
            .map_err(|_| Error::other(format!("{file} is missing assets/{name}")))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        let dest = assets_dir.join(&name);
        paths::write_atomic(&dest, &bytes)?;
        *a = AssetRef::new(&dest);
    }
    save(library, &mut preset)?;
    Ok(preset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib() -> (tempfile::TempDir, Utf8PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = Utf8Path::from_path(d.path()).unwrap().to_owned();
        (d, p)
    }

    fn png(path: &Utf8Path, color: [u8; 4]) {
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba(color));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        img.save(path).unwrap();
    }

    #[test]
    fn library_crud_and_snapshot() {
        let (_d, lib) = lib();
        assert!(list(&lib).is_empty());
        let mut acme = BrandPreset::new("Acme");
        acme.rules = "Do: lead with the product.".into();
        save(&lib, &mut acme).unwrap();
        let mut beta = BrandPreset::new("beta brand");
        save(&lib, &mut beta).unwrap();
        assert_eq!(list(&lib).iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["Acme", "beta brand"]);
        let snap = BrandRef::of(&get(&lib, &acme.id).unwrap());
        assert!(!library_is_newer(&lib, &snap));
        // the library copy changes later
        let mut edited = get(&lib, &acme.id).unwrap();
        edited.colors.primary = "#000000".into();
        edited.modified = "2000-01-01T00:00:00Z".into();
        paths::write_json_atomic(&preset_file(&lib, &edited.id), &edited).unwrap();
        assert!(!library_is_newer(&lib, &snap), "an older library copy doesn't offer an update");
        edited.modified = "2999-01-01T00:00:00Z".into();
        paths::write_json_atomic(&preset_file(&lib, &edited.id), &edited).unwrap();
        assert!(library_is_newer(&lib, &snap));
        delete(&lib, &acme.id).unwrap();
        assert!(get(&lib, &acme.id).is_none());
        assert!(!library_is_newer(&lib, &snap), "a deleted preset leaves the snapshot usable");
    }

    #[test]
    fn default_preset_is_created_once() {
        let (_d, lib) = lib();
        let a = ensure_default(&lib).unwrap();
        let b = ensure_default(&lib).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(list(&lib).len(), 1);
    }

    #[test]
    fn translates_the_preset_for_cutline() {
        let (_t, dir) = lib();
        let logo = dir.join("logo.png");
        png(&logo, [255, 0, 0, 255]);
        let mut b = BrandPreset::new("Acme");
        b.logos.primary = Some(AssetRef::new(&logo));
        b.watermark.asset = Some(AssetRef::new(&logo));
        b.watermark.position = "bottom_right".into();
        b.fonts.heading.family = Some("poppins".into());
        b.fonts.body.family = Some("Helvetica Neue".into());
        b.caption_style = "Sentence captions, bottom third".into();
        let c = b.for_cutline();
        // It must load as a cutline brand, which is the point of the translation.
        let parsed: cutline_core::brand::Brand = serde_json::from_value(c.file.clone()).unwrap();
        assert_eq!(parsed.fonts.title, "Poppins");
        assert_eq!(parsed.fonts.body, "Inter");
        assert_eq!(parsed.logo.as_deref(), Some(logo.as_str()));
        assert_eq!(parsed.watermark_position.as_deref(), Some("bottom_right"));
        assert_eq!(parsed.watermark.unwrap().opacity, 0.6);
        assert_eq!(parsed.colors.stroke, "#FFFFFF", "dark text gets a light outline");
        assert_eq!(parsed.caption_style, None, "free text is not a cutline style name");
        assert!(c.watermark);
        assert_eq!(c.notes.len(), 1);
        assert!(c.notes[0].contains("Helvetica Neue"));

        // A new preset has empty fonts and no logo: nothing invalid is written.
        let plain = BrandPreset::new("Plain").for_cutline();
        let parsed: cutline_core::brand::Brand = serde_json::from_value(plain.file.clone()).unwrap();
        assert_eq!(parsed.fonts.title, "Anton", "cutline's own defaults apply");
        assert!(parsed.logo.is_none() && !plain.watermark && plain.notes.is_empty());
        assert!(plain.file.get("fonts").is_none());
    }

    #[test]
    fn colors_and_contrast() {
        assert_eq!(parse_hex("#fff"), Some((255, 255, 255)));
        assert_eq!(parse_hex("#F26B21"), Some((242, 107, 33)));
        assert_eq!(parse_hex("#11223344"), Some((17, 34, 51)));
        assert_eq!(parse_hex("F26B21"), None);
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#GGGGGG"), None);
        assert!((contrast((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 0.01);
        assert!(contrast((250, 250, 250), (255, 255, 255)) < 1.1);
    }

    #[test]
    fn cutbrand_round_trip_carries_assets() {
        let (_d, lib) = lib();
        let (_d2, other) = self::lib();
        let logo = lib.join("src/logo.png");
        png(&logo, [242, 107, 33, 255]);
        assert_eq!(logo_average_color(&logo), Some((242, 107, 33)));
        let mut p = BrandPreset::new("Acme");
        p.logos.primary = Some(AssetRef::new(&logo));
        p.watermark.asset = Some(AssetRef::new(&logo));
        p.voice.voice_id = "voice123".into();
        save(&lib, &mut p).unwrap();
        let file = lib.join("acme.cutbrand");
        export_cutbrand(&p, &file).unwrap();
        // on another machine the source logo doesn't exist
        let imported = import_cutbrand(&other, &file, false).unwrap();
        assert_eq!(imported.id, p.id);
        assert_eq!(imported.voice.voice_id, "voice123");
        let a = imported.logos.primary.as_ref().unwrap();
        assert!(a.exists() && a.path.starts_with(other.as_str()), "{}", a.path);
        assert_eq!(
            a.fingerprint,
            p.logos.primary.as_ref().unwrap().fingerprint.clone().map(|mut f| {
                f.mtime = a.fingerprint.as_ref().unwrap().mtime.clone();
                f
            })
        );
        // importing again without replace makes a second preset
        let again = import_cutbrand(&other, &file, false).unwrap();
        assert_ne!(again.id, p.id);
        assert_eq!(list(&other).len(), 2);
        // a missing asset blocks export with a clear message
        std::fs::remove_file(&logo).unwrap();
        assert!(export_cutbrand(&p, &file).unwrap_err().to_string().contains("missing"));
    }
}
