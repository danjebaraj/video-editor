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
    r.run("pipeline/shorts_render", check_shorts_pipeline);
    let _ = mode;
    r.finish(if mode == Mode::Quick { "quick" } else { "full" })
}

fn check_capabilities(_: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    let caps = cutline_deps::tools::capabilities(tc)?;
    let checks = cutline_deps::tools::check_capabilities(&caps);
    let missing: Vec<String> =
        checks.iter().filter(|c| c.required && !c.present).map(|c| format!("{}:{}", c.kind, c.name)).collect();
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
    let ok = frames.iter().enumerate().filter(|(i, f)| decode_frame_id(f, &l) == Some(*i as u16)).count();
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
    let a = cutline_media::probe(tc, &p, None)?.video.context("no video")?.alpha;
    let b = cutline_media::probe(tc, &w, None)?.video.context("no video")?.alpha;
    Ok(Outcome::check(a && b, json!({"prores4444": a, "vp9": b}), json!({"prores4444": true, "vp9": true})))
}

/// Render a small Shorts project and verify exact frame mapping, tones and QA.
fn check_shorts_pipeline(d: &Path) -> Result<Outcome> {
    use cutline_render::pipeline;
    let tc = cutline_deps::toolchain()?;
    crate::fixtures::sources(d)?;
    let pp = d.join("shorts.json5");
    std::fs::write(&pp, SHORTS_PROJECT)?;
    let l = pipeline::load(&pp).map_err(|e| anyhow::anyhow!("{e}"))?;
    anyhow::ensure!(!cutline_core::diag::has_errors(&l.diags), "project invalid: {:?}", l.diags);
    let opts = cutline_render::RenderOptions::default();
    let res = crate::deliver::render_all(&l, &opts, None)?;
    let r = &res[0];
    let pr = pipeline::prepare(&l, None, &pipeline::exports(&l.project)[0].1, false)?;
    let tl = &pr.timeline;
    // frame mapping
    let mut samples = vec![];
    for c in &tl.clips {
        let in_frame = (c.src_in * 30.0).round() as i64;
        for j in [0, c.frames / 2, c.frames - 1] {
            let f = c.start + j;
            if tl.transitions.iter().any(|t| f >= t.start - 1 && f < t.start + t.frames + 1) {
                continue;
            }
            if c.clip.effects.iter().any(|e| {
                matches!(e, cutline_core::model::Effect::PunchIn { .. } | cutline_core::model::Effect::Zoom { .. })
            }) {
                continue; // zoomed frames don't map the pattern 1:1
            }
            samples.push((f as u64, (in_frame + (j as f64 * c.speed).round() as i64) as u16));
        }
    }
    samples.sort();
    samples.dedup_by_key(|s| s.0);
    let idx: Vec<u64> = samples.iter().map(|s| s.0).collect();
    let frames = cutline_testkit::analyze::read_frames(tc, &r.output, &idx)?;
    let lo = crate::fixtures::layout().after_cover(crate::fixtures::SRC_W, crate::fixtures::SRC_H, 1080, 1920);
    let mut bad = vec![];
    for (img, (f, want)) in frames.iter().zip(&samples) {
        let got = decode_frame_id(img, &lo);
        if got != Some(*want) {
            bad.push(json!({"frame": f, "want": want, "got": got}));
        }
    }
    // tones per clip (FFT at the clip middle)
    let tone_of = |asset: &str| match asset {
        "a" => 440.0,
        "b" => 660.0,
        _ => 880.0,
    };
    let mut tone_bad = vec![];
    for c in &tl.clips {
        let mid = tl.secs(c.start + c.frames / 2);
        let s = cutline_testkit::analyze::read_audio(tc, &r.output, mid - 0.05, Some(0.1), 1)?;
        let fhz = cutline_testkit::analyze::dominant_freq(&s);
        let want = tone_of(&c.asset) * c.speed;
        if (fhz - want).abs() > want * 0.03 {
            tone_bad.push(json!({"clip": c.id, "want": want, "got": fhz}));
        }
    }
    let ok = bad.is_empty() && tone_bad.is_empty() && r.qa.ok;
    Ok(Outcome::check(
        ok,
        json!({"frames_checked": samples.len(), "frame_mismatches": bad, "tone_mismatches": tone_bad, "qa_errors": r.qa.errors, "qa_warnings": r.qa.warnings}),
        json!({"frame_mismatches": 0, "tone_mismatches": 0, "qa_errors": 0}),
    )
    .artifacts(vec![r.output.clone(), r.qa_path.clone()]))
}

pub const SHORTS_PROJECT: &str = r#"{
  version: 1, name: "selftest_shorts", target: "shorts",
  assets: { a: {path: "a.mp4"}, b: {path: "b.mp4"}, c: {path: "c.mp4"} },
  main: [
    { id: "hook", clip: "a", in: 1.0, out: 3.0 },
    { transition: { type: "fade", duration: 0.3 } },
    { id: "step", clip: "b", in: 2.0, out: 4.0 },
    { id: "outro", clip: "c", in: 3.0, out: 5.0 },
  ],
}"#;
