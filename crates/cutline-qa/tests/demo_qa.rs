//! Review's demo checks: private details read from the recording, legibility, pacing; and masks
//! that follow a framed, zoomed clip.

use cutline_core::model::OcrEngine;
use cutline_qa::demo_qa;
use cutline_qa::ocr;
use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{mean_rgb, read_frames};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

/// A 1600×1000 "screen": white with large black text lines, one of them an email address at
/// x 100.., y 380..440, and small print further down.
fn screen(d: &Path) {
    let tc = test_toolchain().unwrap();
    let font = cutline_deps::paths::assets_dir().join("fonts").join("Inter[opsz,wght].ttf");
    let ff = font
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(',', "\\,");
    let text = |t: &str, y: u32, size: u32| {
        format!("drawtext=fontfile='{ff}':text='{t}':x=100:y={y}:fontsize={size}:fontcolor=black")
    };
    let vf = [
        text("Account settings", 120, 64),
        text("Signed in as", 280, 48),
        text("grace.hopper@navy-lab.io", 380, 56),
        text("Plan Business", 520, 48),
        text("Seats 42 of 50", 620, 48),
    ]
    .join(",");
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "color=c=white:s=1600x1000:d=12:r=30", "-vf", &vf, "-pix_fmt", "yuv420p"])
        .arg(d.join("screen.mp4"));
    gen::run(c).unwrap();
}

fn project(d: &Path, target: &str, extra_layers: &str) -> PathBuf {
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'q', {target}, fps: 30, profile: 'screencast',
              assets: {{screen: {{path: 'screen.mp4'}}}}, audio: {{voice_chain: 'raw'}},
              default_frame: {{style: 'browser', background: '#101014', shadow: 0}},
              main: [{{id: 'a', clip: 'screen'}}],
              layers: [{{type: 'highlight', rect: {{x: 0.05, y: 0.1, w: 0.4, h: 0.1}}, start: 0, end: 2}}{extra_layers}]}}"
        ),
    )
    .unwrap();
    p
}

fn prepared(pp: &Path) -> (pipeline::Loaded, pipeline::Prepared) {
    let l = pipeline::load(pp).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, true).unwrap();
    (l, pr)
}

#[test]
fn pacing_is_checked_without_text_recognition() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    screen(d.path());
    let (l, pr) = prepared(&project(d.path(), "target: 'long_1080p'", ""));
    // nothing is said, and the only annotation ends at 2 s: 10 quiet seconds follow
    assert_eq!(demo_qa::quiet_gaps(&pr, 8.0), vec![(2.0, 12.0)]);
    let rep = demo_qa::run(&l, &pr, Some("off")).unwrap();
    assert!(rep.ocr.starts_with("not run"), "{}", rep.ocr);
    assert_eq!(rep.findings.iter().map(|f| f.kind.as_str()).collect::<Vec<_>>(), vec!["pacing"]);
    assert!(rep.findings[0].message.contains("10.0s"), "{}", rep.findings[0].message);
}

#[test]
fn private_details_and_small_text_are_found_with_each_available_engine() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    screen(d.path());
    let mut ran = 0;
    for (name, setting) in [("vision", OcrEngine::Vision), ("tesseract", OcrEngine::Tesseract)] {
        if let Err(why) = ocr::resolve(setting) {
            eprintln!("SKIP {name}: {why}");
            continue;
        }
        ran += 1;
        // a vertical target: the 1600 px wide screen becomes ~950 px wide, so 48 px text is ~28 px
        let (l, pr) = prepared(&project(d.path(), "target: 'shorts'", ""));
        let rep = demo_qa::run(&l, &pr, Some(name)).unwrap();
        assert_eq!(rep.ocr, name);
        assert!(rep.frames_read >= 2, "{rep:?}");
        let private: Vec<_> = rep.findings.iter().filter(|f| f.kind == "private").collect();
        assert_eq!(private.len(), 1, "{name}: one finding for the one address: {:?}", rep.findings);
        assert!(
            private[0].message.contains("email") && private[0].message.contains("navy-lab.io"),
            "{}",
            private[0].message
        );
        // the suggested mask covers the line (x 100/1600, y 380/1000, about 56 px tall)
        let r = &private[0].fix.as_ref().unwrap()["rect"];
        let (x, y, w, h) =
            (r["x"].as_f64().unwrap(), r["y"].as_f64().unwrap(), r["w"].as_f64().unwrap(), r["h"].as_f64().unwrap());
        assert!(x < 0.07 && x > 0.04 && y > 0.35 && y < 0.40 && w > 0.3 && h > 0.035 && h < 0.09, "{name}: {r}");
        assert!(
            !rep.findings.iter().any(|f| f.kind == "legibility"),
            "{name}: large text reads fine: {:?}",
            rep.findings
        );

        // with that mask in place the address is covered: no finding
        let fix = serde_json::to_string(private[0].fix.as_ref().unwrap()).unwrap();
        let (l, pr) = prepared(&project(d.path(), "target: 'shorts'", &format!(", {fix}")));
        let rep = demo_qa::run(&l, &pr, Some(name)).unwrap();
        assert!(!rep.findings.iter().any(|f| f.kind == "private"), "{name}: {:?}", rep.findings);

        // the same screen in a small window (30 % padding): its text is now 15–20 px tall at 1080p
        let pp = project(d.path(), "target: 'long_1080p'", "");
        let text = std::fs::read_to_string(&pp).unwrap().replace("shadow: 0}", "shadow: 0, padding: 0.3}");
        std::fs::write(&pp, text).unwrap();
        let (l, pr) = prepared(&pp);
        let rep = demo_qa::run(&l, &pr, Some(name)).unwrap();
        let leg = rep
            .findings
            .iter()
            .find(|f| f.kind == "legibility")
            .unwrap_or_else(|| panic!("{name}: {:?}", rep.findings));
        assert!(leg.message.contains("small in the 1920×1080 output"), "{}", leg.message);
    }
    if ran == 0 {
        eprintln!("SKIP: no text recognition engine");
    }
}

#[test]
fn a_following_mask_stays_on_its_text_inside_a_frame_and_a_zoom() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    screen(d.path());
    // mask the address line (recording fractions) in red, and zoom towards it
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, name: 'm', target: 'long_1080p', fps: 30, profile: 'screencast',
          assets: {screen: {path: 'screen.mp4'}}, audio: {voice_chain: 'raw'},
          default_frame: {style: 'browser', background: '#101014', shadow: 0},
          main: [{id: 'a', clip: 'screen', out: 4, effects: [{type: 'zoom_to', rect: {x: 0.0, y: 0.3, w: 0.6, h: 0.3}, start: 1, duration: 0.5}]}],
          layers: [{type: 'mask', mode: 'fill', follow: true, color: '#FF0000', anchor: 'a', rect: {x: 0.055, y: 0.37, w: 0.47, h: 0.075}}]}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let out = pipeline::render(&l, &pr, &RenderOptions { jobs: 3, ..Default::default() }).unwrap().0.output;
    let tl = &pr.timeline;
    let ct = tl.clips[0].content.unwrap();
    let f = read_frames(tc, &out, &[10, 75]).unwrap();
    // where the mask's centre is on screen: through the frame, then through the zoom
    for (img, fr) in f.iter().zip([10i64, 75]) {
        let (s, left, top) = cutline_render::effects::zoom_at(&tl.clips[0], tl.secs(tl.clips[0].frames), tl.secs(fr));
        let (mx, my) = (ct.x + (0.055 + 0.235) * ct.w, ct.y + (0.37 + 0.0375) * ct.h);
        let (px, py) = (((mx - left) * s * 1920.0) as u32, ((my - top) * s * 1080.0) as u32);
        let c = mean_rgb(img, px - 20, py - 6, 40, 12);
        assert!(c[0] > 200.0 && c[1] < 50.0 && c[2] < 50.0, "mask at frame {fr} around ({px}, {py}): {c:?}");
        // and no black text shows through anywhere on that line
        let line = mean_rgb(img, px - 60, py - 4, 120, 8);
        assert!(line[0] > 200.0 && line[1] < 50.0, "{line:?}");
    }
    assert!(tl.clips[0].frames > 75);
}

#[test]
fn a_following_mask_must_be_a_fill() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    screen(d.path());
    let l = pipeline::load(&project(
        d.path(),
        "target: 'long_1080p'",
        ", {type: 'mask', follow: true, rect: {x: 0.1, y: 0.1, w: 0.2, h: 0.1}}",
    ))
    .unwrap();
    assert!(l.diags.iter().any(|x| x.code == "E_FIELD_CONFLICT" && x.path == "/layers/1/follow"), "{:?}", l.diags);
}
