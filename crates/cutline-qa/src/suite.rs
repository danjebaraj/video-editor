//! The concrete selftest checks.

use crate::selftest::{Outcome, Report, Runner};
use anyhow::{Context, Result};
use cutline_testkit::analyze::read_all_frames;
use cutline_testkit::gen::{self, AlphaKind, FrameIdSource};
use cutline_testkit::{decode_frame_id, FrameIdLayout};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Quick,
    Full,
}

pub fn run_selftest(mode: Mode, workdir: PathBuf) -> Report {
    let mut r = Runner::new(workdir);
    r.run("doctor/capabilities", check_capabilities);
    r.run("testkit/frameid_roundtrip", check_frameid);
    r.run("probe/alpha", check_probe_alpha);
    let _ = mode;
    r.finish(if mode == Mode::Quick { "quick" } else { "full" })
}

fn check_capabilities(_: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    let caps = cutline_deps::tools::capabilities(tc)?;
    let checks = cutline_deps::tools::check_capabilities(&caps);
    let missing: Vec<String> = checks
        .iter()
        .filter(|c| c.required && !c.present)
        .map(|c| format!("{}:{}", c.kind, c.name))
        .collect();
    let fonts = cutline_deps::paths::assets_dir().join("fonts");
    let nfonts = std::fs::read_dir(&fonts).map(|r| r.count()).unwrap_or(0);
    Ok(Outcome::check(
        missing.is_empty() && nfonts > 0,
        json!({"missing": missing, "fonts": nfonts, "ffmpeg": tc.version}),
        json!({"missing": []}),
    ))
}

fn check_frameid(d: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    let src = d.join("src.mp4");
    let mut s = FrameIdSource::new(1280, 720, 30, 2.0);
    s.tone_hz = None;
    gen::frameid_video(tc, &src, &s)?;
    let out = d.join("scaled.mp4");
    let mut c = tc.ffmpeg_cmd();
    c.arg("-i")
        .arg(&src)
        .args([
            "-vf",
            "scale=1080:1920,setsar=1",
            "-c:v",
            "libx264",
            "-crf",
            "18",
            "-preset",
            "veryfast",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&out);
    gen::run(c)?;
    let l = FrameIdLayout::top_row().transform(1080.0 / 1280.0, 1920.0 / 720.0, 0.0, 0.0);
    let (_, _, bw, bh) = l.bbox();
    let frames = read_all_frames(tc, &out, Some((0, 0, bw.ceil() as u32, bh.ceil() as u32)))?;
    let ok = frames
        .iter()
        .enumerate()
        .filter(|(i, f)| decode_frame_id(f, &l) == Some(*i as u16))
        .count();
    Ok(Outcome::check(
        ok == frames.len() && ok == 60,
        json!({"decoded": ok, "frames": frames.len()}),
        json!({"decoded": 60}),
    ))
}

fn check_probe_alpha(d: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    let p = d.join("a.mov");
    gen::alpha_overlay(tc, &p, AlphaKind::ProRes4444, 160, 120, 30, 1.0, 200)?;
    let w = d.join("a.webm");
    gen::alpha_overlay(tc, &w, AlphaKind::Vp9Alpha, 160, 120, 30, 1.0, 200)?;
    let a = cutline_media::probe(tc, &p, None)?
        .video
        .context("no video")?
        .alpha;
    let b = cutline_media::probe(tc, &w, None)?
        .video
        .context("no video")?
        .alpha;
    Ok(Outcome::check(
        a && b,
        json!({"prores4444": a, "vp9": b}),
        json!({"prores4444": true, "vp9": true}),
    ))
}
