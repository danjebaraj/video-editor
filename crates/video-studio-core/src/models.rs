//! The Claude model list (spec §4.2). It ships with the core as `models.json` and can be
//! refreshed by placing a newer copy in the app library; nothing else hard-codes model names.

use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::paths;

pub const BUNDLED_MODELS: &str = include_str!("../data/models.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Price {
    /// USD per million input tokens.
    pub input: f64,
    /// USD per million output tokens.
    pub output: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelVersion {
    /// Full model ID passed to `--model`.
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelFamily {
    pub id: String,
    pub name: String,
    /// CLI alias that always means the latest version of the family.
    pub alias: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<Price>,
    pub versions: Vec<ModelVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelDefaults {
    pub assistant: String,
    pub dry_run: String,
    pub production: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelList {
    pub version: u32,
    #[serde(default)]
    pub updated: String,
    pub families: Vec<ModelFamily>,
    pub defaults: ModelDefaults,
}

impl ModelList {
    pub fn bundled() -> Self {
        serde_json::from_str(BUNDLED_MODELS).expect("bundled models.json is valid")
    }

    /// The bundled list, or `<library>/models.json` when that copy is newer.
    pub fn load(library: &Utf8Path) -> Self {
        let bundled = Self::bundled();
        match paths::read_json::<ModelList>(&library.join("models.json")) {
            Ok(l) if !l.families.is_empty() && l.updated >= bundled.updated => l,
            _ => bundled,
        }
    }

    /// The family a model string belongs to: an alias, a listed ID, or an ID containing the
    /// family name (so a custom ID like `claude-opus-9` still gets a price).
    pub fn family_of(&self, model: &str) -> Option<&ModelFamily> {
        let m = model.trim().to_ascii_lowercase();
        self.families
            .iter()
            .find(|f| f.alias == m || f.versions.iter().any(|v| v.id == m))
            .or_else(|| self.families.iter().find(|f| m.contains(&f.id)))
    }

    /// Display name: "Opus (latest)", "Opus 5.5", or the custom ID itself.
    pub fn display_name(&self, model: &str) -> String {
        let m = model.trim();
        for f in &self.families {
            if f.alias == m {
                return format!("{} (latest)", f.name);
            }
            if let Some(v) = f.versions.iter().find(|v| v.id == m) {
                return v.name.clone();
            }
        }
        m.to_string()
    }

    /// True when `a` is a cheaper family than `b` (used to warn when the dry run model isn't).
    pub fn is_cheaper(&self, a: &str, b: &str) -> Option<bool> {
        let pa = self.family_of(a)?.price.as_ref()?;
        let pb = self.family_of(b)?.price.as_ref()?;
        Some(pa.output < pb.output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_list() {
        let l = ModelList::bundled();
        for d in [&l.defaults.assistant, &l.defaults.dry_run, &l.defaults.production] {
            assert!(l.family_of(d).is_some(), "{d}");
        }
        assert_eq!(l.display_name("opus"), "Opus (latest)");
        assert_eq!(l.display_name("claude-haiku-5-5"), "Haiku 5.5");
        assert_eq!(l.display_name("my-custom-model"), "my-custom-model");
        assert_eq!(l.family_of("claude-sonnet-9-9").unwrap().id, "sonnet");
        assert_eq!(l.is_cheaper("haiku", "opus"), Some(true));
        assert_eq!(l.is_cheaper("opus", "haiku"), Some(false));
        assert_eq!(l.is_cheaper("mystery", "opus"), None);
    }

    #[test]
    fn library_copy_replaces_bundled_when_newer() {
        let d = tempfile::tempdir().unwrap();
        let lib = Utf8Path::from_path(d.path()).unwrap();
        assert_eq!(ModelList::load(lib), ModelList::bundled());
        let mut newer = ModelList::bundled();
        newer.updated = "2999-01-01".into();
        newer.families[0].versions.push(ModelVersion { id: "claude-new".into(), name: "New".into() });
        paths::write_json_atomic(&lib.join("models.json"), &newer).unwrap();
        assert_eq!(ModelList::load(lib).display_name("claude-new"), "New");
        newer.updated = "2000-01-01".into();
        paths::write_json_atomic(&lib.join("models.json"), &newer).unwrap();
        assert_eq!(ModelList::load(lib), ModelList::bundled());
    }
}
