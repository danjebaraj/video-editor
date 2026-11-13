//! `profile: screencast` + `chroma: "444"`: 4:4:4 High 4:4:4 output that QA accepts, no freeze
//! warnings for still screens, and `zoom_to` filling the view with its rect.

use cutline_qa::deliver::render_all;
use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::read_frames;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

fn project(d: &Path, name: &str, extra: &str) -> std::path::PathBuf {
    let p = d.join(format!("{name}.json5"));
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: '{name}', target: 'custom', width: 640, height: 360, fps: 30,
              assets: {{s: {{path: 'screen.mp4'}}}},
              main: [{{id: 'c', clip: 's', effects: [{{type: 'zoom_to', rect: {{x: 0.5, y: 0.3, w: 0.4, h: 0.4}}, padding: 0, start: 0.5, duration: 0.6}}]}}],
              audio: {{voice_chain: 'raw'}} {extra}}}"
        ),
    )
    .unwrap();
    p
}

fn red_share(img: &image::RgbImage) -> f64 {
    let red = img.pixels().filter(|p| p[0] > 160 && p[1] < 90 && p[2] < 90).count();
    red as f64 / (img.width() * img.height()) as f64
}

#[test]
fn screencast_profile_444_and_zoom_to() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // a still "screen": gray with a red panel exactly at the zoom_to rect, plus a tone
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        "color=c=0x808080:s=640x360:r=30:d=4,drawbox=x=320:y=108:w=256:h=144:color=0xff0000:t=fill",
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:d=4",
        "-c:v",
        "libx264",
        "-crf",
        "10",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
    ])
    .arg(d.path().join("screen.mp4"));
    gen::run(c).unwrap();
    let opts = RenderOptions { jobs: 3, ..Default::default() };

    let sc = pipeline::load(&project(d.path(), "sc", ", profile: 'screencast', chroma: '444'")).unwrap();
    let r = render_all(&sc, &opts, None).unwrap().remove(0);
    let codec = r.qa.findings.iter().find(|f| f.check == "container_codec").unwrap();
    assert!(codec.ok, "{codec:?}");
    assert_eq!(codec.measured["pix_fmt"], "yuv444p");
    assert_eq!(codec.measured["profile"], "High 4:4:4 Predictive");
    let freeze = r.qa.findings.iter().find(|f| f.check == "freeze").unwrap();
    assert!(freeze.ok, "screencasts may hold still: {freeze:?}");
    // zoom_to: before the zoom the panel is 16 % of the frame; after it, the whole frame
    let f = read_frames(tc, &r.output, &[0, 60]).unwrap();
    assert!((red_share(&f[0]) - 0.16).abs() < 0.03, "before: {}", red_share(&f[0]));
    assert!(red_share(&f[1]) > 0.93, "after: {}", red_share(&f[1]));

    // the same edit without the profile: 4:2:0 High, and the still screen is a QA warning
    let std = pipeline::load(&project(d.path(), "std", "")).unwrap();
    let r = render_all(&std, &opts, None).unwrap().remove(0);
    let codec = r.qa.findings.iter().find(|f| f.check == "container_codec").unwrap();
    assert!(codec.ok && codec.measured["pix_fmt"] == "yuv420p", "{codec:?}");
    let freeze = r.qa.findings.iter().find(|f| f.check == "freeze").unwrap();
    assert!(!freeze.ok, "a still camera clip is a freeze warning: {freeze:?}");
}

#[test]
fn zoom_to_validation() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("b.png"), 64, 64, "0x000000").unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, assets: {b: {path: 'b.png'}}, main: [{id: 'c', clip: 'b', duration: 3, effects: [
            {type: 'zoom_to', rect: {x: 0.9, y: 0.1, w: 0.3, h: 0.1}},
            {type: 'zoom_to', rect: {x: 0.1, y: 0.1, w: 0.05, h: 0.05}},
            {type: 'zoom_to', rect: {x: 0.1, y: 0.1, w: 0.5, h: 0.5}, start: 1, duration: 1, end: 1.5}]}]}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    let has = |code: &str, path: &str| l.diags.iter().any(|x| x.code == code && x.path == path);
    assert!(has("E_BAD_VALUE", "/main/0/effects/0/rect"), "{:?}", l.diags);
    assert!(has("W_ZOOM_CAPPED", "/main/0/effects/1/rect"), "{:?}", l.diags);
    assert!(has("E_BAD_VALUE", "/main/0/effects/2/end"), "{:?}", l.diags);
}
