//! Brand kit: the logo watermark and the intro/outro bumpers declared in the brand file.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{mean_rgb, read_frames};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

fn setup(d: &Path, brand_extra: &str) {
    let tc = test_toolchain().unwrap();
    std::fs::create_dir_all(d.join("brand")).unwrap();
    gen::still(tc, &d.join("body.png"), 320, 180, "0x808080").unwrap();
    gen::still(tc, &d.join("brand/intro.png"), 320, 180, "0x0000FF").unwrap();
    gen::still(tc, &d.join("brand/outro.png"), 320, 180, "0x00FF00").unwrap();
    gen::still(tc, &d.join("brand/logo.png"), 64, 32, "0xFF0000").unwrap();
    std::fs::write(
        d.join("brand/default.json"),
        format!("{{\"logo\": \"logo.png\", \"watermark\": {{\"opacity\": 1.0, \"scale\": 0.2}}, \"watermark_position\": \"top_right\"{brand_extra}}}"),
    )
    .unwrap();
}

fn project(d: &Path, body: &str) -> PathBuf {
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'b', target: 'custom', width: 320, height: 180, fps: 30,
              assets: {{body: {{path: 'body.png'}}}}, audio: {{voice_chain: 'raw'}}, {body}}}"
        ),
    )
    .unwrap();
    p
}

fn render(project: &Path) -> (PathBuf, cutline_core::Timeline) {
    let l = pipeline::load(project).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let opts = RenderOptions { jobs: 3, ..Default::default() };
    (pipeline::render(&l, &pr, &opts).unwrap().0.output, pr.timeline)
}

#[test]
fn bumpers_wrap_the_edit_and_times_count_from_the_edit() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path(), ", \"intro\": \"intro.png\", \"outro\": \"outro.png\", \"bumper_fade\": 0.4");
    let pp = project(
        d.path(),
        "main: [{id: 'a', clip: 'body', end_at: 2.0}, {id: 'b', clip: 'body', duration: 2}],
         layers: [{type: 'mask', rect: {x: 0, y: 0.5, w: 0.2, h: 0.2}, mode: 'fill', color: '#FFFFFF', start: 1.0, end: 2.0}]",
    );
    let (out, tl) = render(&pp);
    // intro 3 s (stills default) with a 0.4 s fade into the edit: the edit starts at 2.6 s
    assert_eq!(tl.clips[0].id, "brand_intro");
    assert_eq!(tl.clips.last().unwrap().id, "brand_outro");
    assert_eq!(tl.origin, 78);
    // `end_at: 2.0` is on the edit: the cut a→b is 2 s after the origin
    assert_eq!(tl.clip_by_id("b").unwrap().start, 78 + 60);
    assert_eq!(tl.body_end, 78 + 120);
    // the layer's 1.0–2.0 s are edit seconds
    assert_eq!((tl.layers[0].start, tl.layers[0].end), (78 + 30, 78 + 60));
    let last = (tl.total_frames - 2) as u64;
    let f = read_frames(tc, &out, &[10, 78 + 45, 78 + 90, last]).unwrap();
    let logo = (255, 12, 40, 14); // top-right inside the 5 % safe zone, 64 px wide
                                  // intro: blue, no logo
    let c = mean_rgb(&f[0], 100, 60, 100, 60);
    assert!(c[2] > 200.0 && c[0] < 40.0, "intro frame {c:?}");
    let c = mean_rgb(&f[0], logo.0, logo.1, logo.2, logo.3);
    assert!(c[0] < 40.0, "no watermark over the intro {c:?}");
    // edit: grey with the red logo, and the mask only during its span
    for (i, masked) in [(1, true), (2, false)] {
        let c = mean_rgb(&f[i], logo.0, logo.1, logo.2, logo.3);
        assert!(c[0] > 200.0 && c[1] < 60.0, "watermark on the edit, frame {i}: {c:?}");
        let c = mean_rgb(&f[i], 140, 60, 40, 20);
        assert!((c[0] - 128.0).abs() < 12.0 && (c[1] - 128.0).abs() < 12.0, "edit frame {i}: {c:?}");
        let m = mean_rgb(&f[i], 8, 95, 40, 25);
        assert_eq!(m[0] > 230.0, masked, "mask at frame {i}: {m:?}");
    }
    // outro: green, no logo
    let c = mean_rgb(&f[3], 100, 60, 100, 60);
    assert!(c[1] > 200.0 && c[0] < 40.0, "outro frame {c:?}");
    let c = mean_rgb(&f[3], logo.0, logo.1, logo.2, logo.3);
    assert!(c[0] < 40.0, "no watermark over the outro {c:?}");
}

#[test]
fn bumpers_and_watermark_can_be_turned_off() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path(), ", \"intro\": \"intro.png\"");
    let pp = project(d.path(), "brand_bumpers: false, watermark: false, main: [{id: 'a', clip: 'body', duration: 1}]");
    let l = pipeline::load(&pp).unwrap();
    let tl = l.timeline.as_ref().unwrap();
    assert_eq!((tl.clips.len(), tl.origin, tl.total_frames), (1, 0, 30));
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    assert!(pr.extras.watermark.is_none());
    // and on by default
    let pp = project(d.path(), "main: [{id: 'a', clip: 'body', duration: 1}]");
    let l = pipeline::load(&pp).unwrap();
    assert_eq!(l.timeline.as_ref().unwrap().clips.len(), 2);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    assert!(pipeline::prepare(&l, name, &e, false).unwrap().extras.watermark.is_some());
}
