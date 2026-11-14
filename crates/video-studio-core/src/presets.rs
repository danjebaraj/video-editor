//! Output config presets and platform limits (spec §9.1). The data lives in `platforms.json`,
//! bundled with the core and replaceable by a newer copy in the app library.

use std::collections::{BTreeMap, HashSet};

use camino::Utf8Path;
use cutline_core::model::TargetName;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::project::{IntentKind, Project, RunConfig};
use crate::{ids, paths, Error, Result};

pub const BUNDLED_PLATFORMS: &str = include_str!("../data/platforms.json");

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Limits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_duration_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NamedSize {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputPreset {
    pub id: String,
    pub name: String,
    pub platform: String,
    /// cutline targets this preset can use; the first is the default.
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    /// Device sizes to pick from (App Store previews).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sizes: Vec<NamedSize>,
    /// `burn`, `sidecar` or `none`.
    #[serde(default = "default_captions")]
    pub captions: String,
    #[serde(default = "yes")]
    pub audio: bool,
    #[serde(default, rename = "loop")]
    pub looped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Container formats to export; more than one gives one file per format.
    #[serde(default = "default_formats")]
    pub formats: Vec<String>,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub notes: String,
    /// True for presets the user made in the app library.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub user: bool,
}

fn default_captions() -> String {
    "burn".into()
}
fn default_formats() -> Vec<String> {
    vec!["mp4".into()]
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntentDefaults {
    pub template: String,
    pub outputs: Vec<String>,
    pub captions: String,
    pub pacing: String,
    pub music_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Platforms {
    pub version: u32,
    #[serde(default)]
    pub updated: String,
    pub presets: Vec<OutputPreset>,
    #[serde(default)]
    pub intents: BTreeMap<String, IntentDefaults>,
}

impl Platforms {
    pub fn bundled() -> Self {
        serde_json::from_str(BUNDLED_PLATFORMS).expect("bundled platforms.json is valid")
    }

    /// The bundled data, replaced by `<library>/platforms.json` when that copy is newer, plus the
    /// user's own presets from `<library>/output-presets/*.json`.
    pub fn load(library: &Utf8Path) -> Self {
        let mut p = Self::bundled();
        if let Ok(over) = paths::read_json::<Platforms>(&library.join("platforms.json")) {
            if over.version >= p.version && over.updated >= p.updated {
                p = over;
            }
        }
        let dir = library.join("output-presets");
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut user: Vec<OutputPreset> = rd
                .flatten()
                .filter_map(|e| paths::utf8(e.path()).ok())
                .filter(|f| f.extension() == Some("json"))
                .filter_map(|f| paths::read_json::<OutputPreset>(&f).ok())
                .collect();
            user.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            for mut u in user {
                u.user = true;
                p.presets.retain(|x| x.id != u.id);
                p.presets.push(u);
            }
        }
        p
    }

    pub fn preset(&self, id: &str) -> Option<&OutputPreset> {
        self.presets.iter().find(|p| p.id == id)
    }

    pub fn intent_defaults(&self, kind: IntentKind) -> IntentDefaults {
        self.intents.get(kind.key()).or_else(|| self.intents.get("other")).cloned().unwrap_or(IntentDefaults {
            template: "blank".into(),
            outputs: vec!["youtube_1080p".into()],
            captions: "sentence".into(),
            pacing: "measured".into(),
            music_level: "low".into(),
        })
    }
}

/// Saves a user preset to the app library (duplicating a built-in one makes a user preset).
pub fn save_user_preset(library: &Utf8Path, preset: &OutputPreset) -> Result<()> {
    if preset.id.trim().is_empty() || preset.name.trim().is_empty() {
        return Err(Error::other("an output preset needs an ID and a name"));
    }
    if preset.targets.is_empty() || preset.targets.iter().any(|t| parse_target(t).is_none()) {
        return Err(Error::other("an output preset needs at least one valid cutline target"));
    }
    let mut p = preset.clone();
    p.user = false;
    paths::write_json_atomic(&library.join("output-presets").join(format!("{}.json", ids::slug(&preset.id))), &p)
}

pub fn delete_user_preset(library: &Utf8Path, id: &str) -> Result<()> {
    let f = library.join("output-presets").join(format!("{}.json", ids::slug(id)));
    if f.exists() {
        std::fs::remove_file(f)?;
    }
    Ok(())
}

/// A copy of a preset as a new user preset.
pub fn duplicate_preset(platforms: &Platforms, id: &str, new_name: &str) -> Result<OutputPreset> {
    let src = platforms.preset(id).ok_or_else(|| Error::NotFound(format!("no output preset \"{id}\"")))?;
    let taken: HashSet<String> = platforms.presets.iter().map(|p| p.id.clone()).collect();
    let mut p = src.clone();
    p.id = ids::unique_slug_id("user", new_name, &taken).replace('-', "_");
    p.name = new_name.to_string();
    p.user = true;
    Ok(p)
}

/// Per-project changes on top of a preset.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OutputOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub looped: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formats: Option<Vec<String>>,
}

/// A named delivery target in a project: a preset plus overrides.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OutputConfig {
    pub id: String,
    pub preset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub overrides: OutputOverrides,
}

impl OutputConfig {
    pub fn from_preset(preset: &OutputPreset, taken: &HashSet<String>) -> Self {
        OutputConfig { id: ids::unique_slug_id("out", &preset.id, taken).replace('-', "_"), preset: preset.id.clone(), name: None, overrides: OutputOverrides::default() }
    }
}

pub fn parse_target(name: &str) -> Option<TargetName> {
    serde_json::from_value(Value::String(name.to_string())).ok()
}

/// One file a run must produce.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportSpec {
    /// cutline export name, unique in the run.
    pub name: String,
    pub format: String,
    /// Path relative to the run folder: `outputs/<slug>/<name>.<ext>`.
    pub out: String,
}

/// An output config with its preset applied: everything validation and the run need.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedOutput {
    pub id: String,
    pub name: String,
    /// Folder name under `outputs/`, unique in the run.
    pub slug: String,
    pub preset: String,
    pub platform: String,
    pub target: String,
    pub width: u32,
    pub height: u32,
    pub fps: Option<f64>,
    pub audio: bool,
    #[serde(rename = "loop")]
    pub looped: bool,
    pub captions: String,
    pub max_bytes: Option<u64>,
    pub limits: Limits,
    pub exports: Vec<ExportSpec>,
    pub notes: String,
}

impl ResolvedOutput {
    pub fn aspect(&self) -> f64 {
        self.width as f64 / self.height.max(1) as f64
    }
    pub fn aspect_label(&self) -> String {
        let a = self.aspect();
        for (label, v) in [("16:9", 16.0 / 9.0), ("9:16", 9.0 / 16.0), ("1:1", 1.0), ("4:5", 0.8), ("4:3", 4.0 / 3.0), ("3:4", 0.75)] {
            if (a - v).abs() < 0.02 {
                return label.to_string();
            }
        }
        format!("{}×{}", self.width, self.height)
    }
    /// The effective duration cap: the tighter of the platform limit and the override.
    pub fn max_duration_s(&self) -> Option<f64> {
        self.limits.max_duration_s
    }
}

/// Applies a preset and the config's overrides. `taken_slugs` keeps folder names unique.
pub fn resolve_output(config: &OutputConfig, platforms: &Platforms, taken_slugs: &mut HashSet<String>) -> Result<ResolvedOutput> {
    let preset = platforms
        .preset(&config.preset)
        .ok_or_else(|| Error::NotFound(format!("output \"{}\" uses the preset \"{}\", which doesn't exist", config.name.as_deref().unwrap_or(&config.id), config.preset)))?;
    let o = &config.overrides;
    let target_name = o.target.clone().unwrap_or_else(|| preset.targets.first().cloned().unwrap_or_else(|| "long_1080p".into()));
    let target = parse_target(&target_name).ok_or_else(|| Error::other(format!("\"{target_name}\" isn't a cutline target")))?;
    let (w, h) = if target == TargetName::Custom { (o.width.or(preset.width), o.height.or(preset.height)) } else { (None, None) };
    let spec = cutline_core::target::resolve(target, None, None, w, h);
    let fps = o.fps.or(preset.fps);
    let mut limits = preset.limits.clone();
    // cutline's own cap for the target (Shorts: 180 s) and the override both tighten the limit.
    for cap in [spec.max_duration, o.max_duration_s].into_iter().flatten() {
        limits.max_duration_s = Some(limits.max_duration_s.map_or(cap, |m| m.min(cap)));
    }
    let name = config.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| preset.name.clone());
    let mut slug = ids::slug(&name);
    if slug.is_empty() {
        slug = "output".into();
    }
    let base = slug.clone();
    let mut n = 2;
    while !taken_slugs.insert(slug.clone()) {
        slug = format!("{base}-{n}");
        n += 1;
    }
    let formats = o.formats.clone().filter(|f| !f.is_empty()).unwrap_or_else(|| preset.formats.clone());
    let formats: Vec<String> = formats.into_iter().filter(|f| f == "mp4" || f == "webm").collect();
    let formats = if formats.is_empty() { vec!["mp4".to_string()] } else { formats };
    let exports = formats
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let export_name = if i == 0 { slug.clone() } else { format!("{slug}-{f}") };
            ExportSpec { out: format!("outputs/{slug}/{export_name}.{f}"), name: export_name, format: f.clone() }
        })
        .collect();
    let looped = o.looped.unwrap_or(preset.looped);
    Ok(ResolvedOutput {
        id: config.id.clone(),
        name,
        slug,
        preset: preset.id.clone(),
        platform: preset.platform.clone(),
        target: target_name,
        width: spec.width,
        height: spec.height,
        fps,
        // a seamless loop needs a silent file (cutline: `loop` needs `audio: false`)
        audio: o.audio.unwrap_or(preset.audio) && !looped,
        looped,
        captions: o.captions.clone().unwrap_or_else(|| preset.captions.clone()),
        max_bytes: o.max_bytes.or(preset.max_bytes),
        limits,
        exports,
        notes: preset.notes.clone(),
    })
}

/// The outputs a run config selects, resolved in order.
pub fn resolve_run_outputs(project: &Project, rc: &RunConfig, platforms: &Platforms) -> Result<Vec<ResolvedOutput>> {
    let mut taken = HashSet::new();
    let mut out = Vec::new();
    for id in &rc.outputs {
        let cfg = project.output_config(id).ok_or_else(|| Error::NotFound(format!("run config \"{}\" selects the output \"{id}\", which doesn't exist", rc.name)))?;
        out.push(resolve_output(cfg, platforms, &mut taken)?);
    }
    Ok(out)
}

/// The `target` and `exports` section of `project.json5` for these outputs (spec §12.2): the
/// first output's target becomes the project target, and every file is one cutline export.
pub fn cutline_exports(outputs: &[ResolvedOutput]) -> Value {
    let mut exports = Vec::new();
    for o in outputs {
        for e in &o.exports {
            let mut x = serde_json::Map::new();
            x.insert("name".into(), json!(e.name));
            x.insert("target".into(), json!(o.target));
            if o.target == "custom" {
                x.insert("width".into(), json!(o.width));
                x.insert("height".into(), json!(o.height));
            }
            if let Some(fps) = o.fps {
                x.insert("fps".into(), json!(fps));
            }
            if e.format != "mp4" {
                x.insert("format".into(), json!(e.format));
            }
            if !o.audio {
                x.insert("audio".into(), json!(false));
            }
            if o.looped {
                x.insert("loop".into(), json!(true));
            }
            if let Some(b) = o.max_bytes {
                x.insert("max_bytes".into(), json!(b));
            }
            if o.captions != "burn" {
                x.insert("captions".into(), json!(o.captions));
            }
            x.insert("out".into(), json!(e.out));
            exports.push(Value::Object(x));
        }
    }
    let first = outputs.first();
    let mut root = serde_json::Map::new();
    if let Some(f) = first {
        root.insert("target".into(), json!(f.target));
        if f.target == "custom" {
            root.insert("width".into(), json!(f.width));
            root.insert("height".into(), json!(f.height));
        }
    }
    root.insert("exports".into(), Value::Array(exports));
    Value::Object(root)
}

/// Rough size of one rendered output in bytes, for the free disk space check: the size cap when
/// set, else a bitrate guess from the frame size.
pub fn estimate_output_bytes(o: &ResolvedOutput, duration_s: f64) -> u64 {
    let per_file = match o.max_bytes {
        Some(b) => b,
        None => {
            let pixels = o.width as f64 * o.height as f64;
            // ~8 Mb/s at 1080p, scaled by area, plus audio
            let video_bps = 8_000_000.0 * (pixels / (1920.0 * 1080.0)).max(0.25);
            let audio_bps = if o.audio { 192_000.0 } else { 0.0 };
            ((video_bps + audio_bps) / 8.0 * duration_s) as u64
        }
    };
    per_file * o.exports.len() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(id: &str, preset: &str) -> OutputConfig {
        OutputConfig { id: id.into(), preset: preset.into(), name: None, overrides: OutputOverrides::default() }
    }

    #[test]
    fn bundled_data_is_complete() {
        let p = Platforms::bundled();
        for id in [
            "youtube_1080p", "youtube_4k", "youtube_shorts", "tiktok", "instagram_reels", "instagram_feed", "linkedin", "x", "facebook",
            "app_store_preview", "google_play_promo", "website_hero_loop", "custom",
        ] {
            let preset = p.preset(id).unwrap_or_else(|| panic!("missing preset {id}"));
            assert!(preset.targets.iter().all(|t| parse_target(t).is_some()), "{id} has an unknown target");
            let mut taken = HashSet::new();
            resolve_output(&cfg("o", id), &p, &mut taken).unwrap();
        }
        for k in IntentKind::ALL {
            let d = p.intent_defaults(k);
            assert!(d.outputs.iter().all(|o| p.preset(o).is_some()), "{k:?}");
            assert!(crate::spec::template(&d.template).is_some(), "{k:?} names a missing template");
        }
    }

    #[test]
    fn resolves_geometry_and_limits() {
        let p = Platforms::bundled();
        let mut taken = HashSet::new();
        let shorts = resolve_output(&cfg("a", "youtube_shorts"), &p, &mut taken).unwrap();
        assert_eq!((shorts.width, shorts.height, shorts.target.as_str()), (1080, 1920, "shorts"));
        assert_eq!(shorts.limits.max_duration_s, Some(180.0));
        assert_eq!(shorts.aspect_label(), "9:16");
        let reels = resolve_output(&cfg("b", "instagram_reels"), &p, &mut taken).unwrap();
        assert_eq!(reels.limits.max_duration_s, Some(90.0));
        // TikTok allows 10 minutes but cutline's shorts target stops at 180 s
        let tiktok = resolve_output(&cfg("c", "tiktok"), &p, &mut taken).unwrap();
        assert_eq!(tiktok.limits.max_duration_s, Some(180.0));
        let mut li = cfg("d", "linkedin");
        assert_eq!(resolve_output(&li, &p, &mut HashSet::new()).unwrap().target, "portrait_4x5");
        li.overrides.target = Some("square".into());
        li.overrides.max_duration_s = Some(60.0);
        let li = resolve_output(&li, &p, &mut HashSet::new()).unwrap();
        assert_eq!((li.width, li.height, li.limits.max_duration_s), (1080, 1080, Some(60.0)));
        assert_eq!(li.captions, "burn", "LinkedIn captions are on by default");
        let app = resolve_output(&cfg("e", "app_store_preview"), &p, &mut HashSet::new()).unwrap();
        assert_eq!((app.width, app.height, app.fps), (886, 1920, Some(30.0)));
        assert_eq!((app.limits.min_duration_s, app.limits.max_duration_s), (Some(15.0), Some(30.0)));
    }

    #[test]
    fn slugs_are_unique_and_named_configs_use_their_name() {
        let p = Platforms::bundled();
        let mut taken = HashSet::new();
        let a = resolve_output(&cfg("a", "linkedin"), &p, &mut taken).unwrap();
        let b = resolve_output(&cfg("b", "linkedin"), &p, &mut taken).unwrap();
        let mut c = cfg("c", "linkedin");
        c.name = Some("LinkedIn 4:5 (EU)".into());
        let c = resolve_output(&c, &p, &mut taken).unwrap();
        assert_eq!((a.slug.as_str(), b.slug.as_str(), c.slug.as_str()), ("linkedin", "linkedin-2", "linkedin-4-5-eu"));
    }

    #[test]
    fn maps_run_config_to_cutline_exports() {
        let p = Platforms::bundled();
        let mut project = Project::new("Launch");
        project.output_configs = vec![cfg("out_yt", "youtube_1080p"), cfg("out_shorts", "youtube_shorts"), cfg("out_loop", "website_hero_loop"), cfg("out_app", "app_store_preview")];
        let mut rc = RunConfig::new("Everything");
        rc.outputs = project.output_configs.iter().map(|o| o.id.clone()).collect();
        let outs = resolve_run_outputs(&project, &rc, &p).unwrap();
        let v = cutline_exports(&outs);
        assert_eq!(v["target"], "long_1080p");
        let ex = v["exports"].as_array().unwrap();
        assert_eq!(ex.len(), 5, "the hero loop exports MP4 and WebM");
        assert_eq!(ex[0], json!({"name": "youtube-1080p", "target": "long_1080p", "captions": "sidecar", "out": "outputs/youtube-1080p/youtube-1080p.mp4"}));
        assert_eq!(ex[1]["target"], "shorts");
        assert_eq!(ex[2], json!({"name": "website-hero-loop", "target": "long_1080p", "audio": false, "loop": true, "max_bytes": 8000000, "captions": "none", "out": "outputs/website-hero-loop/website-hero-loop.mp4"}));
        assert_eq!(ex[3]["format"], "webm");
        assert_eq!(ex[3]["out"], "outputs/website-hero-loop/website-hero-loop-webm.webm");
        assert_eq!((ex[4]["target"].as_str(), ex[4]["width"].as_u64(), ex[4]["height"].as_u64(), ex[4]["fps"].as_f64()), (Some("custom"), Some(886), Some(1920), Some(30.0)));
        // what cutline accepts: the export section parses as cutline exports
        let exports: Vec<cutline_core::model::Export> = serde_json::from_value(v["exports"].clone()).expect("cutline accepts the exports");
        assert_eq!(exports.len(), 5);
        // a dangling output is an error
        rc.outputs.push("nope".into());
        assert!(resolve_run_outputs(&project, &rc, &p).is_err());
    }

    #[test]
    fn user_presets_and_library_override() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Utf8Path::from_path(dir.path()).unwrap();
        let base = Platforms::load(lib);
        let mine = duplicate_preset(&base, "linkedin", "LinkedIn square 60 s").unwrap();
        assert!(mine.user && mine.id.starts_with("user_"));
        save_user_preset(lib, &mine).unwrap();
        let loaded = Platforms::load(lib);
        assert!(loaded.preset(&mine.id).unwrap().user);
        assert_eq!(loaded.presets.len(), base.presets.len() + 1);
        delete_user_preset(lib, &mine.id).unwrap();
        assert!(Platforms::load(lib).preset(&mine.id).is_none());
        // a newer platforms.json in the library replaces the bundled limits
        let mut newer = Platforms::bundled();
        newer.updated = "2099-01-01".into();
        newer.presets.iter_mut().find(|p| p.id == "x").unwrap().limits.max_duration_s = Some(600.0);
        paths::write_json_atomic(&lib.join("platforms.json"), &newer).unwrap();
        assert_eq!(Platforms::load(lib).preset("x").unwrap().limits.max_duration_s, Some(600.0));
    }

    #[test]
    fn size_estimates() {
        let p = Platforms::bundled();
        let yt = resolve_output(&cfg("a", "youtube_1080p"), &p, &mut HashSet::new()).unwrap();
        let b = estimate_output_bytes(&yt, 60.0);
        assert!(b > 50_000_000 && b < 80_000_000, "{b}");
        let lp = resolve_output(&cfg("b", "website_hero_loop"), &p, &mut HashSet::new()).unwrap();
        assert_eq!(estimate_output_bytes(&lp, 10.0), 16_000_000);
    }
}
