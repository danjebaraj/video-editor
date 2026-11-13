//! Mask layers hide a region (fill / blur / pixelate) only inside the rect and only while active.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::read_frames;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

fn render(project: &Path) -> PathBuf {
    let l = pipeline::load(project).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let opts = RenderOptions { jobs: 3, ..Default::default() };
    pipeline::render(&l, &pr, &opts).unwrap().0.output
}

/// Mean and variance of luma-ish green over a pixel rect.
fn stats(img: &image::RgbImage, (x, y, w, h): (u32, u32, u32, u32)) -> (f64, f64) {
    let v: Vec<f64> = (y..y + h)
        .flat_map(|yy| (x..x + w).map(move |xx| (xx, yy)))
        .map(|(xx, yy)| img.get_pixel(xx, yy)[1] as f64)
        .collect();
    let m = v.iter().sum::<f64>() / v.len() as f64;
    (m, v.iter().map(|a| (a - m).powi(2)).sum::<f64>() / v.len() as f64)
}

#[test]
fn masks_hide_only_their_rect_and_span() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // 4 px checkerboard: maximal detail everywhere
    gen::grid_video(tc, &d.path().join("g.mp4"), 640, 360, 30, 3.0, 4).unwrap();
    // rect = x 0.25–0.75, y 0.25–0.75 → pixels 160..480 × 90..270
    let rect = "{x: 0.25, y: 0.25, w: 0.5, h: 0.5}";
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        format!(
            "{{version: 1, name: 'm', target: 'custom', width: 640, height: 360, fps: 30, assets: {{g: {{path: 'g.mp4'}}}},
              main: [{{id: 'c', clip: 'g', in: 0, out: 3}}],
              layers: [{{type: 'mask', rect: {rect}, mode: 'fill', color: '#FF0000', start: 0.5, end: 1.0}},
                       {{type: 'mask', rect: {rect}, start: 1.0, end: 1.5}},
                       {{type: 'mask', rect: {rect}, mode: 'pixelate', start: 1.5, end: 2.0}}],
              audio: {{voice_chain: 'raw'}}}}"
        ),
    )
    .unwrap();
    let out = render(&pp);
    // frames 0 (none), 22 (fill), 37 (blur), 52 (pixelate), 75 (none again)
    let f = read_frames(tc, &out, &[0, 22, 37, 52, 75]).unwrap();
    let inner = (180, 110, 280, 140);
    let outside = (20, 20, 100, 50);
    let (_, v_detail) = stats(&f[0], inner);
    assert!(v_detail > 2000.0, "source is detailed ({v_detail})");
    // fill: solid red inside
    let p = f[1].get_pixel(320, 180);
    assert!(p[0] > 220 && p[1] < 40 && p[2] < 40, "fill color {p:?}");
    // blur: detail gone inside
    let (_, v_blur) = stats(&f[2], inner);
    assert!(v_blur < v_detail * 0.02, "blur variance {v_blur} vs {v_detail}");
    // pixelate: blocks of constant color, so far less detail than the checkerboard
    let (_, v_pix) = stats(&f[3], inner);
    assert!(v_pix < v_detail * 0.5, "pixelate variance {v_pix} vs {v_detail}");
    // outside the rect nothing changes, and after the last mask the frame is untouched
    for (i, img) in f.iter().enumerate() {
        let (_, v) = stats(img, outside);
        assert!(v > 2000.0, "frame {i}: detail outside the rect kept ({v})");
    }
    let (_, v_after) = stats(&f[4], inner);
    assert!(v_after > 2000.0, "mask ends at its end time ({v_after})");
}

#[test]
fn mask_rect_outside_the_frame_is_an_error() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(test_toolchain().unwrap(), &d.path().join("b.png"), 64, 64, "0x000000").unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, assets: {b: {path: 'b.png'}}, main: [{id: 'c', clip: 'b'}],
          layers: [{type: 'mask', rect: {x: 0.8, y: 0.1, w: 0.4, h: 0.1}}, {type: 'mask', rect: {x: 0.1, y: 0.1, w: 0.1, h: 0.1}, mode: 'smudge'}]}",
    )
    .unwrap();
    let diags = match pipeline::load(&pp) {
        Ok(l) => l.diags,
        Err(pipeline::LoadError::Invalid(d)) => d,
        Err(e) => panic!("{e}"),
    };
    assert!(diags.iter().any(|x| x.code == "E_UNKNOWN_VARIANT" && x.path.starts_with("/layers/1")), "{diags:?}");
    // with the bad mode fixed, the out-of-frame rect is reported
    std::fs::write(
        &pp,
        "{version: 1, assets: {b: {path: 'b.png'}}, main: [{id: 'c', clip: 'b'}],
          layers: [{type: 'mask', rect: {x: 0.8, y: 0.1, w: 0.4, h: 0.1}}]}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    assert!(l.diags.iter().any(|x| x.code == "E_BAD_VALUE" && x.path == "/layers/0/rect"), "{:?}", l.diags);
}
