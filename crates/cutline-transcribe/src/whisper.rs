//! whisper.cpp via its CLI (`whisper-cli`). Word-level timestamps come from `-ml 1 -sow`
//! (one segment per word, token timestamps) plus DTW alignment for known models.
//!
//! The in-process `whisper-rs` binding is not compiled in this build (see docs/decisions.md);
//! the sidecar is the supported path on every platform.

use crate::model::{Transcript, Word};
use anyhow::{bail, Context, Result};
use cutline_deps::paths::{exe_name, tools_dir};
use cutline_deps::Toolchain;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_MODEL: &str = "large-v3-turbo-q5_0";
pub const FAST_MODEL: &str = "base.en";

/// Locate the whisper.cpp CLI: `CUTLINE_WHISPER_CLI`, `<tools>/whisper/whisper-cli`, PATH.
pub fn find_cli() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUTLINE_WHISPER_CLI") {
        return Some(PathBuf::from(p));
    }
    let t = tools_dir().join("whisper").join(exe_name("whisper-cli"));
    if t.exists() {
        return Some(t);
    }
    for n in ["whisper-cli", "whisper-cpp", "whisper.cpp"] {
        if let Ok(p) = which_in_path(n) {
            return Some(p);
        }
    }
    None
}

fn which_in_path(n: &str) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").context("no PATH")?;
    std::env::split_paths(&path).map(|d| d.join(exe_name(n))).find(|p| p.is_file()).context("not found")
}

pub fn model_path(name: &str) -> PathBuf {
    tools_dir().join("models").join(format!("ggml-{name}.bin"))
}

/// DTW alignment preset name for whisper.cpp (`-dtw`), when the model is a known one.
fn dtw_preset(model: &str) -> Option<&'static str> {
    let base = model.split("-q").next().unwrap_or(model);
    Some(match base {
        "tiny" => "tiny",
        "tiny.en" => "tiny.en",
        "base" => "base",
        "base.en" => "base.en",
        "small" => "small",
        "small.en" => "small.en",
        "medium" => "medium",
        "medium.en" => "medium.en",
        "large-v3" => "large.v3",
        "large-v3-turbo" => "large.v3.turbo",
        _ => return None,
    })
}

pub fn available(model: &str) -> Result<(PathBuf, PathBuf)> {
    let cli = find_cli().context("whisper-cli not found (install whisper.cpp or set CUTLINE_WHISPER_CLI)")?;
    let m = model_path(model);
    if !m.exists() {
        bail!("whisper model {} not installed (run `cutline deps install whisper-model {model}`)", m.display());
    }
    Ok((cli, m))
}

/// Transcribe a media file with word timestamps.
pub fn transcribe(tc: &Toolchain, media: &Path, model: &str, lang: Option<&str>, work: &Path) -> Result<Transcript> {
    let (cli, mpath) = available(model)?;
    std::fs::create_dir_all(work)?;
    let wav = work.join("whisper_in.wav");
    let st = Command::new(&tc.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-i"])
        .arg(media)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(&wav)
        .status()?;
    anyhow::ensure!(st.success(), "audio extraction for whisper failed");
    let out_base = work.join("whisper_out");
    let mut c = Command::new(&cli);
    c.arg("-m").arg(&mpath).arg("-f").arg(&wav).args(["-ml", "1", "-sow", "-ojf", "-np"]).arg("-of").arg(&out_base);
    c.args(["-l", lang.unwrap_or("auto")]);
    if let Some(p) = dtw_preset(model) {
        c.args(["-dtw", p]);
    }
    let o = c.output().context("running whisper-cli")?;
    if !o.status.success() {
        bail!(
            "whisper-cli failed: {}",
            String::from_utf8_lossy(&o.stderr).lines().rev().take(5).collect::<Vec<_>>().join(" | ")
        );
    }
    let json = std::fs::read_to_string(out_base.with_extension("json")).context("whisper JSON output")?;
    let mut t = parse_whisper_json(&json)?;
    t.model = model.into();
    Ok(t)
}

/// Parse whisper.cpp `--output-json-full` (or `-oj`) output into words.
pub fn parse_whisper_json(s: &str) -> Result<Transcript> {
    let v: Value = serde_json::from_str(s)?;
    let lang = v["result"]["language"].as_str().unwrap_or("").to_string();
    let segs = v["transcription"].as_array().context("no `transcription` array")?;
    let mut words: Vec<Word> = vec![];
    for seg in segs {
        let text = seg["text"].as_str().unwrap_or("").to_string();
        let from = seg["offsets"]["from"].as_f64().unwrap_or(0.0) / 1000.0;
        let to = seg["offsets"]["to"].as_f64().unwrap_or(0.0) / 1000.0;
        let toks = seg["tokens"].as_array().cloned().unwrap_or_default();
        let probs: Vec<f64> = toks
            .iter()
            .filter(|t| !t["text"].as_str().unwrap_or("").starts_with("[_"))
            .filter_map(|t| t["p"].as_f64())
            .collect();
        let conf = if probs.is_empty() { 1.0 } else { probs.iter().sum::<f64>() / probs.len() as f64 };
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with('[') || trimmed.starts_with('(') {
            continue;
        }
        // with -ml 1 -sow each segment is one word, but continuation pieces (no leading space)
        // belong to the previous word
        if !text.starts_with(' ')
            && !words.is_empty()
            && !trimmed.chars().next().map(|c| c.is_alphanumeric()).unwrap_or(true)
        {
            let lw = words.last_mut().unwrap();
            lw.w.push_str(trimmed);
            lw.end = lw.end.max(to);
            continue;
        }
        for (k, piece) in trimmed.split_whitespace().enumerate() {
            let n = trimmed.split_whitespace().count() as f64;
            let a = from + (to - from) * k as f64 / n;
            let b = from + (to - from) * (k + 1) as f64 / n;
            words.push(Word { w: piece.to_string(), start: a, end: b.max(a), conf, speaker: None });
        }
    }
    let mut t = Transcript { asset: String::new(), lang, model: String::new(), words, segments: vec![] };
    t.rebuild_segments();
    Ok(t)
}

/// Synthesize speech for tests: macOS `say`, Windows SAPI, or espeak-ng. Returns false when no
/// engine exists.
pub fn synthesize(text: &str, out_wav: &Path, tc: &Toolchain) -> Result<bool> {
    let tmp = out_wav.with_extension("tts.aiff");
    if which_in_path("say").is_ok() {
        let st = Command::new("say").arg("-o").arg(&tmp).arg(text).status()?;
        if st.success() {
            let st = Command::new(&tc.ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                .arg(&tmp)
                .args(["-ar", "48000"])
                .arg(out_wav)
                .status()?;
            let _ = std::fs::remove_file(&tmp);
            return Ok(st.success());
        }
    }
    if cfg!(windows) {
        let script = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.SetOutputToWaveFile('{}'); $s.Speak('{}'); $s.Dispose()",
            out_wav.display(),
            text.replace('\'', "''")
        );
        let st = Command::new("powershell").args(["-NoProfile", "-Command", &script]).status();
        return Ok(st.map(|s| s.success()).unwrap_or(false));
    }
    for e in ["espeak-ng", "espeak"] {
        if which_in_path(e).is_ok() {
            let st = Command::new(e).arg("-w").arg(out_wav).arg(text).status()?;
            return Ok(st.success());
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_full_json() {
        let s = r#"{"result": {"language": "en"}, "transcription": [
            {"offsets": {"from": 0, "to": 320}, "text": " Your", "tokens": [{"text": " Your", "p": 0.9}]},
            {"offsets": {"from": 320, "to": 700}, "text": " knife", "tokens": [{"text": " knife", "p": 0.8}]},
            {"offsets": {"from": 700, "to": 760}, "text": ",", "tokens": [{"text": ",", "p": 0.9}]},
            {"offsets": {"from": 800, "to": 900}, "text": " [BLANK_AUDIO]", "tokens": []},
            {"offsets": {"from": 900, "to": 1300}, "text": " sharp.", "tokens": [{"text": " sharp", "p": 0.7}, {"text": ".", "p": 0.9}]}
        ]}"#;
        let t = parse_whisper_json(s).unwrap();
        assert_eq!(t.lang, "en");
        assert_eq!(t.text(), "Your knife, sharp.");
        assert_eq!(t.words[1].end, 0.76);
        assert!((t.words[0].conf - 0.9).abs() < 1e-9);
    }
}
