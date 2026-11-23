//! Stand-in media for dry runs. A dry run records and generates nothing, so the edit Claude
//! writes points at recordings and audio that don't exist yet, and cutline stops at the missing
//! files before it checks timing, captions or layout. `video-studio standin …` writes a plain
//! clip or a quiet sound of the planned length in their place, so the rest of the edit can be
//! validated and reviewed. Stand-ins exist only in a dry run's scratch folder.

use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};

use crate::capture::CAPTURES_DIR;
use crate::claude::Mode;
use crate::eleven::HelperContext;
use crate::{Error, Result};

/// Set in tests: stand-ins are written as small text files instead of being encoded.
pub const FAKE_ENV: &str = "VIDEO_STUDIO_STANDIN_FAKE";
pub const AUDIO_DIR: &str = "audio";
pub const MAX_SECONDS: f64 = 1800.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// A flat-colored clip of this size in pixels, in place of a recording.
    Video { width: u32, height: u32 },
    /// A quiet noise bed, in place of voice-over, music or a sound effect.
    Audio,
}

/// Where a stand-in may be written: `captures/…` for video and `audio/…` for audio, inside the
/// run folder, with an extension the encoder knows.
fn target(ctx: &HelperContext, kind: Kind, out: &str) -> Result<Utf8PathBuf> {
    let (dir, exts, default): (&str, &[&str], &str) = match kind {
        Kind::Video { .. } => (CAPTURES_DIR, &["mov", "mp4"], "mov"),
        Kind::Audio => (AUDIO_DIR, &["mp3", "wav", "m4a"], "mp3"),
    };
    let rel = Utf8Path::new(out.trim());
    let rel = rel.strip_prefix(dir).unwrap_or(rel);
    let plain = rel.components().all(|c| matches!(c, camino::Utf8Component::Normal(_)));
    if rel.as_str().is_empty() || !plain {
        return Err(Error::Denied(format!("--out must be a file name under {dir}/, such as {dir}/scene-2.{default}.")));
    }
    let mut path = ctx.run_folder.join(dir).join(rel);
    match path.extension().map(str::to_ascii_lowercase) {
        None => {
            path.set_extension(default);
        }
        Some(e) if exts.contains(&e.as_str()) => {}
        Some(e) => {
            return Err(Error::other(format!("A stand-in under {dir}/ can't be .{e}; use {}.", exts.join(", "))));
        }
    }
    Ok(path)
}

/// A path in the run folder as Claude should write it: relative, with forward slashes (a
/// backslash would be an escape in project.json5 and in Git Bash on Windows).
fn rel(ctx: &HelperContext, p: &Utf8Path) -> String {
    p.strip_prefix(&ctx.run_folder).unwrap_or(p).as_str().replace('\\', "/")
}

/// A muted color that differs from file to file, so cuts show up in the review pack.
fn color_for(name: &str) -> String {
    let h = name.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let ch = |shift: u32| 0x30 + ((h >> shift) & 0x3F);
    format!("0x{:02X}{:02X}{:02X}", ch(0), ch(6), ch(12))
}

fn ffmpeg(ctx: &HelperContext) -> Result<String> {
    let cutline =
        ctx.cutline.as_ref().ok_or_else(|| Error::Environment("cutline isn't set up for this run.".into()))?;
    let out = Command::new(cutline.as_std_path())
        .args(["deps", "path", "ffmpeg"])
        .output()
        .map_err(|e| Error::Environment(format!("couldn't ask cutline for ffmpeg: {e}")))?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || path.is_empty() {
        return Err(Error::Environment("cutline has no ffmpeg installed, so stand-ins can't be made.".into()));
    }
    Ok(path)
}

/// Writes a stand-in and returns a line for Claude.
pub fn create(ctx: &HelperContext, kind: Kind, out: &str, seconds: f64) -> Result<String> {
    if ctx.mode != Mode::DryRun {
        return Err(Error::Denied(
            "Stand-ins are only for dry runs. In a production run, record or generate the real file.".into(),
        ));
    }
    if !(seconds.is_finite() && seconds > 0.0 && seconds <= MAX_SECONDS) {
        return Err(Error::other(format!("--seconds must be between 0 and {MAX_SECONDS:.0}.")));
    }
    if let Kind::Video { width, height } = kind {
        if !(16..=7680).contains(&width) || !(16..=7680).contains(&height) || width % 2 == 1 || height % 2 == 1 {
            return Err(Error::other("--size must be even numbers of pixels, such as 1280x800."));
        }
    }
    let path = target(ctx, kind, out)?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let rel = rel(ctx, &path);
    if std::env::var(FAKE_ENV).is_ok_and(|v| !v.is_empty()) {
        std::fs::write(&path, format!("stand-in {kind:?} {seconds}\n"))?;
    } else {
        let d = format!("{seconds:.3}");
        let mut cmd = Command::new(ffmpeg(ctx)?);
        cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"]);
        match kind {
            Kind::Video { width, height } => {
                // Grain keeps the bitrate near a real recording's, so cutline's quality checks
                // don't warn about the stand-in itself.
                cmd.arg(format!("color=c={}:s={width}x{height}:r=30:d={d},noise=alls=24:allf=t+u", color_for(&rel)));
                cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "26", "-pix_fmt", "yuv420p"]);
            }
            Kind::Audio => {
                cmd.arg(format!("anoisesrc=color=pink:amplitude=0.05:r=44100:d={d}"));
                cmd.args(["-ac", "2"]);
            }
        }
        let res = cmd
            .arg(path.as_std_path())
            .output()
            .map_err(|e| Error::Environment(format!("ffmpeg didn't start: {e}")))?;
        if !res.status.success() || !path.is_file() {
            let _ = std::fs::remove_file(&path);
            let why = String::from_utf8_lossy(&res.stderr);
            return Err(Error::other(format!(
                "The stand-in couldn't be made: {}",
                why.lines().last().unwrap_or("ffmpeg failed").trim()
            )));
        }
    }
    Ok(format!(
        "Stand-in written: {rel} ({seconds:.1} s{}). It only holds the place of the real file; the production run makes that.",
        match kind {
            Kind::Video { width, height } => format!(", {width}×{height}, a plain color"),
            Kind::Audio => ", quiet noise".to_string(),
        }
    ))
}

/// Words per minute used to time a stand-in voice-over.
pub const WORDS_PER_MINUTE: f64 = 150.0;

/// Word timings for `script` spread over `seconds`, in the ElevenLabs Scribe shape cutline
/// imports. Longer words take longer and a sentence end is followed by a short pause, which is
/// close enough to speech for captions and cues to land in plausible places.
pub fn transcript(script: &str, seconds: f64) -> serde_json::Value {
    let spoken = crate::eleven::tags::spoken(script);
    let words: Vec<&str> = spoken.split_whitespace().collect();
    let weight = |w: &str| w.chars().count() as f64 + 2.0;
    let pause = |w: &str| if w.ends_with(['.', '!', '?', ':']) { 5.0 } else { 0.0 };
    let total: f64 = words.iter().map(|w| weight(w) + pause(w)).sum::<f64>().max(1.0);
    // Speech starts a moment in and ends a moment before the file does.
    let lead = (seconds * 0.02).min(0.25);
    let scale = (seconds - 2.0 * lead).max(0.1) / total;
    let mut at = lead;
    let mut out = Vec::with_capacity(words.len());
    for w in &words {
        let end = at + weight(w) * scale * 0.9;
        out.push(serde_json::json!({
            "text": w, "type": "word",
            "start": (at * 1000.0).round() / 1000.0, "end": (end * 1000.0).round() / 1000.0,
        }));
        at += (weight(w) + pause(w)) * scale;
    }
    serde_json::json!({ "language_code": "en", "text": words.join(" "), "words": out, "stand_in": true })
}

/// A stand-in for one voice-over: the quiet audio, plus a transcript made from the script so
/// the dry run can check captions and cue timing. Without `seconds`, the length is what the
/// words take to speak.
pub fn create_voice(ctx: &HelperContext, out: &str, script: &str, seconds: Option<f64>) -> Result<String> {
    let count = crate::eleven::tags::spoken(script).split_whitespace().count();
    if count == 0 {
        return Err(Error::other("The voice-over text is empty."));
    }
    let seconds = seconds.unwrap_or_else(|| (count as f64 / WORDS_PER_MINUTE * 60.0).max(1.0));
    let line = create(ctx, Kind::Audio, out, seconds)?;
    let audio = target(ctx, Kind::Audio, out)?;
    let json = audio.with_extension("json");
    crate::paths::write_json_atomic(&json, &transcript(script, seconds))?;
    let json_rel = rel(ctx, &json);
    Ok(format!(
        "{line}\nStand-in transcript written: {json_rel} ({count} words spread over {seconds:.1} s). Give the asset `transcript: \"{json_rel}\"` in project.json5 so captions and cues can be checked; the production run replaces it with the real one from `video-studio eleven scribe`."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &Utf8Path, mode: Mode) -> HelperContext {
        HelperContext {
            run_id: "run_1".into(),
            run_folder: dir.to_path_buf(),
            mode,
            logs_dir: dir.join("logs"),
            cutline: None,
            credit_cap: 0,
            cache_dir: dir.join("cache"),
            cache_limit_bytes: 0,
            default_voice: None,
            default_voice_settings: None,
            default_model: None,
            voice_preference: Default::default(),
            voice_gender: Default::default(),
            voice_provider: Default::default(),
            cartesia_base_url: None,
            cartesia_test_key: None,
            base_url: None,
            test_api_key: None,
            capture: None,
            relay_dir: None,
        }
    }

    #[test]
    fn stand_ins_stay_in_their_folders_and_in_dry_runs() {
        let t = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(t.path()).unwrap();
        let dry = ctx(dir, Mode::DryRun);
        let video = Kind::Video { width: 1280, height: 800 };
        assert_eq!(target(&dry, video, "captures/scene-2.mov").unwrap(), dir.join("captures/scene-2.mov"));
        assert_eq!(target(&dry, video, "scene-2").unwrap(), dir.join("captures/scene-2.mov"));
        assert_eq!(target(&dry, Kind::Audio, "audio/vo/one.wav").unwrap(), dir.join("audio/vo/one.wav"));
        for bad in ["../x.mov", "/tmp/x.mov", "captures/../../x.mov", ""] {
            assert!(target(&dry, video, bad).is_err(), "{bad}");
        }
        assert!(target(&dry, Kind::Audio, "audio/x.mov").is_err());
        assert!(create(&dry, video, "captures/a.mov", 0.0).is_err());
        assert!(create(&dry, Kind::Video { width: 1281, height: 800 }, "captures/a.mov", 5.0).is_err());
        let e = create(&ctx(dir, Mode::Production), video, "captures/a.mov", 5.0).unwrap_err();
        assert!(e.to_string().contains("only for dry runs"));
        assert_ne!(color_for("captures/a.mov"), color_for("captures/b.mov"));
    }

    #[test]
    fn a_stand_in_voice_over_gets_a_transcript_from_its_script() {
        let script = "Meet the fastest way to ship. [pause] It takes one click.";
        let t = transcript(script, 4.0);
        let words = t["words"].as_array().unwrap();
        assert_eq!(words.len(), 10, "audio tags are not spoken");
        let (first, last) = (&words[0], &words[9]);
        assert_eq!(first["text"], "Meet");
        assert!(first["start"].as_f64().unwrap() > 0.0 && last["end"].as_f64().unwrap() <= 4.0);
        let mut prev = 0.0;
        for w in words {
            let (s, e) = (w["start"].as_f64().unwrap(), w["end"].as_f64().unwrap());
            assert!(s >= prev && e > s, "{w}");
            prev = e;
        }
        // the gap after a sentence end is longer than between ordinary words
        let gap = |i: usize| words[i + 1]["start"].as_f64().unwrap() - words[i]["end"].as_f64().unwrap();
        assert!(gap(5) > gap(1) * 2.0, "{} vs {}", gap(5), gap(1));

        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        std::env::set_var(FAKE_ENV, "1");
        let line = create_voice(&ctx(dir, Mode::DryRun), "audio/vo-1.mp3", script, None).unwrap();
        std::env::remove_var(FAKE_ENV);
        assert!(line.contains("audio/vo-1.json") && line.contains("10 words spread over 4.0 s"), "{line}");
        assert!(dir.join("audio/vo-1.mp3").is_file());
        let saved: serde_json::Value = crate::paths::read_json(&dir.join("audio/vo-1.json")).unwrap();
        assert_eq!(saved["stand_in"], true);
        assert!(create_voice(&ctx(dir, Mode::DryRun), "audio/vo-2.mp3", "  ", None).is_err());
    }
}
