//! Simple content-addressed JSON/file cache.

use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone)]
pub struct Cache {
    pub root: PathBuf,
}

impl Cache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Global cache (`<cache_dir>/<ns>`).
    pub fn global(ns: &str) -> Self {
        Self::new(cutline_deps::paths::cache_dir().join(ns))
    }

    pub fn path(&self, key: &str, ext: &str) -> PathBuf {
        self.root
            .join(&key[..2.min(key.len())])
            .join(format!("{key}.{ext}"))
    }

    pub fn get_json<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let p = self.path(key, "json");
        let s = std::fs::read_to_string(p).ok()?;
        serde_json::from_str(&s).ok()
    }

    pub fn put_json<T: Serialize>(&self, key: &str, v: &T) -> Result<PathBuf> {
        let p = self.path(key, "json");
        std::fs::create_dir_all(p.parent().unwrap())?;
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
        std::fs::rename(&tmp, &p)?;
        Ok(p)
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CacheStats {
    pub root: String,
    pub files: u64,
    pub bytes: u64,
}

fn walk(p: &Path, f: &mut dyn FnMut(&Path, &std::fs::Metadata)) {
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if let Ok(m) = e.metadata() {
                if m.is_dir() {
                    walk(&path, f);
                } else {
                    f(&path, &m);
                }
            }
        }
    }
}

pub fn stats(root: &Path) -> CacheStats {
    let mut s = CacheStats {
        root: root.display().to_string(),
        ..Default::default()
    };
    walk(root, &mut |_, m| {
        s.files += 1;
        s.bytes += m.len();
    });
    s
}

/// Delete files not modified within `older_than` (or everything when `None`). Returns stats of
/// what was removed.
pub fn clean(root: &Path, older_than: Option<Duration>) -> CacheStats {
    let mut s = CacheStats {
        root: root.display().to_string(),
        ..Default::default()
    };
    let now = SystemTime::now();
    walk(root, &mut |p, m| {
        let old = match older_than {
            None => true,
            Some(d) => m
                .modified()
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .map(|age| age > d)
                .unwrap_or(false),
        };
        if old && std::fs::remove_file(p).is_ok() {
            s.files += 1;
            s.bytes += m.len();
        }
    });
    s
}

/// Parse durations like `30d`, `12h`, `45m`, `10s`.
pub fn parse_age(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = n.parse().ok()?;
    let secs = match unit {
        "d" => n * 86400,
        "h" => n * 3600,
        "m" => n * 60,
        "s" => n,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ages() {
        assert_eq!(parse_age("30d"), Some(Duration::from_secs(30 * 86400)));
        assert_eq!(parse_age("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_age("x"), None);
    }
}
