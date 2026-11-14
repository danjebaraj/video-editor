//! Text recognition for the demo checks, with two engines:
//!
//! * **Vision** (macOS): the system's text recognizer, driven by a small Swift helper that is
//!   compiled once into the cutline home (needs the Xcode command-line tools).
//! * **Tesseract**: the `tesseract` command (`CUTLINE_TESSERACT`, else `PATH`).
//!
//! Which one runs: `review --ocr`, else `CUTLINE_OCR`, else the project's `qa.ocr`, else
//! `auto` (Vision on macOS, Tesseract elsewhere, whichever is present). Nothing is sent
//! anywhere: both run on this machine.

use anyhow::{bail, Context, Result};
use cutline_core::model::OcrEngine;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A recognized line of text; the box is in fractions of the image (top-left origin).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub conf: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Vision,
    Tesseract,
}

impl Engine {
    pub fn name(&self) -> &'static str {
        match self {
            Engine::Vision => "vision",
            Engine::Tesseract => "tesseract",
        }
    }
}

/// The engine setting in effect: an explicit choice, the environment, the project, `auto`.
pub fn setting(cli: Option<&str>, project: Option<OcrEngine>) -> Result<OcrEngine> {
    let parse = |s: &str| -> Result<OcrEngine> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Ok(OcrEngine::Auto),
            "vision" => Ok(OcrEngine::Vision),
            "tesseract" => Ok(OcrEngine::Tesseract),
            "off" | "none" => Ok(OcrEngine::Off),
            other => bail!("unknown OCR engine `{other}` (auto, vision, tesseract, off)"),
        }
    };
    if let Some(c) = cli {
        return parse(c);
    }
    if let Ok(e) = std::env::var("CUTLINE_OCR") {
        return parse(&e);
    }
    Ok(project.unwrap_or_default())
}

pub fn tesseract_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUTLINE_TESSERACT") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let exe = if cfg!(windows) { "tesseract.exe" } else { "tesseract" };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(exe)).find(|p| p.is_file())
}

const VISION_SWIFT: &str = r#"import Foundation
import Vision
import AppKit

// usage: cutline-ocr-vision <image>... ; prints one JSON array of lines per image
for path in CommandLine.arguments.dropFirst() {
    var out: [[String: Any]] = []
    if let img = NSImage(contentsOfFile: path),
       let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) {
        let req = VNRecognizeTextRequest()
        req.recognitionLevel = .accurate
        req.usesLanguageCorrection = false
        try? VNImageRequestHandler(cgImage: cg, options: [:]).perform([req])
        for obs in req.results ?? [] {
            guard let top = obs.topCandidates(1).first else { continue }
            let b = obs.boundingBox  // normalized, origin bottom-left
            out.append(["text": top.string, "x": b.minX, "y": 1 - b.maxY, "w": b.width, "h": b.height,
                        "conf": Double(top.confidence)])
        }
    }
    let data = try! JSONSerialization.data(withJSONObject: out)
    print(String(data: data, encoding: .utf8)!)
}
"#;

/// The compiled Vision helper, built on first use (macOS with `swiftc`).
pub fn vision_helper() -> Result<PathBuf> {
    if !cfg!(target_os = "macos") {
        bail!("the Vision engine needs macOS");
    }
    let dir = cutline_deps::paths::tools_dir().join("ocr");
    let key = cutline_media::hash_key(&[VISION_SWIFT.as_bytes(), b"vision-v1"]);
    let bin = dir.join(format!("cutline-ocr-vision-{}", &key[..12]));
    if bin.is_file() {
        return Ok(bin);
    }
    std::fs::create_dir_all(&dir)?;
    let src = dir.join("cutline-ocr-vision.swift");
    std::fs::write(&src, VISION_SWIFT)?;
    let tmp = dir.join(format!("cutline-ocr-vision-{}.tmp", std::process::id()));
    let o = Command::new("swiftc")
        .args(["-O", "-o"])
        .arg(&tmp)
        .arg(&src)
        .output()
        .context("running swiftc (install the Xcode command-line tools: `xcode-select --install`)")?;
    if !o.status.success() {
        bail!(
            "compiling the Vision helper failed: {}",
            String::from_utf8_lossy(&o.stderr).lines().take(6).collect::<Vec<_>>().join(" | ")
        );
    }
    std::fs::rename(&tmp, &bin)?;
    Ok(bin)
}

/// The engine to use for a setting, and why not when there is none.
pub fn resolve(setting: OcrEngine) -> std::result::Result<Engine, String> {
    let vision = || vision_helper().map(|_| Engine::Vision).map_err(|e| format!("Vision: {e:#}"));
    let tess = || {
        tesseract_binary().map(|_| Engine::Tesseract).ok_or_else(|| {
            "Tesseract: `tesseract` not found (install it, e.g. `brew install tesseract`, or set CUTLINE_TESSERACT)"
                .to_string()
        })
    };
    match setting {
        OcrEngine::Off => Err("text recognition is off (qa.ocr / CUTLINE_OCR)".into()),
        OcrEngine::Vision => vision(),
        OcrEngine::Tesseract => tess(),
        OcrEngine::Auto => {
            if cfg!(target_os = "macos") {
                vision().or_else(|v| tess().map_err(|t| format!("{v}; {t}")))
            } else {
                tess()
            }
        }
    }
}

/// Lines from Tesseract's TSV output.
pub fn parse_tesseract_tsv(tsv: &str) -> Vec<OcrLine> {
    let mut page = (0.0f64, 0.0f64);
    // (block, par, line) → words
    let mut lines: Vec<((i64, i64, i64), Vec<(String, f64, f64, f64, f64, f64)>)> = vec![];
    for row in tsv.lines().skip(1) {
        let c: Vec<&str> = row.split('\t').collect();
        if c.len() < 12 {
            continue;
        }
        let n = |i: usize| c[i].trim().parse::<f64>().unwrap_or(0.0);
        if c[0] == "1" {
            page = (n(8), n(9));
        }
        let text = c[11..].join("\t");
        if c[0] != "5" || text.trim().is_empty() || n(10) < 0.0 {
            continue;
        }
        let key = (n(2) as i64, n(3) as i64, n(4) as i64);
        let word = (text.trim().to_string(), n(6), n(7), n(8), n(9), n(10));
        match lines.last_mut() {
            Some((k, ws)) if *k == key => ws.push(word),
            _ => lines.push((key, vec![word])),
        }
    }
    if page.0 <= 0.0 || page.1 <= 0.0 {
        return vec![];
    }
    lines
        .into_iter()
        .map(|(_, ws)| {
            let x0 = ws.iter().map(|w| w.1).fold(f64::INFINITY, f64::min);
            let y0 = ws.iter().map(|w| w.2).fold(f64::INFINITY, f64::min);
            let x1 = ws.iter().map(|w| w.1 + w.3).fold(0.0, f64::max);
            let y1 = ws.iter().map(|w| w.2 + w.4).fold(0.0, f64::max);
            OcrLine {
                text: ws.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" "),
                x: x0 / page.0,
                y: y0 / page.1,
                w: (x1 - x0) / page.0,
                h: (y1 - y0) / page.1,
                conf: ws.iter().map(|w| w.5).sum::<f64>() / ws.len() as f64 / 100.0,
            }
        })
        .collect()
}

/// Recognize the text in one image.
pub fn recognize(engine: Engine, image: &Path) -> Result<Vec<OcrLine>> {
    match engine {
        Engine::Vision => {
            let o = Command::new(vision_helper()?).arg(image).output().context("running the Vision helper")?;
            if !o.status.success() {
                bail!("Vision helper failed: {}", String::from_utf8_lossy(&o.stderr));
            }
            let line = String::from_utf8_lossy(&o.stdout);
            Ok(serde_json::from_str(line.lines().next().unwrap_or("[]"))
                .context("parsing the Vision helper's output")?)
        }
        Engine::Tesseract => {
            let bin = tesseract_binary().context("tesseract not found")?;
            // sparse text: interface text is scattered, not paragraphs
            let o = Command::new(bin)
                .arg(image)
                .args(["stdout", "--psm", "11", "tsv"])
                .output()
                .context("running tesseract")?;
            if !o.status.success() {
                bail!("tesseract failed: {}", String::from_utf8_lossy(&o.stderr));
            }
            Ok(parse_tesseract_tsv(&String::from_utf8_lossy(&o.stdout)))
        }
    }
}

// ------------------------------------------------------------------ private details

/// What kind of private detail a piece of text looks like, if any.
pub fn private_kind(text: &str) -> Option<(&'static str, String)> {
    for raw in text.split_whitespace() {
        let tok = raw.trim_matches(|c: char| matches!(c, ',' | ';' | '(' | ')' | '<' | '>' | '"' | '\'' | '[' | ']'));
        let lower = tok.to_ascii_lowercase();
        // email: local@domain.tld
        if let Some((local, domain)) = tok.split_once('@') {
            let dom_ok = domain.contains('.')
                && domain
                    .rsplit('.')
                    .next()
                    .is_some_and(|t| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic()))
                && domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
            if !local.is_empty() && dom_ok && !lower.ends_with("@example.com") && !lower.ends_with("@example.org") {
                return Some(("email", tok.to_string()));
            }
        }
        // home directory with a user name
        for prefix in ["/users/", "/home/", "c:\\users\\"] {
            if let Some(i) = lower.find(prefix) {
                let user: String = tok[i + prefix.len()..].chars().take_while(|c| *c != '/' && *c != '\\').collect();
                if user.len() >= 2
                    && !matches!(user.to_ascii_lowercase().as_str(), "shared" | "public" | "you" | "user" | "username")
                {
                    return Some(("home path", tok.to_string()));
                }
            }
        }
        // well-known secret prefixes
        let secret_prefix = [
            "sk-",
            "sk_live_",
            "sk_test_",
            "pk_live_",
            "ghp_",
            "gho_",
            "github_pat_",
            "xoxb-",
            "xoxp-",
            "akia",
            "aiza",
            "glpat-",
        ];
        if secret_prefix.iter().any(|p| lower.starts_with(p)) && tok.len() >= 16 {
            return Some(("access token", tok.to_string()));
        }
        // long random-looking strings: letters and digits mixed, 28+ characters
        let body = tok.trim_end_matches('=');
        if body.len() >= 28
            && body.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/'))
            && body.chars().filter(|c| c.is_ascii_digit()).count() >= 4
            && body.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 8
            && !body.contains('/')
        {
            return Some(("key or token", tok.to_string()));
        }
        // IPv4 (not loopback or documentation ranges)
        let parts: Vec<&str> = tok.split(':').next().unwrap_or("").split('.').collect();
        if parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok()) {
            let a: Vec<u8> = parts.iter().map(|p| p.parse().unwrap()).collect();
            let doc = (a[0] == 127)
                || (a[0] == 0)
                || (a[0] == 192 && a[1] == 0 && a[2] == 2)
                || (a[0] == 203 && a[1] == 0 && a[2] == 113);
            if !doc {
                return Some(("IP address", tok.to_string()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_details_are_recognized() {
        let k = |s: &str| private_kind(s).map(|x| x.0);
        assert_eq!(k("Signed in as ada.lovelace@analytical.io"), Some("email"));
        assert_eq!(k("ada@example.com"), None, "example addresses are fine");
        assert_eq!(k("cd /Users/danielj/projects/app"), Some("home path"));
        assert_eq!(k("~/Library/Application Support"), None);
        assert_eq!(k("token ghp_a8Kd93jdkLLq02mmZp4477xyTq"), Some("access token"));
        assert_eq!(k("Authorization: 9f8a7b6c5d4e3f2a1b0c9d8e7f6a5b4c"), Some("key or token"));
        assert_eq!(k("server 10.2.14.7:8080"), Some("IP address"));
        assert_eq!(k("listening on 127.0.0.1:3000"), None);
        assert_eq!(k("Version 4.2.1 released on 2026-11-14"), None);
        assert_eq!(k("Revenue $128,400 Active teams 1,204"), None);
    }

    #[test]
    fn tesseract_tsv_is_grouped_into_lines() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   1\t1\t0\t0\t0\t0\t0\t0\t1000\t500\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t100\t50\t80\t20\t96.0\tSigned\n\
                   5\t1\t1\t1\t1\t2\t190\t50\t30\t20\t92.0\tin\n\
                   5\t1\t2\t1\t1\t1\t100\t200\t300\t25\t90.0\tada@corp.io\n";
        let l = parse_tesseract_tsv(tsv);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].text, "Signed in");
        assert!(
            (l[0].x - 0.1).abs() < 1e-9 && (l[0].w - 0.12).abs() < 1e-9 && (l[0].h - 0.04).abs() < 1e-9,
            "{:?}",
            l[0]
        );
        assert!((l[0].conf - 0.94).abs() < 1e-9);
        assert_eq!(l[1].text, "ada@corp.io");
    }

    #[test]
    fn the_engine_setting_has_an_order_of_precedence() {
        assert_eq!(setting(Some("tesseract"), Some(OcrEngine::Vision)).unwrap(), OcrEngine::Tesseract);
        assert!(setting(Some("magic"), None).is_err());
        assert_eq!(resolve(OcrEngine::Off).unwrap_err(), "text recognition is off (qa.ocr / CUTLINE_OCR)");
    }
}
