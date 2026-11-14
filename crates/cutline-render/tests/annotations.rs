//! Annotation layers: highlight, spotlight, arrow, keys, cursor.

use cutline_core::model::AssetKind;
use cutline_core::time::Fps;
use cutline_core::timeline::{resolve, Context, MediaFacts, StaticLookup, Timeline};
use cutline_render::annot::{self, Canvas, Style};
use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{mean_rgb, read_frames};
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

fn project(d: &Path, main: &str, layers: &str) -> PathBuf {
    let tc = test_toolchain().unwrap();
    gen::still(tc, &d.join("bg.png"), 640, 360, "0x808080").unwrap();
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'a', target: 'custom', width: 640, height: 360, fps: 30,
              assets: {{bg: {{path: 'bg.png'}}}}, audio: {{voice_chain: 'raw'}},
              main: [{main}], layers: [{layers}]}}"
        ),
    )
    .unwrap();
    p
}

fn is_accent(c: [f64; 3]) -> bool {
    c[0] > 215.0 && (c[1] - 196.0).abs() < 35.0 && c[2] < 60.0
}
fn is_grey(c: [f64; 3], v: f64) -> bool {
    c.iter().all(|x| (x - v).abs() < 12.0)
}

#[test]
fn highlight_and_spotlight_mark_their_rect_for_their_span() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // rect = pixels 160..480 × 90..270
    let rect = "{x: 0.25, y: 0.25, w: 0.5, h: 0.5}";
    let pp = project(
        d.path(),
        "{id: 'c', clip: 'bg', duration: 4}",
        &format!(
            "{{type: 'highlight', rect: {rect}, start: 0.5, end: 1.5, enter: 'none'}},
             {{type: 'spotlight', rect: {rect}, start: 2.0, end: 3.0, dim: 0.6}},
             {{type: 'highlight', rect: {rect}, style: 'fill', color: '#FF0000', start: 3.2, end: 3.8, enter: 'none'}}"
        ),
    );
    let out = render(&pp);
    // frames: 5 (nothing), 30 (outline), 75 (spotlight), 105 (fill)
    let f = read_frames(tc, &out, &[5, 30, 75, 105]).unwrap();
    let (inside, outside) = ((300, 170, 40, 20), (20, 20, 60, 40));
    // the 4 px stroke sits just outside the rect: x 154..158 at mid height
    let stroke = (155, 170, 2, 20);
    assert!(is_grey(mean_rgb(&f[0], stroke.0, stroke.1, stroke.2, stroke.3), 128.0), "nothing before the start");
    let c = mean_rgb(&f[1], stroke.0, stroke.1, stroke.2, stroke.3);
    assert!(is_accent(c), "outline in the brand accent: {c:?}");
    assert!(is_grey(mean_rgb(&f[1], inside.0, inside.1, inside.2, inside.3), 128.0), "outline leaves the inside alone");
    assert!(is_grey(mean_rgb(&f[1], outside.0, outside.1, outside.2, outside.3), 128.0));
    // spotlight: 60 % dim outside (128 → ~51), untouched inside
    let c = mean_rgb(&f[2], outside.0, outside.1, outside.2, outside.3);
    assert!(is_grey(c, 51.0), "dimmed outside: {c:?}");
    assert!(is_grey(mean_rgb(&f[2], inside.0, inside.1, inside.2, inside.3), 128.0), "lit inside");
    assert!(is_grey(mean_rgb(&f[2], stroke.0, stroke.1, stroke.2, stroke.3), 128.0), "the outline has ended");
    // fill: a red tint inside only
    let c = mean_rgb(&f[3], inside.0, inside.1, inside.2, inside.3);
    assert!(c[0] > 150.0 && c[1] < 110.0, "fill tint: {c:?}");
    assert!(is_grey(mean_rgb(&f[3], outside.0, outside.1, outside.2, outside.3), 128.0));
}

#[test]
fn annotations_follow_the_clip_zoom() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let rect = "{x: 0.25, y: 0.25, w: 0.5, h: 0.5}";
    let pp = project(
        d.path(),
        &format!("{{id: 'c', clip: 'bg', duration: 3, effects: [{{type: 'zoom_to', rect: {rect}, start: 0.5, duration: 0.5}}]}}"),
        &format!(
            "{{type: 'highlight', rect: {rect}, start: 0, end: 3, enter: 'none', width: 6}},
             {{type: 'highlight', rect: {{x: 0.05, y: 0.8, w: 0.1, h: 0.1}}, color: '#00FF00', start: 0, end: 3, enter: 'none', follow: false, width: 6}}"
        ),
    );
    let out = render(&pp);
    let f = read_frames(tc, &out, &[5, 60]).unwrap();
    // before the zoom the 6 px stroke is at x 153..159
    assert!(is_accent(mean_rgb(&f[0], 154, 170, 4, 20)), "{:?}", mean_rgb(&f[0], 154, 170, 4, 20));
    // zoomed 1/0.56 = 1.786× about the centre: the rect's left edge moves to (0.25 − 0.22)·1.786·640 = 34.3 px
    let (s, _) =
        cutline_core::model::zoom_to_params(&cutline_core::model::Rect { x: 0.25, y: 0.25, w: 0.5, h: 0.5 }, 0.03, 3.0);
    let edge = (0.25 - (1.0 - 1.0 / s) / 2.0) * s * 640.0;
    let x = (edge - 5.0).round() as u32;
    let c = mean_rgb(&f[1], x, 170, 3, 20);
    assert!(is_accent(c), "outline follows the zoom to x≈{edge:.1}: {c:?}");
    assert!(!is_accent(mean_rgb(&f[1], 154, 170, 4, 20)), "and has left its unzoomed place");
    // `follow: false` keeps screen coordinates: left edge stroke at x 26..32 in both frames
    for (i, fr) in f.iter().enumerate() {
        let c = mean_rgb(fr, 27, 300, 3, 10);
        assert!(c[1] > 200.0 && c[0] < 60.0, "fixed highlight in frame {i}: {c:?}");
    }
}

#[test]
fn annotation_rects_must_be_inside_the_frame() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let pp = project(
        d.path(),
        "{id: 'c', clip: 'bg', duration: 2}",
        "{type: 'highlight', rect: {x: 0.8, y: 0.1, w: 0.4, h: 0.1}}, {type: 'arrow', from: {x: 0.1, y: 0.1}, to: {x: 1.2, y: 0.5}},
         {type: 'keys', keys: []}, {type: 'cursor', path: [{t: 1, x: 0.5, y: 0.5}, {t: 0.5, x: 0.6, y: 0.5}]}",
    );
    let l = pipeline::load(&pp).unwrap();
    let has = |code: &str, path: &str| l.diags.iter().any(|x| x.code == code && x.path == path);
    assert!(has("E_RECT_OUT_OF_FRAME", "/layers/0/rect"), "{:?}", l.diags);
    assert!(has("E_RECT_OUT_OF_FRAME", "/layers/1/to"), "{:?}", l.diags);
    assert!(has("E_BAD_VALUE", "/layers/2/keys"), "{:?}", l.diags);
    assert!(has("E_BAD_VALUE", "/layers/3/path/1/t"), "{:?}", l.diags);
}

fn timeline(layers: &str) -> Timeline {
    let mut m = StaticLookup::default();
    m.0.insert(
        "v".into(),
        MediaFacts {
            exists: true,
            kind: AssetKind::Video,
            duration: 20.0,
            has_video: true,
            has_audio: false,
            alpha: false,
            width: 640,
            height: 360,
            fps: Some(Fps::new(30, 1)),
            rotation: 0,
        },
    );
    let p = cutline_core::parse::parse_project(&format!(
        "{{version: 1, target: 'custom', width: 640, height: 360, fps: 30, assets: {{v: {{path: 'v.mp4'}}}},
           main: [{{id: 'c', clip: 'v', out: 10}}], layers: [{layers}]}}"
    ))
    .unwrap();
    let (tl, d) = resolve(&p, &m, &Context::default());
    tl.unwrap_or_else(|| panic!("{d:?}"))
}

fn draw(tl: &Timeline, fr: i64) -> Canvas {
    let font = cutline_text::FontDb::bundled().ok().and_then(|db| db.get("Inter", false).map(|f| f.font.clone()));
    let st = Style {
        accent: [255, 196, 0],
        safe: (32.0, 18.0, 576.0, 313.0),
        k: 1.0,
        font,
        cards: None,
        look_key: String::new(),
    };
    let layers: Vec<_> = tl.layers.iter().collect();
    let mut cv = Canvas::new(640, 360);
    annot::draw_frame(&mut cv, tl, &layers, &st, fr);
    cv
}

fn px(cv: &Canvas, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * cv.w + x) * 4) as usize;
    [cv.px[i], cv.px[i + 1], cv.px[i + 2], cv.px[i + 3]]
}

/// Bounding box of every pixel with alpha.
fn ink(cv: &Canvas) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..cv.h {
        for x in 0..cv.w {
            if px(cv, x, y)[3] > 20 {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    b
}

#[test]
fn cursor_arrow_and_keys_draw_where_expected() {
    // cursor: moves from (64, 36) to (320, 180) in 1 s, clicks on arrival
    let tl =
        timeline("{type: 'cursor', path: [{t: 0, x: 0.1, y: 0.1}, {t: 1, x: 0.5, y: 0.5, click: true}], start: 1}");
    assert_eq!((tl.layers[0].start, tl.layers[0].end), (30, 78), "ends 0.6 s after the last key");
    let cv = draw(&tl, 31);
    let (x0, y0, x1, y1) = ink(&cv).unwrap();
    assert!(x0 <= 66 && y0 <= 38 && x1 < 100 && y1 < 80, "pointer near its first key: {:?}", (x0, y0, x1, y1));
    // inside the pointer body: dark, opaque
    let cv = draw(&tl, 62);
    let p = px(&cv, 323, 190);
    assert!(p[3] > 240 && p[0] < 60, "pointer body at the click point: {p:?}");
    // the ripple ring in the accent, spreading around the click
    let b = ink(&cv).unwrap();
    assert!(b.0 < 310 && b.1 < 170, "ripple spreads around the click: {b:?}");
    let ring = (0..360).any(|y| {
        (0..640).any(|x| {
            let p = px(&cv, x, y);
            p[3] > 60 && p[0] > 200 && p[1] > 150 && p[2] < 80
        })
    });
    assert!(ring, "accent ripple");

    // arrow: accent along the shaft, label text behind the tail
    let tl = timeline("{type: 'arrow', from: {x: 0.5, y: 0.5}, to: {x: 0.8, y: 0.5}, label: 'New', start: 0, end: 3}");
    let cv = draw(&tl, 60);
    let p = px(&cv, 400, 180);
    assert!(p[3] > 240 && p[0] > 230 && p[2] < 40, "shaft: {p:?}");
    let (x0, _, x1, _) = ink(&cv).unwrap();
    assert!(x1 >= 508 && x1 <= 516, "tip at x=512: {x1}");
    assert!(x0 < 300, "label to the left of the tail: {x0}");
    let white = (170..190).any(|y| (220..320).any(|x| px(&cv, x, y)[..3].iter().all(|c| *c > 235)));
    assert!(white, "label text");

    // keys: two caps centred on the bottom of the safe zone
    let tl = timeline("{type: 'keys', keys: ['cmd', 'K'], start: 0, end: 3}");
    let cv = draw(&tl, 45);
    let (x0, y0, x1, y1) = ink(&cv).unwrap();
    assert!((((x0 + x1) / 2) as i64 - 320).abs() <= 3, "centred: {x0}..{x1}");
    assert!(y1 <= 333 && y1 >= 326 && y1 - y0 >= 60, "on the safe zone's bottom: {y0}..{y1}");
    assert!(x1 - x0 > 130 && x1 - x0 < 190, "two 64 px caps and a gap: {}", x1 - x0);
}
