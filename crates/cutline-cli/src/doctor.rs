use crate::out::*;
use cutline_deps::paths;
use cutline_deps::tools::{capabilities, check_capabilities};
use serde_json::json;

pub fn run() -> CmdResult {
    let mut fixes: Vec<String> = vec![];
    let mut errors = 0;
    let mut warnings = 0;
    let mut lines = vec![];

    let tc = cutline_deps::tools::discover();
    let (tool_json, caps_json) = match &tc {
        Ok(tc) => {
            lines.push(format!("✔ ffmpeg {} ({:?}) at {}", tc.version, tc.source, tc.ffmpeg.display()));
            if cutline_deps::tools::parse_version(&tc.version) < (6, 0) {
                warnings += 1;
                lines.push("⚠ ffmpeg < 6.0 is untested".into());
                fixes.push("cutline deps install ffmpeg".into());
            }
            let caps = capabilities(tc).map_err(CliError::tool)?;
            let checks = check_capabilities(&caps);
            for c in &checks {
                if !c.present {
                    if c.required {
                        errors += 1;
                        lines.push(format!("✘ missing required {} `{}`", c.kind, c.name));
                    } else {
                        lines.push(format!("· optional {} `{}` not available", c.kind, c.name));
                    }
                }
            }
            if checks.iter().any(|c| c.required && !c.present) {
                fixes
                    .push("cutline deps install ffmpeg   # the static build includes every required capability".into());
            } else {
                lines.push(format!(
                    "✔ all {} required filters/encoders/decoders present",
                    checks.iter().filter(|c| c.required).count()
                ));
            }
            (json!(tc), json!(checks))
        }
        Err(e) => {
            errors += 1;
            lines.push(format!("✘ ffmpeg: {e:#}"));
            fixes.push("cutline deps install ffmpeg".into());
            (json!(null), json!([]))
        }
    };

    // fonts
    let fonts_dir = paths::assets_dir().join("fonts");
    let fonts: Vec<String> = std::fs::read_dir(&fonts_dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| n.ends_with(".ttf") || n.ends_with(".otf"))
                .collect()
        })
        .unwrap_or_default();
    if fonts.is_empty() {
        errors += 1;
        lines.push(format!("✘ no bundled fonts in {}", fonts_dir.display()));
        fixes.push("set CUTLINE_ASSETS to the cutline assets directory".into());
    } else {
        lines.push(format!("✔ {} bundled fonts", fonts.len()));
    }

    // chrome (optional: HTML layers)
    let chrome = paths::find_chrome();
    match &chrome {
        Some(p) => lines.push(format!("✔ chrome: {}", p.display())),
        None => {
            warnings += 1;
            lines.push("⚠ no Chrome/Edge found (HTML/Lottie layers unavailable)".into());
            fixes.push("cutline deps install chrome".into());
        }
    }

    // whisper / face model (optional)
    let models = paths::tools_dir().join("models");
    let whisper: Vec<String> = std::fs::read_dir(&models)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| n.starts_with("ggml-"))
                .collect()
        })
        .unwrap_or_default();
    let whisper_cli = which_any(&["whisper-cli", "whisper-cpp", "main"]);
    if whisper.is_empty() {
        warnings += 1;
        lines.push("⚠ no whisper model installed (transcription unavailable; Scribe import still works)".into());
        fixes.push("cutline deps install whisper-model base.en".into());
    } else {
        lines.push(format!("✔ whisper models: {}", whisper.join(", ")));
    }
    let face = models.join("face_detection_yunet_2023mar.onnx").exists();
    let det = cutline_reframe_status();
    if !face {
        lines.push("· face model not installed (track_face falls back to fit_blur)".into());
        fixes.push("cutline deps install face-model".into());
    } else {
        lines.push(format!("✔ face model installed; detector: {det}"));
    }

    let free = fs2::available_space(paths::data_dir().parent().unwrap_or(std::path::Path::new("."))).ok();
    if let Some(f) = free {
        let gb = f as f64 / 1e9;
        if gb < 5.0 {
            warnings += 1;
            lines.push(format!("⚠ low disk space: {gb:.1} GB free"));
        } else {
            lines.push(format!("✔ disk: {gb:.1} GB free"));
        }
    }

    let ok = errors == 0;
    if !fixes.is_empty() {
        lines.push("\nFix-it list:".into());
        for f in &fixes {
            lines.push(format!("  {f}"));
        }
    }
    let (home_dir, src) = paths::home();
    let home_src = match src {
        paths::HomeSource::Env => "CUTLINE_HOME",
        paths::HomeSource::WorkingDir => ".cutline-home above the working directory",
        paths::HomeSource::Binary => ".cutline-home above the cutline binary",
        paths::HomeSource::Platform => "platform data directory",
    };
    lines.insert(0, format!("· home: {} ({home_src})", home_dir.display()));
    let j = json!({
        "ok": ok,
        "errors": errors,
        "warnings": warnings,
        "platform": paths::platform_id(),
        "ffmpeg": tool_json,
        "capabilities": caps_json,
        "fonts": {"dir": fonts_dir, "files": fonts},
        "chrome": chrome,
        "whisper_models": whisper,
        "whisper_cli": whisper_cli,
        "face_model": face,
        "face_detector": det,
        "home": home_dir,
        "home_source": home_src,
        "tools_dir": paths::tools_dir(),
        "cache_dir": paths::cache_dir(),
        "disk_free_bytes": free,
        "fixes": fixes,
    });
    Ok(Output::ok(j, lines.join("\n")).with_code(if ok { EXIT_OK } else { EXIT_TOOL }))
}

fn which_any(names: &[&str]) -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("CUTLINE_WHISPER_CLI") {
        return Some(p.into());
    }
    names.iter().find_map(|n| which_path(n))
}

fn which_path(n: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(paths::exe_name(n))).find(|p| p.is_file())
}

fn cutline_reframe_status() -> &'static str {
    cutline_render::reframe_detector_status()
}
