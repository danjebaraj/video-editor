//! M4 acceptance: template goldens (Shorts + 16:9), auto-fit inside the safe zone measured on
//! rendered pixels, rendered ASS escaping, identical output with/without system fonts.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{golden_compare, read_frames};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

const TEMPLATES: &[(&str, &str)] = &[
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

fn bg(dir: &Path) {
    let tc = test_toolchain().unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "color=c=0x30405a:s=1920x1080:d=1", "-frames:v", "1"]).arg(dir.join("bg.png"));
    gen::run(c).unwrap();
}

fn render(project: &Path, envs: &[(&str, &str)]) -> (PathBuf, pipeline::Prepared) {
    for (k, v) in envs {
        std::env::set_var(k, v);
    }
    let l = pipeline::load(project).unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let (plan, _) = pipeline::render(&l, &pr, &RenderOptions::default()).unwrap();
    for (k, _) in envs {
        std::env::remove_var(k);
    }
    (plan.output, pr)
}

fn template_project(dir: &Path, target: &str) -> PathBuf {
    let layers: Vec<String> = TEMPLATES
        .iter()
        .enumerate()
        .map(|(i, (t, extra))| format!("{{type: 'text', template: '{t}', start: {i}, duration: 1, {extra}}}"))
        .collect();
    let p = dir.join(format!("{target}.json5"));
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'tpl_{target}', target: '{target}', fps: 30, assets: {{bg: {{path: 'bg.png'}}}}, main: [{{id: 'c', clip: 'bg', duration: {}}}], layers: [{}]}}",
            TEMPLATES.len(),
            layers.join(",")
        ),
    )
    .unwrap();
    p
}

#[test]
fn template_goldens_shorts_and_wide() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    bg(d.path());
    let major = cutline_deps::tools::parse_version(&tc.version).0;
    let gold = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/golden/templates/ffmpeg{major}"));
    if !gold.exists() && std::env::var("CUTLINE_BLESS_MISSING").is_err() && std::env::var("CUTLINE_BLESS").is_err() {
        eprintln!("SKIP: no template goldens for ffmpeg {major}");
        return;
    }
    let mut failures = vec![];
    for target in ["shorts", "long_1080p"] {
        let pp = template_project(d.path(), target);
        let (out, pr) = render(&pp, &[]);
        // QA boxes from layout must all be inside the safe zone
        let (sx, sy, sw, sh) = pr.timeline.target.safe_rect();
        for b in &pr.extras.text_boxes {
            assert!(
                b.x >= sx - 1.0 && b.y >= sy - 1.0 && b.x + b.w <= sx + sw + 1.0 && b.y + b.h <= sy + sh + 1.0,
                "{target}: box outside safe zone: {b:?}"
            );
        }
        // frame 0.7 s into each template (after pop/slide animations settle)
        let idx: Vec<u64> = (0..TEMPLATES.len()).map(|i| (i as u64) * 30 + 21).collect();
        let frames = read_frames(tc, &out, &idx).unwrap();
        for ((t, _), img) in TEMPLATES.iter().zip(frames) {
            let small =
                image::imageops::resize(&img, img.width() / 2, img.height() / 2, image::imageops::FilterType::Triangle);
            if let Err(e) = golden_compare(&small, &gold.join(format!("{target}_{t}.png")), 0.97) {
                failures.push(format!("{target}/{t}: {e}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Bounding box of pixels brighter than the background in a frame.
fn bright_bbox(img: &image::RgbImage, thr: u8) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p[0].max(p[1]).max(p[2]) > thr {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    (x1 >= x0 && y1 >= y0 && x0 != u32::MAX).then_some((x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

#[test]
fn autofit_keeps_long_title_in_safe_zone_and_escapes_render_literally() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "color=c=black:s=1080x1920:d=1", "-frames:v", "1"]).arg(d.path().join("bg.png"));
    gen::run(c).unwrap();
    let long = "This is an absurdly long hook title that would never fit on a phone screen at the default size {braces} and C:\\\\New stays literal";
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        format!(
            "{{version: 1, name: 'fit', target: 'shorts', assets: {{bg: {{path: 'bg.png'}}}}, main: [{{id: 'c', clip: 'bg', duration: 2}}], layers: [{{type: 'text', template: 'hook_title', text: {long:?}, start: 0, duration: 2, params: {{uppercase: false}}}}]}}"
        ),
    )
    .unwrap();
    let (out, pr) = render(&pp, &[]);
    let f = &read_frames(tc, &out, &[45]).unwrap()[0];
    let (x, y, w, h) = bright_bbox(f, 40).expect("text rendered");
    let (sx, sy, sw, sh) = pr.timeline.target.safe_rect();
    assert!(x as f64 >= sx - 2.0 && y as f64 >= sy - 2.0, "bbox {x},{y} starts outside safe {sx},{sy}");
    assert!(
        (x + w) as f64 <= sx + sw + 2.0 && (y + h) as f64 <= sy + sh + 2.0,
        "bbox {x},{y},{w},{h} ends outside safe zone {sx},{sy},{sw},{sh}"
    );
    // layout prediction vs rendered pixels (shadow adds a little)
    let b = &pr.extras.text_boxes[0];
    assert!((w as f64 - b.w).abs() / b.w < 0.08, "rendered width {w} vs predicted {}", b.w);
    // line boxes span the full font height (win ascent+descent); ink is shorter but must lie
    // inside the predicted box
    assert!(h as f64 <= b.h + 4.0 && h as f64 >= 0.55 * b.h, "rendered height {h} vs predicted {}", b.h);
    assert!(
        y as f64 >= b.y - 4.0 && (y + h) as f64 <= b.y + b.h + 8.0,
        "ink {y}+{h} outside predicted box {}+{}",
        b.y,
        b.h
    );
    // the escaped braces/backslash produced glyphs rather than override blocks: prediction (which
    // includes them as text) matched rendered width above. Also check the ASS source.
    let ass = &pr.extras.ass_docs[0].1;
    assert!(ass.contains("\\{braces\\}"), "{ass}");
}

#[test]
fn identical_render_with_and_without_system_fonts() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    bg(d.path());
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, name: 'fonts', target: 'shorts', assets: {bg: {path: 'bg.png'}}, main: [{id: 'c', clip: 'bg', duration: 1}], layers: [{type: 'text', template: 'hook_title', text: 'Fonts are bundled', start: 0, duration: 1}, {type: 'text', template: 'lower_third', text: 'Name', subtitle: 'Role', start: 0, duration: 1}]}",
    )
    .unwrap();
    let (a, _) = render(&pp, &[]);
    let fa = read_frames(tc, &a, &[20]).unwrap().remove(0);
    std::fs::remove_dir_all(d.path().join(".cutline")).unwrap();
    // hide every system font from fontconfig for the whole process tree
    let empty = d.path().join("empty-fonts.conf");
    std::fs::write(
        &empty,
        "<?xml version=\"1.0\"?><!DOCTYPE fontconfig SYSTEM \"fonts.dtd\"><fontconfig></fontconfig>",
    )
    .unwrap();
    let (b, _) =
        render(&pp, &[("FONTCONFIG_FILE", empty.to_str().unwrap()), ("XDG_DATA_HOME", d.path().to_str().unwrap())]);
    let fb = read_frames(tc, &b, &[20]).unwrap().remove(0);
    assert!(fa.as_raw() == fb.as_raw(), "frames differ with/without system fonts");
}
