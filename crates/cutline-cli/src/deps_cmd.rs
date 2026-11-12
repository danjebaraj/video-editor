use crate::out::*;
use cutline_deps::install::{self, Release};
use cutline_deps::lockfile::{default_lockfile_path, Lockfile};
use cutline_deps::paths::{exe_name, platform_id, tools_dir};
use serde_json::json;

pub fn install_ffmpeg(force_latest: bool) -> CmdResult {
    let lock_path = default_lockfile_path();
    let mut lock = Lockfile::load(&lock_path).tool_err()?;
    let rel = match (force_latest, install::pinned_ffmpeg(&lock)) {
        (false, Some(r)) => r,
        _ => install::resolve_latest_ffmpeg().tool_err()?,
    };
    let dir = install::stage_ffmpeg(&rel).tool_err()?;
    install::promote_ffmpeg(&rel.version).tool_err()?;
    install::record(&mut lock, &rel, None);
    lock.save(&lock_path).tool_err()?;
    Ok(Output::ok(
        json!({"ok": true, "tool": "ffmpeg", "version": rel.version, "source": rel.source, "dir": dir, "lockfile": lock_path}),
        format!("installed ffmpeg {} into {}\nlockfile: {}", rel.version, dir.display(), lock_path.display()),
    ))
}

pub struct ModelSpec {
    pub file: &'static str,
    pub url: String,
}

pub fn whisper_model_spec(name: &str) -> ModelSpec {
    let file: &'static str = Box::leak(format!("ggml-{name}.bin").into_boxed_str());
    ModelSpec { file, url: format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{file}") }
}

pub fn install_model(kind: &str, name: &str, url: &str, file: &str) -> CmdResult {
    let lock_path = default_lockfile_path();
    let mut lock = Lockfile::load(&lock_path).tool_err()?;
    let pinned = lock.get(name, "any").map(|e| e.sha256.clone()).filter(|s| !s.is_empty());
    let (path, sha) = install::install_file("models", file, url, pinned.as_deref()).tool_err()?;
    lock.upsert(cutline_deps::lockfile::LockEntry {
        name: name.to_string(),
        version: file.to_string(),
        platform: "any".into(),
        url: url.to_string(),
        sha256: sha.clone(),
        installed_at: None,
        selftest: None,
        license: Some(if kind == "whisper" { "MIT".into() } else { "MIT (OpenCV Zoo)".into() }),
    });
    lock.save(&lock_path).tool_err()?;
    Ok(Output::ok(
        json!({"ok": true, "tool": name, "path": path, "sha256": sha}),
        format!("installed {name} → {}", path.display()),
    ))
}

pub fn install_face_model() -> CmdResult {
    let file = "face_detection_yunet_2023mar.onnx";
    let url = std::env::var("CUTLINE_FACE_MODEL_URL").unwrap_or_else(|_| {
        format!("https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/{file}")
    });
    install_model("face", "yunet", &url, file)
}

pub fn status() -> CmdResult {
    let lock_path = default_lockfile_path();
    let lock = Lockfile::load(&lock_path).tool_err()?;
    let plat = platform_id();
    let active = cutline_deps::tools::discover().ok();
    let cur = install::current_ffmpeg_version();
    let mut rows = vec![];
    let mut human = vec![format!("lockfile: {}", lock_path.display()), format!("platform: {plat}")];
    for e in lock.tools.iter().filter(|e| e.platform == plat || e.platform == "any") {
        let installed = match e.name.as_str() {
            "ffmpeg" | "ffprobe" | "ffmpeg+ffprobe" => cur.as_deref() == Some(&install::sanitize(&e.version)),
            _ => tools_dir().join("models").join(&e.version).exists(),
        };
        human.push(format!(
            "  {:<16} {:<24} {}",
            e.name,
            e.version,
            if installed { "installed" } else { "NOT installed" }
        ));
        rows.push(json!({"name": e.name, "locked_version": e.version, "installed": installed, "sha256": e.sha256, "selftest": e.selftest}));
    }
    if let Some(a) = &active {
        human.push(format!("active ffmpeg: {} ({:?}) {}", a.version, a.source, a.ffmpeg.display()));
    }
    Ok(Output::ok(
        json!({"lockfile": lock_path, "platform": plat, "tools": rows, "active_ffmpeg": active, "tools_dir": tools_dir()}),
        human.join("\n"),
    ))
}

/// Resolve latest → stage → `selftest --quick` with staged tools → promote (or keep current).
pub fn update(check_only: bool) -> CmdResult {
    let latest = install::resolve_latest_ffmpeg().tool_err()?;
    let cur = install::current_ffmpeg_version();
    let newer = cur.as_deref() != Some(&install::sanitize(&latest.version));
    if check_only || !newer {
        return Ok(Output::ok(
            json!({"current": cur, "latest": latest.version, "update_available": newer, "updated": false}),
            if newer {
                format!("update available: {} → {}", cur.unwrap_or_default(), latest.version)
            } else {
                format!("up to date ({})", latest.version)
            },
        ));
    }
    let dir = install::stage_ffmpeg(&latest).tool_err()?;
    let st = run_selftest_with(&dir);
    let lock_path = default_lockfile_path();
    let mut lock = Lockfile::load(&lock_path).tool_err()?;
    match st {
        Ok(()) => {
            install::promote_ffmpeg(&latest.version).tool_err()?;
            install::record(&mut lock, &latest, Some("pass"));
            lock.save(&lock_path).tool_err()?;
            Ok(Output::ok(
                json!({"current": latest.version, "previous": cur, "updated": true, "selftest": "pass"}),
                format!("updated ffmpeg {} → {} (selftest passed)", cur.unwrap_or_default(), latest.version),
            ))
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Ok(Output::ok(
                json!({"current": cur, "rejected": latest.version, "updated": false, "selftest": "fail", "reason": format!("{e:#}")}),
                format!("kept ffmpeg {}: staged {} failed selftest: {e:#}", cur.unwrap_or_default(), latest.version),
            )
            .with_code(EXIT_TOOL))
        }
    }
}

fn run_selftest_with(dir: &std::path::Path) -> anyhow::Result<()> {
    if let Ok(cmd) = std::env::var("CUTLINE_UPDATE_SELFTEST_CMD") {
        // test hook: run an arbitrary command instead of the real selftest
        let st = std::process::Command::new(cmd).env("CUTLINE_FFMPEG", dir.join(exe_name("ffmpeg"))).status()?;
        anyhow::ensure!(st.success(), "selftest command failed");
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let st = std::process::Command::new(exe)
        .args(["selftest", "--quick"])
        .env("CUTLINE_FFMPEG", dir.join(exe_name("ffmpeg")))
        .env("CUTLINE_FFPROBE", dir.join(exe_name("ffprobe")))
        .status()?;
    anyhow::ensure!(st.success(), "selftest --quick failed with staged tools");
    Ok(())
}

pub fn rollback() -> CmdResult {
    let v = install::rollback_ffmpeg().tool_err()?;
    Ok(Output::ok(json!({"ok": true, "current": v}), format!("rolled back to ffmpeg {v}")))
}

#[allow(dead_code)]
pub fn describe(r: &Release) -> String {
    format!("{} ({})", r.version, r.source)
}
