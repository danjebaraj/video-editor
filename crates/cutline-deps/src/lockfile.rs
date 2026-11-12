//! `tools.lock.json`: pinned external tool versions + sha256.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockEntry {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub installed_at: Option<String>,
    /// `pass`, `fail`, `skipped` or absent when selftest was not run.
    #[serde(default)]
    pub selftest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub tools: Vec<LockEntry>,
}

fn one() -> u32 {
    1
}

impl Lockfile {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self { version: 1, tools: vec![] });
        }
        let s = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        // write atomically
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, s)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn get(&self, name: &str, platform: &str) -> Option<&LockEntry> {
        self.tools.iter().find(|t| t.name == name && t.platform == platform)
    }

    /// Insert or replace the entry for (name, platform). Entries stay sorted for stable diffs.
    pub fn upsert(&mut self, e: LockEntry) {
        self.tools.retain(|t| !(t.name == e.name && t.platform == e.platform));
        self.tools.push(e);
        self.tools.sort_by(|a, b| (&a.name, &a.platform).cmp(&(&b.name, &b.platform)));
    }
}

/// Lockfile location: `CUTLINE_LOCKFILE`, else the nearest `tools.lock.json` in the cwd's
/// ancestors (the repo's checked-in pin), else `<tools_dir>/tools.lock.json`.
pub fn default_lockfile_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CUTLINE_LOCKFILE") {
        return PathBuf::from(p);
    }
    if let Ok(cwd) = std::env::current_dir() {
        for d in cwd.ancestors() {
            let c = d.join("tools.lock.json");
            if c.exists() {
                return c;
            }
        }
    }
    crate::paths::tools_dir().join("tools.lock.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_upsert() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tools.lock.json");
        let mut l = Lockfile::load(&p).unwrap();
        assert!(l.tools.is_empty());
        let e = LockEntry {
            name: "ffmpeg".into(),
            version: "9.0".into(),
            platform: "macos-arm64".into(),
            url: "https://x".into(),
            sha256: "ab".into(),
            installed_at: None,
            selftest: None,
            license: None,
        };
        l.upsert(e.clone());
        l.upsert(LockEntry { version: "9.1".into(), ..e.clone() });
        assert_eq!(l.tools.len(), 1);
        l.save(&p).unwrap();
        let l2 = Lockfile::load(&p).unwrap();
        assert_eq!(l2.get("ffmpeg", "macos-arm64").unwrap().version, "9.1");
    }
}
