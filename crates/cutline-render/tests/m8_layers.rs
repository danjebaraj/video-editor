//! HTML assets as alpha layers and as main clips, HTML text templates, params injection.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{mean_rgb, read_frames};
use cutline_testkit::test_toolchain;

#[test]
fn html_layer_main_clip_and_template() {
    let Some(tc) = test_toolchain() else { return };
    if cutline_html::chrome().is_none() {
        eprintln!("SKIP: no Chrome");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    cutline_testkit::gen::still(tc, &d.path().join("bg.png"), 640, 360, "0x204080").unwrap();
    // overlay: a box positioned from params (proves __cutline_params injection)
    std::fs::write(
        d.path().join("box.html"),
        r#"<!doctype html><html><body style="margin:0;background:transparent"><div id="b" style="position:absolute;top:100px;width:80px;height:80px;background:#ff0000"></div>
<script>document.getElementById('b').style.left = window.__cutline_params.x + 'px';</script></body></html>"#,
    )
    .unwrap();
    // main clip: opaque green page
    std::fs::write(
        d.path().join("card.html"),
        r#"<!doctype html><html><body style="margin:0;background:#00c000"></body></html>"#,
    )
    .unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, name: 'h', target: 'custom', width: 640, height: 360, fps: 30,
          assets: {bg: {path: 'bg.png'}, box: {path: 'box.html', duration: 2, params: {x: 300}}, card: {path: 'card.html', duration: 1}},
          main: [{id: 'a', clip: 'bg', duration: 2}, {id: 'b', clip: 'card'}],
          layers: [{type: 'html', asset: 'box', start: 0, duration: 2},
                   {type: 'text', template: 'lower_third_html', text: 'Name', subtitle: 'Role', start: 0.5, duration: 1.5}]}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let (plan, _) = pipeline::render(&l, &pr, &RenderOptions::default()).unwrap();
    let f = read_frames(tc, &plan.output, &[15, 45, 75]).unwrap();
    // box at x=300..380, y=100..180 over the blue background
    let inside = mean_rgb(&f[0], 320, 120, 40, 40);
    let outside = mean_rgb(&f[0], 20, 20, 40, 40);
    assert!(inside[0] > 200.0 && inside[1] < 60.0, "box {inside:?}");
    assert!(outside[2] > 100.0 && outside[0] < 60.0, "background {outside:?}");
    // lower third: bright text pixels appear in the lower-left band at 1.5 s (none at 0.5 s)
    let bright = |img: &image::RgbImage| {
        (0..320u32)
            .flat_map(|x| (200..360u32).map(move |y| (x, y)))
            .filter(|(x, y)| img.get_pixel(*x, *y).0.iter().all(|c| *c > 200))
            .count()
    };
    assert!(bright(&f[1]) > 50, "lower third not visible ({} bright px)", bright(&f[1]));
    assert_eq!(bright(&f[0]), 0);
    // HTML main clip (green) after 2 s
    let g = mean_rgb(&f[2], 200, 100, 100, 100);
    assert!(g[1] > 150.0 && g[0] < 60.0, "html main clip {g:?}");
}
