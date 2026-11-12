//! An ffmpeg invocation plus the filter scripts it needs. Never goes through a shell.

use anyhow::{bail, Context, Result};
use cutline_deps::Toolchain;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub label: String,
    /// Arguments after the ffmpeg binary.
    pub args: Vec<String>,
    /// Filter scripts to write before running: (path, content).
    pub scripts: Vec<(PathBuf, String)>,
    /// Binary helper files (masks, frames) to write before running.
    #[serde(skip)]
    pub files: Vec<(PathBuf, Vec<u8>)>,
    pub outputs: Vec<PathBuf>,
}

impl Job {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), args: vec![], scripts: vec![], files: vec![], outputs: vec![] }
    }
    pub fn arg(&mut self, a: impl Into<String>) -> &mut Self {
        self.args.push(a.into());
        self
    }
    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(&mut self, a: I) -> &mut Self {
        self.args.extend(a.into_iter().map(Into::into));
        self
    }

    /// Human-readable command line (for --dry-run / --verbose). Quotes args with spaces.
    pub fn display(&self, ffmpeg: &str) -> String {
        let mut s = String::from(ffmpeg);
        for a in &self.args {
            s.push(' ');
            if a.is_empty() || a.chars().any(|c| c.is_whitespace() || "'\"$`\\;&|<>()[]{}*?!".contains(c)) {
                s.push('\'');
                s.push_str(&a.replace('\'', "'\\''"));
                s.push('\'');
            } else {
                s.push_str(a);
            }
        }
        s
    }

    pub fn write_scripts(&self) -> Result<()> {
        for (p, c) in &self.scripts {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(p, c).with_context(|| format!("writing {}", p.display()))?;
        }
        for (p, c) in &self.files {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(p, c).with_context(|| format!("writing {}", p.display()))?;
        }
        Ok(())
    }

    /// Run with ffmpeg; returns stderr (useful for measurement filters).
    pub fn run(&self, tc: &Toolchain) -> Result<String> {
        self.write_scripts()?;
        for o in &self.outputs {
            if let Some(d) = o.parent() {
                std::fs::create_dir_all(d)?;
            }
        }
        tracing::debug!(cmd = %self.display(&tc.ffmpeg.to_string_lossy()), "ffmpeg");
        let out = Command::new(&tc.ffmpeg)
            .args(&self.args)
            .output()
            .with_context(|| format!("spawning ffmpeg for {}", self.label))?;
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        if !out.status.success() {
            let tail: Vec<&str> = err.lines().rev().take(25).collect::<Vec<_>>().into_iter().rev().collect();
            bail!(
                "ffmpeg failed ({}):\n{}\ncommand: {}",
                self.label,
                tail.join("\n"),
                self.display(&tc.ffmpeg.to_string_lossy())
            );
        }
        Ok(err)
    }
}

/// Standard global args.
pub fn base_args(verbose_stats: bool) -> Vec<String> {
    let mut v: Vec<String> = ["-hide_banner", "-nostdin", "-y"].iter().map(|s| s.to_string()).collect();
    if !verbose_stats {
        v.extend(["-loglevel", "error"].iter().map(|s| s.to_string()));
    }
    v
}
