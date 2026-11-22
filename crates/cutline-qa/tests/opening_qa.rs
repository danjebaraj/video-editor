//! Opening-frame QA on dark title cards (TASK-silver-cherry-R3MZ, TASK-honest-cedar-A73E).
//!
//! blackdetect calls a frame black when 98 % of it is under 8 % luma, so a near-black card with
//! sparse light text counted as a black span. And the hook check only knew the caption and text
//! layers, so a title drawn by an HTML card from frame 0 was a "first text at 1.34 s" warning.
//! Each case below is rendered through the normal render + QA path.

use cutline_qa::deliver::render_all;
use cutline_qa::qa::{Finding, QaReport};
use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

fn finding<'a>(r: &'a QaReport, check: &str) -> &'a Finding {
    r.findings.iter().find(|f| f.check == check).unwrap_or_else(|| panic!("no {check} finding"))
}

fn card(dir: &Path, name: &str, bg: &str, text: &str) {
    std::fs::write(
        dir.join(name),
        format!(
            "<!doctype html><html><body style=\"margin:0;width:100vw;height:100vh;background:{bg};display:flex;\
             align-items:center;justify-content:center\"><div style=\"color:#F2EDE4;font:600 44px sans-serif;\
             text-align:center;max-width:80%\">{text}</div></body></html>"
        ),
    )
    .unwrap();
}

/// Renders a vertical project (540×960, 30 fps) and returns its QA report.
fn render(dir: &Path, name: &str, assets: &str, main: &str, layers: &str, ocr: &str) -> QaReport {
    let p = dir.join(format!("{name}.json5"));
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: '{name}', target: 'custom', width: 540, height: 960, fps: 30, brand_bumpers: false,
              watermark: false, qa: {{ocr: '{ocr}'}},
              assets: {{body: {{path: 'body.mp4'}}, vo: {{path: 'voice.wav', role: 'voice'}}, {assets}}},
              main: [{main}],
              layers: [{{type: 'audio', asset: 'vo', role: 'voice', start: 0}}, {layers}],
              audio: {{voice_chain: 'raw'}}}}"
        ),
    )
    .unwrap();
    let l = pipeline::load(&p).unwrap();
    render_all(&l, &RenderOptions { jobs: 3, ..Default::default() }, &[]).unwrap().remove(0).qa
}

fn sources(tc: &cutline_deps::Toolchain, dir: &Path) {
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "testsrc2=s=540x960:r=30:d=3", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(dir.join("body.mp4"));
    gen::run(c).unwrap();
    // speech starts at 1.2 s
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=48000:cl=stereo:d=1.2",
        "-f",
        "lavfi",
        "-i",
        "sine=f=330:d=5.4:sample_rate=48000",
    ])
    .args(["-filter_complex", "[1]aformat=channel_layouts=stereo[s];[0][s]concat=n=2:v=0:a=1"])
    .arg(dir.join("voice.wav"));
    gen::run(c).unwrap();
}

/// Is text recognition available here (Vision on macOS, Tesseract elsewhere)?
fn ocr_available() -> bool {
    cutline_qa::ocr::resolve(cutline_core::model::OcrEngine::Auto).is_ok()
}

#[test]
fn a_visible_dark_title_card_is_not_black_and_counts_as_the_hook() {
    let Some(tc) = test_toolchain() else { return };
    if cutline_html::chrome().is_none() {
        eprintln!("SKIP: no Chrome");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    sources(tc, d.path());
    card(d.path(), "title.html", "#141211", "He quoted half a verse.");
    let caption = "{type: 'text', template: 'lower_third', text: 'first caption', start: 1.34, end: 3.0}";

    // off-white title on #141211 for 3.6 s, speech at 1.2 s, first caption at 1.34 s
    let r = render(
        d.path(),
        "title",
        "card: {path: 'title.html', duration: 3.6}",
        "{id: 'card', clip: 'card'}, {id: 'body', clip: 'body'}",
        caption,
        "auto",
    );
    let black = finding(&r, "black_frames");
    assert!(black.ok, "a visible title card is not a black span: {black:?}");
    assert!(black.measured["dark_with_content"][0]["evidence"].is_array(), "the evidence is reported: {black:?}");
    let hook = finding(&r, "hook");
    if ocr_available() {
        assert!(hook.ok, "{hook:?}");
        assert_eq!(hook.message, "text on screen at 0.50s (recognised)");
    } else {
        assert!(!hook.ok && hook.message.ends_with("(text in HTML isn't checked)"), "{hook:?}");
    }
    // with recognition off the warning stays and says why
    let r = render(
        d.path(),
        "title_off",
        "card: {path: 'title.html', duration: 3.6}",
        "{id: 'card', clip: 'card'}, {id: 'body', clip: 'body'}",
        caption,
        "off",
    );
    let hook = finding(&r, "hook");
    assert!(!hook.ok, "{hook:?}");
    assert_eq!(hook.message, "first caption/text at 1.33s (caption and text layers) (text in HTML isn't checked)");
    assert!(finding(&r, "black_frames").ok, "the variance alone clears a visible card");

    // the same opening, solid black: a real black span and a real hook warning
    let r = render(
        d.path(),
        "black",
        "card: {path: 'color:#000000', duration: 3.6}",
        "{id: 'card', clip: 'card'}, {id: 'body', clip: 'body'}",
        caption,
        "auto",
    );
    let black = finding(&r, "black_frames");
    assert!(!black.ok, "{black:?}");
    assert_eq!(black.measured[0][0].as_f64(), Some(0.0));
    let hook = finding(&r, "hook");
    assert!(!hook.ok, "{hook:?}");
    assert_eq!(hook.message, "first caption/text at 1.33s (caption and text layers)");
}

/// TASK-honest-cedar-A73E step D: a 3 s #0D0D0F HTML card with "He quoted half a verse." from
/// frame 0 has no hook and no black-frames warning; a truly black 1 s gap between two clips still
/// gives `black_frames`.
#[test]
fn near_black_card_versus_a_black_gap() {
    let Some(tc) = test_toolchain() else { return };
    if cutline_html::chrome().is_none() {
        eprintln!("SKIP: no Chrome");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    sources(tc, d.path());
    card(d.path(), "verse.html", "#0D0D0F", "He quoted half a verse.");
    let caption = "{type: 'text', template: 'lower_third', text: 'first caption', start: 1.32, end: 2.5}";
    let r = render(
        d.path(),
        "verse",
        "card: {path: 'verse.html', duration: 3}",
        "{id: 'card', clip: 'card'}, {id: 'body', clip: 'body'}",
        caption,
        "auto",
    );
    assert!(finding(&r, "black_frames").ok, "{:?}", finding(&r, "black_frames"));
    if ocr_available() {
        assert!(finding(&r, "hook").ok, "{:?}", finding(&r, "hook"));
    }

    let r = render(
        d.path(),
        "gap",
        "gap: {path: 'color:#000000', duration: 1}",
        "{id: 'a', clip: 'body'}, {id: 'gap', clip: 'gap'}, {id: 'b', clip: 'body'}",
        "{type: 'text', template: 'lower_third', text: 'hello there', start: 0.2, end: 2.5}",
        "auto",
    );
    let black = finding(&r, "black_frames");
    assert!(!black.ok, "a black gap is still a black span: {black:?}");
    let s = black.measured[0][0].as_f64().unwrap();
    assert!((s - 3.0).abs() < 0.1, "{black:?}");
}
