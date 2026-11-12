//! `selftest --full` checks (in addition to `--quick`).

use crate::selftest::{Outcome, Runner, Status};
use anyhow::{Context, Result};
use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{golden_compare, read_all_frames, read_audio, read_frames};
use cutline_testkit::gen;
use serde_json::json;
use std::path::{Path, PathBuf};

const FACE_JPG: &[u8] = include_bytes!("../../../tests/fixtures/face_armstrong.jpg");
const SCRIBE: &str = include_str!("../../cutline-transcribe/tests/fixtures/scribe_sample.json");

pub fn run_all(r: &mut Runner) {
    r.run("full/transitions_all", check_transitions);
    r.run("full/caption_styles", check_captions);
    r.run("full/text_templates", check_templates);
    r.run("full/whisper_wer", check_whisper);
    r.run("full/reframe_tracking", check_reframe);
    r.run("full/alpha_correctness", check_alpha);
    r.run("full/loudness_accuracy", check_loudness);
    r.run("full/av_sync", check_av_sync);
    r.run("full/punch_in_smoothness", check_punch_in);
    r.run("full/performance", check_performance);
}

/// Golden dir for a set (dev builds find the source tree; installs can point
/// `CUTLINE_GOLDEN_DIR` at a checkout's `crates/cutline-render/tests/golden`).
fn golden_dir(set: &str) -> Option<PathBuf> {
    let tc = cutline_deps::toolchain().ok()?;
    let major = cutline_deps::tools::parse_version(&tc.version).0;
    let base = std::env::var_os("CUTLINE_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../cutline-render/tests/golden"));
    let d = base.join(set).join(format!("ffmpeg{major}"));
    d.exists().then_some(d)
}

fn render(project: &Path) -> Result<(PathBuf, pipeline::Prepared)> {
    let l = pipeline::load(project).map_err(|e| anyhow::anyhow!("{e}"))?;
    anyhow::ensure!(!cutline_core::diag::has_errors(&l.diags), "invalid project: {:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false)?;
    let (plan, _) = pipeline::render(&l, &pr, &RenderOptions::default())?;
    let _ = std::fs::remove_dir_all(&plan.work_dir);
    Ok((plan.output, pr))
}

fn tagged_source(tc: &cutline_deps::Toolchain, out: &Path, src: &str, extra_audio: &str) -> Result<()> {
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", src, "-f", "lavfi", "-i", extra_audio]);
    c.args([
        "-vf",
        "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
    ]);
    c.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "12", "-c:a", "aac", "-shortest"]).arg(out);
    gen::run(c)
}

fn check_transitions(d: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    tagged_source(tc, &d.join("a.mp4"), "testsrc2=s=320x180:r=30:d=3", "sine=f=440:d=3")?;
    tagged_source(tc, &d.join("b.mp4"), "smptebars=s=320x180:r=30:d=3", "sine=f=660:d=3")?;
    let mut ts: Vec<&str> = cutline_core::catalog::XFADE_TRANSITIONS.to_vec();
    ts.extend(cutline_core::catalog::CUSTOM_TRANSITIONS.iter().filter(|t| **t != "cut"));
    let mut main = vec![];
    for (i, t) in ts.iter().enumerate() {
        main.push(format!("{{id: 'c{i}', clip: '{}', in: 1, out: 1.6}}", if i % 2 == 0 { "a" } else { "b" }));
        main.push(format!("{{transition: {{type: '{t}', duration: 0.3}}}}"));
    }
    main.push(format!("{{id: 'last', clip: '{}', in: 1, out: 1.6}}", if ts.len() % 2 == 0 { "a" } else { "b" }));
    let pp = d.join("p.json5");
    std::fs::write(&pp, format!("{{version: 1, name: 'tr', target: 'custom', width: 320, height: 180, fps: 30, assets: {{a: {{path: 'a.mp4'}}, b: {{path: 'b.mp4'}}}}, main: [{}], audio: {{voice_chain: 'raw'}}}}", main.join(",")))?;
    let (out, pr) = render(&pp)?;
    let tl = &pr.timeline;
    let expected = 18 * (ts.len() as i64 + 1) - 9 * ts.len() as i64;
    let o = tc
        .ffprobe_cmd()
        .args(["-count_frames", "-select_streams", "v:0", "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0"])
        .arg(&out)
        .output()?;
    let frames: i64 = String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(-1);
    let mut golden_fail = vec![];
    let gold = golden_dir("transitions");
    if let Some(g) = &gold {
        let mids: Vec<u64> = tl.transitions.iter().map(|t| (t.start + t.frames / 2) as u64).collect();
        let imgs = read_frames(tc, &out, &mids)?;
        for (t, img) in tl.transitions.iter().zip(imgs) {
            std::env::remove_var("CUTLINE_BLESS");
            if let Err(e) = golden_compare(&img, &g.join(format!("{}.png", t.transition.kind)), 0.97) {
                golden_fail.push(format!("{}: {e}", t.transition.kind));
            }
        }
    }
    let ok = frames == expected && golden_fail.is_empty();
    let mut o = Outcome::check(
        ok,
        json!({"transitions": ts.len(), "frames": frames, "golden_failures": golden_fail, "goldens": gold.is_some()}),
        json!({"frames": expected}),
    );
    if gold.is_none() {
        o = o.msg("golden images unavailable (set CUTLINE_GOLDEN_DIR); checked durations only");
    }
    Ok(o)
}

fn check_captions(d: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        "color=c=0x30405a:s=1080x1920:r=30:d=4",
        "-f",
        "lavfi",
        "-i",
        "sine=f=300:d=4",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
    ])
    .arg(d.join("v.mp4"));
    gen::run(c)?;
    std::fs::write(d.join("vo.scribe.json"), SCRIBE)?;
    let gold = golden_dir("captions");
    let mut fails = vec![];
    let mut outside = 0;
    for style in cutline_core::catalog::CAPTION_STYLES {
        let pp = d.join(format!("{style}.json5"));
        std::fs::write(&pp, format!("{{version: 1, name: '{style}', target: 'shorts', assets: {{v: {{path: 'v.mp4', transcript: 'vo.scribe.json'}}}}, main: [{{id: 'c', clip: 'v'}}], captions: {{style: '{style}', emphasis: ['this'], emoji: {{dull: 'knife'}}}}}}"))?;
        let (out, pr) = render(&pp)?;
        let (sx, sy, sw, sh) = pr.timeline.target.safe_rect();
        outside += pr
            .extras
            .text_boxes
            .iter()
            .filter(|b| b.x < sx - 1.0 || b.x + b.w > sx + sw + 1.0 || b.y < sy - 1.0 || b.y + b.h > sy + sh + 1.0)
            .count();
        if let Some(g) = &gold {
            for (k, img) in read_frames(tc, &out, &[30, 57])?.iter().enumerate() {
                let small = image::imageops::resize(
                    img,
                    img.width() / 2,
                    img.height() / 2,
                    image::imageops::FilterType::Triangle,
                );
                if let Err(e) = golden_compare(&small, &g.join(format!("{style}_{k}.png")), 0.97) {
                    fails.push(format!("{style}/{k}: {e}"));
                }
            }
        }
    }
    let ok = fails.is_empty() && outside == 0;
    Ok(Outcome::check(
        ok,
        json!({"styles": cutline_core::catalog::CAPTION_STYLES, "golden_failures": fails, "boxes_outside_safe_zone": outside, "goldens": gold.is_some()}),
        json!({"golden_failures": 0, "boxes_outside_safe_zone": 0}),
    ))
}

fn check_templates(d: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "color=c=0x30405a:s=1920x1080:d=1", "-frames:v", "1"]).arg(d.join("bg.png"));
    gen::run(c)?;
    let templates: &[(&str, &str)] = &[
        ("hook_title", r#"text: "Your knife is dull because of THIS", params: {emphasis: ["THIS"]}"#),
        ("lower_third", r#"text: "Dan Jebaraj", subtitle: "Chef & knife nerd""#),
        ("callout", r#"text: "Hold at 15°", position: {x: 0.5, y: 0.45}"#),
        ("step_counter", r#"params: {step: 2, total: 5}"#),
        ("big_number", r#"params: {number: 92, suffix: "%"}, subtitle: "sharper""#),
        ("quote", r#"text: "A sharp knife is a safe knife.", subtitle: "Every chef ever""#),
        ("cta_subscribe", ""),
        ("end_card", r#"subtitle: "More tips every week""#),
        ("emoji_pop", r#"text: "fire""#),
    ];
    let gold = golden_dir("templates");
    let mut fails = vec![];
    for target in ["shorts", "long_1080p"] {
        let layers: Vec<String> = templates
            .iter()
            .enumerate()
            .map(|(i, (t, extra))| format!("{{type: 'text', template: '{t}', start: {i}, duration: 1, {extra}}}"))
            .collect();
        let pp = d.join(format!("{target}.json5"));
        std::fs::write(&pp, format!("{{version: 1, name: 'tpl_{target}', target: '{target}', fps: 30, assets: {{bg: {{path: 'bg.png'}}}}, main: [{{id: 'c', clip: 'bg', duration: {}}}], layers: [{}]}}", templates.len(), layers.join(",")))?;
        let (out, _) = render(&pp)?;
        if let Some(g) = &gold {
            let idx: Vec<u64> = (0..templates.len()).map(|i| i as u64 * 30 + 21).collect();
            for ((t, _), img) in templates.iter().zip(read_frames(tc, &out, &idx)?) {
                let small = image::imageops::resize(
                    &img,
                    img.width() / 2,
                    img.height() / 2,
                    image::imageops::FilterType::Triangle,
                );
                if let Err(e) = golden_compare(&small, &g.join(format!("{target}_{t}.png")), 0.97) {
                    fails.push(format!("{target}/{t}: {e}"));
                }
            }
        }
    }
    Ok(Outcome::check(
        fails.is_empty(),
        json!({"templates": templates.len(), "golden_failures": fails, "goldens": gold.is_some()}),
        json!({"golden_failures": 0}),
    ))
}

fn check_whisper(d: &Path) -> Result<Outcome> {
    use cutline_transcribe::{model::wer, whisper};
    let tc = cutline_deps::toolchain()?;
    let model = std::env::var("CUTLINE_WHISPER_MODEL").unwrap_or_else(|_| whisper::FAST_MODEL.into());
    if let Err(e) = whisper::available(&model) {
        return Ok(Outcome::skip(format!("{e:#}")));
    }
    let sentence = "The quick brown fox jumps over the lazy dog while the chef sharpens a knife.";
    let wav = d.join("speech.wav");
    if !whisper::synthesize(sentence, &wav, tc)? {
        return Ok(Outcome::skip("no TTS engine (say / SAPI / espeak-ng)"));
    }
    let t = whisper::transcribe(tc, &wav, &model, Some("en"), &d.join("w"))?;
    let w = wer(sentence, &t.text());
    let mono = t.words.windows(2).all(|p| p[0].start <= p[1].start);
    Ok(Outcome::check(
        w <= 0.15 && mono,
        json!({"wer": w, "text": t.text(), "monotone": mono}),
        json!({"wer_max": 0.15}),
    ))
}

fn check_reframe(d: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    if cutline_reframe::default_detector().is_none() {
        return Ok(Outcome::skip(format!("face detector: {}", cutline_reframe::detector_status())));
    }
    let face = d.join("face.jpg");
    std::fs::write(&face, FACE_JPG)?;
    let (dur, fw, x0, x1, fcx) = (8.0, 360.0, 150.0, 1400.0, 0.615);
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &format!("color=c=0x505860:s=1920x1080:r=30:d={dur}"),
        "-loop",
        "1",
        "-framerate",
        "30",
        "-t",
        &dur.to_string(),
        "-i",
    ])
    .arg(&face);
    c.args([
        "-filter_complex",
        &format!(
            "[1:v]scale={fw}:-2[f];[0:v][f]overlay=x='{x0}+({x1}-{x0})*t/{dur}':y=180:shortest=1,format=yuv420p[v]"
        ),
        "-map",
        "[v]",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "16",
    ])
    .arg(d.join("pan.mp4"));
    gen::run(c)?;
    let pp = d.join("p.json5");
    std::fs::write(&pp, "{version: 1, name: 'rf', target: 'shorts', assets: {v: {path: 'pan.mp4'}}, main: [{id: 'c', clip: 'v', reframe: {mode: 'track_face', max_pan_speed: 0.5}}]}")?;
    let l = pipeline::load(&pp).map_err(|e| anyhow::anyhow!("{e}"))?;
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false)?;
    let path = pr.extras.crop_paths.get("c").context("no faces tracked")?;
    let truth = |t: f64| (x0 + (x1 - x0) * t / dur + fcx * fw) / 1920.0;
    let settle = path.fps as usize;
    let err = (settle..path.len()).map(|i| (path.center_x(i) - truth(i as f64 / path.fps)).abs()).fold(0.0, f64::max);
    let speed =
        path.x.windows(2).map(|w| (w[1] as f64 - w[0] as f64).abs() * path.fps / path.src_w as f64).fold(0.0, f64::max);
    let ok = err <= 0.08 && speed <= 0.5 + 2.0 * path.fps / path.src_w as f64;
    Ok(Outcome::check(
        ok,
        json!({"max_error_width_fraction": err, "max_pan_speed": speed}),
        json!({"max_error": 0.08, "max_pan_speed": 0.5}),
    ))
}

fn check_alpha(d: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    gen::still(tc, &d.join("bg.png"), 640, 360, "0x3c6eaa")?;
    let graph = "color=c=black@0:s=640x360:r=30:d=2,format=rgba,geq=r='255*lte(hypot(X-320,Y-180),100)':g='40*lte(hypot(X-320,Y-180),100)':b='40*lte(hypot(X-320,Y-180),100)':a='192*lte(hypot(X-320,Y-180),100)'";
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", graph, "-c:v", "prores_ks", "-profile:v", "4444", "-pix_fmt", "yuva444p10le"])
        .arg(d.join("o.mov"));
    gen::run(c)?;
    let pp = d.join("p.json5");
    std::fs::write(&pp, "{version: 1, name: 'alpha', target: 'custom', width: 640, height: 360, fps: 30, assets: {bg: {path: 'bg.png'}, o: {path: 'o.mov', alpha: true}}, main: [{id: 'c', clip: 'bg', duration: 2}], layers: [{type: 'video', asset: 'o', start: 0, scale: 1.0, position: 'center'}]}")?;
    let (out, _) = render(&pp)?;
    let f = &read_frames(tc, &out, &[30])?[0];
    let inside = cutline_testkit::analyze::mean_rgb(f, 300, 160, 40, 40);
    let a = 192.0 / 255.0;
    let want = [a * 255.0 + (1.0 - a) * 60.0, a * 40.0 + (1.0 - a) * 110.0, a * 40.0 + (1.0 - a) * 170.0];
    let err = (0..3).map(|i| (inside[i] - want[i]).abs()).fold(0.0, f64::max);
    Ok(Outcome::check(
        err <= 4.0,
        json!({"composited": inside, "expected": want, "max_error": err}),
        json!({"max_error": 4.0}),
    ))
}

fn check_loudness(d: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    tagged_source(tc, &d.join("a.mp4"), "testsrc2=s=640x360:r=30:d=6", "sine=f=440:d=6")?;
    gen::noise_bed(tc, &d.join("m.wav"), 8.0, 0.3)?;
    let pp = d.join("p.json5");
    std::fs::write(&pp, "{version: 1, name: 'loud', target: 'long_1080p', assets: {a: {path: 'a.mp4'}, m: {path: 'm.wav', role: 'music'}}, main: [{id: 'c', clip: 'a'}], layers: [{type: 'audio', asset: 'm', gain_db: -10}], audio: {loudness: {integrated: -16, true_peak: -1.5}, voice_chain: 'raw'}}")?;
    let (out, _) = render(&pp)?;
    let l = cutline_media::probe::measure_loudness(tc, &out)?;
    let ok = (l.integrated_lufs + 16.0).abs() <= 0.5 && l.true_peak_dbtp <= -1.45;
    Ok(Outcome::check(ok, json!(l), json!({"integrated": -16, "tolerance": 0.5, "true_peak_max": -1.5})))
}

fn check_av_sync(d: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    let at = [1.0, 2.5, 4.0];
    gen::beep_flash(tc, &d.join("bf.mp4"), 640, 360, 30, 5.0, &at)?;
    let pp = d.join("p.json5");
    std::fs::write(&pp, "{version: 1, name: 'sync', target: 'long_1080p', fps: 30, assets: {v: {path: 'bf.mp4'}}, main: [{id: 'a', clip: 'v', in: 0.5, out: 2.0}, {id: 'b', clip: 'v', in: 2.0, out: 4.6}], audio: {voice_chain: 'raw'}}")?;
    let (out, _) = render(&pp)?;
    // flash frames (bright) and beep onsets
    let frames = read_all_frames(tc, &out, Some((0, 0, 64, 64)))?;
    let flashes: Vec<f64> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| cutline_testkit::analyze::mean_rgb(f, 0, 0, 64, 64)[0] > 128.0)
        .map(|(i, _)| i as f64 / 30.0)
        .collect();
    let s = read_audio(tc, &out, 0.0, None, 1)?;
    let mut beeps = vec![];
    let win = 240;
    let mut in_beep = false;
    for (k, c) in s.chunks(win).enumerate() {
        let rms = cutline_testkit::analyze::rms_db(c);
        if rms > -30.0 && !in_beep {
            beeps.push(k as f64 * win as f64 / 48000.0);
            in_beep = true;
        } else if rms < -40.0 {
            in_beep = false;
        }
    }
    let mut offsets = vec![];
    for f in &flashes {
        if let Some(b) = beeps.iter().min_by(|a, b| (*a - f).abs().partial_cmp(&(*b - f).abs()).unwrap()) {
            offsets.push(b - f);
        }
    }
    let worst = offsets.iter().map(|o: &f64| o.abs()).fold(0.0, f64::max);
    let ok = !flashes.is_empty() && flashes.len() == beeps.len() && worst <= 1.0 / 30.0 + 0.006;
    Ok(Outcome::check(
        ok,
        json!({"flashes": flashes, "beeps": beeps, "max_offset_s": worst}),
        json!({"max_offset_s": 1.0 / 30.0}),
    ))
}

fn check_punch_in(d: &Path) -> Result<Outcome> {
    let tc = cutline_deps::toolchain()?;
    gen::grid_video(tc, &d.join("grid.mp4"), 1280, 720, 30, 3.0, 40)?;
    let pp = d.join("p.json5");
    std::fs::write(&pp, "{version: 1, name: 'punch', target: 'custom', width: 1280, height: 720, fps: 30, assets: {g: {path: 'grid.mp4'}}, main: [{id: 'c', clip: 'g', in: 0, out: 2.5, effects: [{type: 'punch_in', at: 0.5, scale: 1.3, duration: 1.0, ease: 'out_cubic'}]}]}")?;
    let (out, _) = render(&pp)?;
    let frames = read_all_frames(tc, &out, Some((240, 340, 800, 40)))?;
    let period = |f: &image::RgbImage| -> f64 {
        let row: Vec<f64> = (0..f.width()).map(|x| f.get_pixel(x, 20)[1] as f64).collect();
        let m = 128.0;
        let xs: Vec<f64> = (1..row.len())
            .filter(|&i| (row[i - 1] - m).signum() != (row[i] - m).signum())
            .map(|i| (i - 1) as f64 + (row[i - 1] - m) / (row[i - 1] - row[i]))
            .collect();
        if xs.len() < 3 {
            0.0
        } else {
            2.0 * (xs[xs.len() - 1] - xs[0]) / (xs.len() - 1) as f64
        }
    };
    let p: Vec<f64> = frames.iter().map(period).collect();
    let scales: Vec<f64> = p.iter().map(|v| v / p[0].max(1e-9)).collect();
    let reversals = scales.windows(2).filter(|w| w[1] < w[0] - 0.002).count();
    let fin = *scales.last().unwrap_or(&0.0);
    Ok(Outcome::check(
        reversals == 0 && (fin - 1.3).abs() < 0.03,
        json!({"reversals": reversals, "final_scale": fin}),
        json!({"reversals": 0, "final_scale": 1.3}),
    ))
}

fn check_performance(d: &Path) -> Result<Outcome> {
    crate::fixtures::sources(d)?;
    let pp = d.join("perf.json5");
    std::fs::write(&pp, crate::suite::SHORTS_PROJECT)?;
    let t0 = std::time::Instant::now();
    render(&pp)?;
    let cold = t0.elapsed().as_secs_f64();
    // caption-only change → segments cached
    std::fs::write(&pp, crate::suite::SHORTS_PROJECT.replace("emphasis: [\"sharp\"]", "emphasis: [\"cleaner\"]"))?;
    let t1 = std::time::Instant::now();
    render(&pp)?;
    let warm = t1.elapsed().as_secs_f64();
    let dur = 5.75;
    let o = Outcome {
        status: Status::Pass,
        measured: json!({"cold_s": cold, "warm_s": warm, "timeline_s": dur, "cold_realtime_factor": cold / dur, "warm_realtime_factor": warm / dur, "cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)}),
        threshold: json!({"note": "recorded benchmark; spec targets ≤ 1.5× realtime cold, ≤ 0.33× warm for 60 s on Apple Silicon"}),
        artifacts: vec![],
        message: None,
    };
    Ok(o)
}
