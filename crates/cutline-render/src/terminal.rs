//! Terminal recordings as assets: an asset whose path is a VHS `.tape` script is recorded with
//! `vhs` when the project loads and replaced by the cached video. The cache key is the tape's
//! content and the vhs version, so editing the tape re-records it and nothing else does.

use anyhow::{bail, Context, Result};
use cutline_core::diag::{ptr_token, Diagnostic};
use cutline_core::model::Project;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `.tape` assets are VHS scripts.
pub fn is_tape(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".tape")
}

/// The vhs binary: `CUTLINE_VHS`, else `vhs` on `PATH`.
pub fn vhs_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUTLINE_VHS") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let exe = if cfg!(windows) { "vhs.exe" } else { "vhs" };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(exe)).find(|p| p.is_file())
}

/// Tape text with its `Output` lines replaced by one writing `out`.
pub fn retarget(tape: &str, out: &Path) -> String {
    let body: Vec<&str> = tape.lines().filter(|l| !l.trim_start().starts_with("Output")).collect();
    let out = out.to_string_lossy().replace('\\', "/").replace('"', "\\\"");
    format!("Output \"{out}\"\n{}\n", body.join("\n"))
}

/// Record every `.tape` asset (cached) and point the asset at the recording. Returns errors as
/// diagnostics on the asset's `path`.
pub fn record_tapes(p: &mut Project, dir: &Path, cache: &Path) -> Vec<Diagnostic> {
    let mut d = vec![];
    let ids: Vec<String> = p.assets.iter().filter(|(_, a)| is_tape(&a.path)).map(|(id, _)| id.clone()).collect();
    if ids.is_empty() {
        return d;
    }
    let Some(vhs) = vhs_binary() else {
        for id in ids {
            d.push(
                Diagnostic::error(
                    "E_TOOL_MISSING",
                    format!("/assets/{}/path", ptr_token(&id)),
                    "this asset is a VHS tape, but `vhs` was not found",
                )
                .hint("install it (`brew install vhs`, or `go install github.com/charmbracelet/vhs@latest` plus ttyd) or set CUTLINE_VHS"),
            );
        }
        return d;
    };
    for id in ids {
        let tape = dir.join(&p.assets[&id].path);
        match record(&vhs, &tape, cache) {
            Ok(video) => {
                let a = p.assets.get_mut(&id).unwrap();
                a.path = video.to_string_lossy().to_string();
            }
            Err(e) => d.push(Diagnostic::error(
                "E_TAPE_FAILED",
                format!("/assets/{}/path", ptr_token(&id)),
                format!("recording {} failed: {e:#}", tape.display()),
            )),
        }
    }
    d
}

fn vhs_version(vhs: &Path) -> String {
    Command::new(vhs)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Record `tape` into the cache (or reuse an earlier recording of the same tape).
pub fn record(vhs: &Path, tape: &Path, cache: &Path) -> Result<PathBuf> {
    record_with(vhs, tape, cache, false)
}

/// `record`; with `force`, record again even when the tape is cached (fresh live data such as
/// clocks or process lists). The new recording replaces the cached one only once it succeeds.
pub fn record_with(vhs: &Path, tape: &Path, cache: &Path, force: bool) -> Result<PathBuf> {
    let text = std::fs::read_to_string(tape).with_context(|| format!("cannot read {}", tape.display()))?;
    let key = cutline_media::hash_key(&[text.as_bytes(), vhs_version(vhs).as_bytes(), b"tape-v1"]);
    let out = cache.join(format!("{key}.mp4"));
    if out.is_file() && !force {
        return Ok(out);
    }
    std::fs::create_dir_all(cache)?;
    let tmp = cache.join(format!("{key}.tmp.mp4"));
    let script = cache.join(format!("{key}.tape"));
    std::fs::write(&script, retarget(&text, &tmp))?;
    // vhs finds ffmpeg and ttyd on PATH: put cutline's ffmpeg first
    let mut path = vec![];
    if let Ok(tc) = cutline_deps::toolchain() {
        if let Some(d) = tc.ffmpeg.parent() {
            path.push(d.to_path_buf());
        }
    }
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    tracing::info!("recording terminal tape {}", tape.display());
    let o = Command::new(vhs)
        .arg(&script)
        // relative paths inside the tape (MarginFill, Source, scripts) resolve next to it
        .current_dir(tape.parent().unwrap_or(Path::new(".")))
        .env("PATH", std::env::join_paths(path)?)
        .output()
        .context("spawning vhs")?;
    let _ = std::fs::remove_file(&script);
    if !o.status.success() || !tmp.is_file() {
        let err = String::from_utf8_lossy(&o.stderr);
        let out_s = String::from_utf8_lossy(&o.stdout);
        let tail: Vec<&str> = err.lines().chain(out_s.lines()).rev().take(12).collect();
        bail!("vhs exited with {}:\n{}", o.status, tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    std::fs::rename(&tmp, &out)?;
    Ok(out)
}

/// Re-record the `.tape` assets of the project at `project` (all, or the one named `only`).
/// Returns `(asset id, recording)` for each.
pub fn rerecord(project: &Path, only: Option<&str>) -> Result<Vec<(String, PathBuf)>> {
    let project = std::fs::canonicalize(project).with_context(|| format!("cannot open {}", project.display()))?;
    let p = crate::pipeline::parse_file(&project).map_err(|e| anyhow::anyhow!("{e}"))?;
    let dir = project.parent().unwrap_or(Path::new("."));
    let tapes: Vec<(&String, &cutline_core::model::Asset)> =
        p.assets.iter().filter(|(id, a)| is_tape(&a.path) && only.map_or(true, |o| o == id.as_str())).collect();
    if tapes.is_empty() {
        match only {
            Some(o) => bail!("no `.tape` asset named `{o}`"),
            None => bail!("the project has no `.tape` assets"),
        }
    }
    let vhs = vhs_binary().context("`vhs` not found (install it or set CUTLINE_VHS)")?;
    let cache = crate::cutline_dir(dir).join("cache").join("terminal");
    tapes.into_iter().map(|(id, a)| Ok((id.clone(), record_with(&vhs, &dir.join(&a.path), &cache, true)?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retarget_replaces_output_lines() {
        let t = "Output demo.gif\nSet FontSize 22\n  Output \"other.mp4\"\nType \"ls\"\n";
        let r = retarget(t, Path::new("/cache/k.tmp.mp4"));
        assert_eq!(r, "Output \"/cache/k.tmp.mp4\"\nSet FontSize 22\nType \"ls\"\n");
        assert!(is_tape("x/Demo.TAPE") && !is_tape("x/demo.mp4"));
    }
}
