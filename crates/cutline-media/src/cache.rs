//! Simple content-addressed JSON/file cache.

use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone)]
pub struct Cache {
    pub root: PathBuf,
    /// Read after `root`, and written when `root` can't be: the project's own copy of a shared
    /// cache (see [`Cache::shared`]).
    pub fallback: Option<PathBuf>,
}

impl Cache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), fallback: None }
    }

    /// Global cache (`<cache_dir>/<ns>`).
    pub fn global(ns: &str) -> Self {
        Self::new(cutline_deps::paths::cache_dir().join(ns))
    }

    /// The global cache `<cache_dir>/<ns>`, backed by the project's `<project>/.cutline/cache/<ns>`.
    /// Entries are shared between projects where the global cache can be written; where it
    /// can't (Codex's sandbox lets a run write only its own folder), they are kept in the
    /// project instead of being lost on every run.
    pub fn shared(ns: &str, project_dir: &Path) -> Self {
        Self { fallback: Some(project_dir.join(".cutline").join("cache").join(ns)), ..Self::global(ns) }
    }

    pub fn path(&self, key: &str, ext: &str) -> PathBuf {
        entry(&self.root, key, ext)
    }

    pub fn get_json<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        std::iter::once(&self.root).chain(&self.fallback).find_map(|root| {
            let s = std::fs::read_to_string(entry(root, key, "json")).ok()?;
            serde_json::from_str(&s).ok()
        })
    }

    /// Saves a value when it can: in `root`, else in `fallback`. The cache only saves time, so
    /// a folder this process may not write (a command run in Codex's sandbox can't write the
    /// global cache) must not fail the work being cached: that failed `cutline beats` with
    /// "Operation not permitted".
    pub fn store_json<T: Serialize>(&self, key: &str, v: &T) {
        let Err(e) = put_json_in(&self.root, key, v) else { return };
        let Some(fb) = &self.fallback else {
            tracing::warn!("couldn't save to the cache in {}: {e:#}", self.root.display());
            return;
        };
        match put_json_in(fb, key, v) {
            Ok(_) => tracing::debug!("saved to the project cache {} ({}: {e:#})", fb.display(), self.root.display()),
            Err(e2) => tracing::warn!(
                "couldn't save to the cache in {} ({e:#}) or in {} ({e2:#})",
                self.root.display(),
                fb.display()
            ),
        }
    }

    pub fn put_json<T: Serialize>(&self, key: &str, v: &T) -> Result<PathBuf> {
        put_json_in(&self.root, key, v)
    }
}

fn entry(root: &Path, key: &str, ext: &str) -> PathBuf {
    root.join(&key[..2.min(key.len())]).join(format!("{key}.{ext}"))
}

fn put_json_in<T: Serialize>(root: &Path, key: &str, v: &T) -> Result<PathBuf> {
    let p = entry(root, key, "json");
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(p)
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
    let mut s = CacheStats { root: root.display().to_string(), ..Default::default() };
    walk(root, &mut |_, m| {
        s.files += 1;
        s.bytes += m.len();
    });
    s
}

/// Delete files not modified within `older_than` (or everything when `None`). Returns stats of
/// what was removed.
pub fn clean(root: &Path, older_than: Option<Duration>) -> CacheStats {
    let mut s = CacheStats { root: root.display().to_string(), ..Default::default() };
    let now = SystemTime::now();
    walk(root, &mut |p, m| {
        let old = match older_than {
            None => true,
            Some(d) => m.modified().ok().and_then(|t| now.duration_since(t).ok()).map(|age| age > d).unwrap_or(false),
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
    fn a_cache_that_cant_be_written_doesnt_fail_the_caller() {
        let d = std::env::temp_dir().join(format!("cutline-cache-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let blocked = d.join("not-a-folder");
        std::fs::write(&blocked, b"").unwrap();
        let c = Cache::new(blocked.join("beats"));
        assert!(c.put_json("abcd", &1).is_err());
        c.store_json("abcd", &1);
        assert_eq!(c.get_json::<i32>("abcd"), None);
        let ok = Cache::new(d.join("beats"));
        ok.store_json("abcd", &2);
        assert_eq!(ok.get_json::<i32>("abcd"), Some(2));
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn a_shared_cache_keeps_entries_in_the_project_when_the_global_one_is_read_only() {
        let d = std::env::temp_dir().join(format!("cutline-cache-shared-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // a file where the global folder would be: nothing can be created under it
        let blocked = d.join("global");
        std::fs::write(&blocked, b"").unwrap();
        let project = d.join("project");
        let c = Cache { root: blocked.join("beats"), ..Cache::shared("beats", &project) };
        assert_eq!(c.fallback.as_deref(), Some(project.join(".cutline/cache/beats").as_path()));
        c.store_json("abcd", &3);
        assert_eq!(c.get_json::<i32>("abcd"), Some(3));
        assert!(project.join(".cutline/cache/beats/ab/abcd.json").is_file());
        // a writable global cache is preferred, and read first
        let g = Cache { root: d.join("ok").join("beats"), ..Cache::shared("beats", &project) };
        g.store_json("abcd", &4);
        assert_eq!(g.get_json::<i32>("abcd"), Some(4));
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn ages() {
        assert_eq!(parse_age("30d"), Some(Duration::from_secs(30 * 86400)));
        assert_eq!(parse_age("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_age("x"), None);
    }
}
