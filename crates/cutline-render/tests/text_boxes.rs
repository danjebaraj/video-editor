//! Text template boxes (used by QA safe-zone checks and review) match the ink libass draws,
//! at full size and at the 540p draft size.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::read_frames;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;

#[test]
fn lower_third_box_matches_rendered_ink() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("b.png"), 1920, 1080, "0x000000").unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, name: 'lt', target: 'long_1080p', fps: 30, assets: {b: {path: 'b.png'}},
          main: [{id: 'c', clip: 'b', duration: 3}],
          layers: [{type: 'text', template: 'lower_third', text: 'Bible reader', subtitle: 'Picks up right where you left off', start: 0, duration: 3}],
          audio: {voice_chain: 'raw'}}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    for draft in [false, true] {
        let (name, e) = pipeline::exports(&l.project).remove(0);
        let pr = pipeline::prepare(&l, name, &e, draft).unwrap();
        let b = pr.extras.text_boxes[0].clone();
        let opts = RenderOptions { jobs: 3, draft, ..Default::default() };
        let (plan, _) = pipeline::render(&l, &pr, &opts).unwrap();
        let k = plan.geometry.width as f64 / 1920.0;
        let f = &read_frames(tc, &plan.output, &[60]).unwrap()[0];
        // white text (the accent bar is colored and excluded by the threshold)
        let ink: Vec<(u32, u32)> = f
            .enumerate_pixels()
            .filter(|(_, _, p)| p[0] > 150 && p[1] > 150 && p[2] > 150)
            .map(|(x, y, _)| (x, y))
            .collect();
        let (x0, x1) = (ink.iter().map(|p| p.0).min().unwrap() as f64, ink.iter().map(|p| p.0).max().unwrap() as f64);
        let (y0, y1) = (ink.iter().map(|p| p.1).min().unwrap() as f64, ink.iter().map(|p| p.1).max().unwrap() as f64);
        let slack = 4.0; // anti-aliasing + shadow, in output px
        assert!(
            x0 >= b.x * k - slack && x1 <= (b.x + b.w) * k + slack,
            "draft={draft}: ink x {x0}..{x1} vs box {:.1}..{:.1}",
            b.x * k,
            (b.x + b.w) * k
        );
        assert!(
            y0 >= b.y * k - slack && y1 <= (b.y + b.h) * k + slack,
            "draft={draft}: ink y {y0}..{y1} vs box {:.1}..{:.1}",
            b.y * k,
            (b.y + b.h) * k
        );
        // and the box is not padded beyond the text: the right edge is within 2 % of the ink
        assert!(
            ((b.x + b.w) * k - x1).abs() <= 0.02 * b.w * k + slack,
            "draft={draft}: box right {:.1} vs ink {x1}",
            (b.x + b.w) * k
        );
    }
}
