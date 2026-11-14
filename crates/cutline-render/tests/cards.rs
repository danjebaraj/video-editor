//! Business card templates: layout inside the safe zone, reveals, validation, chapters.

use cutline_core::brand::Brand;
use cutline_render::annot::Canvas;
use cutline_render::cards::{self, CardCtx, CardInput};
use cutline_render::pipeline;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn logo(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    image::RgbaImage::from_fn(
        120,
        40,
        |x, _| if x < 60 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) },
    )
    .save(&p)
    .unwrap();
    p
}

fn ctx(dir: &Path) -> CardCtx {
    let mut brand = Brand::default();
    brand.fonts.title = "Inter".into();
    brand.fonts.body = "Inter".into();
    brand.colors.accent = "#3B82F6".into();
    brand.colors.text = "#FFFFFF".into();
    brand.colors.background = "#0B1220".into();
    brand.logo = Some("logo.png".into());
    brand.dir = dir.to_path_buf();
    logo(dir, "logo.png");
    let assets: BTreeMap<String, PathBuf> =
        ["la", "lb", "lc", "photo"].iter().map(|id| (id.to_string(), logo(dir, &format!("{id}.png")))).collect();
    CardCtx::new(&brand, assets).unwrap()
}

/// Every card template with realistic content.
fn samples() -> Vec<(&'static str, Option<&'static str>, Option<&'static str>, Value)> {
    vec![
        ("title_card", Some("Glance"), Some("Every metric your team needs, on one screen"), json!({})),
        ("section_card", Some("Invite your team"), Some("Roles, groups and single sign-on"), json!({"number": 2})),
        (
            "feature_list",
            Some("What is new in 4.2"),
            None,
            json!({"items": ["Shared dashboards", "One-click export to PDF and CSV", "Alerts in Slack and email"]}),
        ),
        ("stat", None, Some("faster month-end close"), json!({"number": 43, "suffix": "%", "source": "Survey, 2026"})),
        (
            "testimonial",
            Some("We replaced four tools with Glance and the whole team finally looks at the same numbers every morning."),
            None,
            json!({"name": "Ada Lovelace", "title": "VP Finance", "company": "Analytical Engines", "photo": "photo", "logo": "lb"}),
        ),
        (
            "compare",
            Some("Before and after"),
            None,
            json!({"left": {"title": "Spreadsheets", "items": ["Manual exports", "Stale by Monday"]},
                   "right": {"title": "Glance", "items": ["Live data", "Always current"]}}),
        ),
        ("logo_wall", Some("Trusted by finance teams at"), None, json!({"logos": ["la", "lb", "lc"]})),
        ("cta_link", Some("Start your free trial"), Some("No credit card needed"), json!({"url": "glance.example.com/start"})),
    ]
}

fn draw(
    c: &CardCtx,
    (w, h): (u32, u32),
    t: &str,
    text: Option<&str>,
    sub: Option<&str>,
    params: &Value,
    at: f64,
    reveal: Vec<f64>,
) -> Canvas {
    let mut pm: Map<String, Value> = params.as_object().cloned().unwrap_or_default();
    // no background: the alpha channel is then exactly the card's ink
    pm.insert("background".into(), json!("none"));
    let safe = if h > w {
        (0.06 * w as f64, 0.10 * h as f64, 0.78 * w as f64, 0.68 * h as f64)
    } else {
        (0.05 * w as f64, 0.05 * h as f64, 0.90 * w as f64, 0.87 * h as f64)
    };
    let mut cv = Canvas::new(w, h);
    cards::draw(
        &mut cv,
        c,
        &CardInput { template: t, text, subtitle: sub, params: &pm, position: None, t: at, len: 4.0, reveal, safe },
    );
    cv
}

fn ink(cv: &Canvas) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..cv.h {
        for x in 0..cv.w {
            if cv.px[((y * cv.w + x) * 4 + 3) as usize] > 24 {
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
fn every_card_stays_inside_the_safe_zone_on_wide_and_vertical_frames() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path());
    for (w, h) in [(960u32, 540u32), (540, 960)] {
        for (t, text, sub, params) in samples() {
            let cv = draw(&c, (w, h), t, text, sub, &params, 2.5, vec![]);
            let (x0, y0, x1, y1) = ink(&cv).unwrap_or_else(|| panic!("{t} drew nothing at {w}x{h}"));
            let (sx, sy, sw, sh) = if h > w {
                (0.06 * w as f64, 0.10 * h as f64, 0.78 * w as f64, 0.68 * h as f64)
            } else {
                (0.05 * w as f64, 0.05 * h as f64, 0.90 * w as f64, 0.87 * h as f64)
            };
            assert!(
                x0 as f64 >= sx - 1.0
                    && y0 as f64 >= sy - 1.0
                    && x1 as f64 <= sx + sw + 1.0
                    && y1 as f64 <= sy + sh + 1.0,
                "{t} at {w}x{h}: ink {:?} outside safe ({sx:.0}, {sy:.0}, {sw:.0}, {sh:.0})",
                (x0, y0, x1, y1)
            );
            // and it is a real block of content, not a sliver
            assert!(
                (x1 - x0) as f64 > 0.2 * sw && (y1 - y0) as f64 > 0.06 * sh,
                "{t} at {w}x{h}: {:?}",
                (x0, y0, x1, y1)
            );
        }
    }
}

#[test]
fn cards_fade_in_and_feature_list_items_build_at_their_reveal_times() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path());
    let items = json!({"items": ["One", "Two", "Three"]});
    let h_at = |t: f64| {
        ink(&draw(&c, (960, 540), "feature_list", None, None, &items, t, vec![0.5, 1.5, 2.5])).map(|b| b.3 - b.1)
    };
    assert!(h_at(0.2).is_none(), "nothing before the first reveal");
    let (h1, h2, h3) = (h_at(1.2).unwrap(), h_at(2.2).unwrap(), h_at(3.3).unwrap());
    assert!(h2 > h1 + 20 && h3 > h2 + 20, "items build: {h1} {h2} {h3}");
    // stat counts up to its number and holds its size while counting
    let stat = json!({"number": 1250, "prefix": "$"});
    let w_at = |t: f64| ink(&draw(&c, (960, 540), "stat", None, None, &stat, t, vec![])).map(|b| b.2 - b.0).unwrap();
    assert!(w_at(0.3) < w_at(2.0), "fewer digits early: {} vs {}", w_at(0.3), w_at(2.0));
    assert_eq!(w_at(2.0), w_at(3.0), "settled");
}

#[test]
fn card_params_are_validated_and_section_cards_make_chapters() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::still(tc, &d.path().join("bg.png"), 320, 180, "0x101010").unwrap();
    let write = |layers: &str| {
        let p = d.path().join("p.json5");
        std::fs::write(
            &p,
            format!(
                "{{version: 1, target: 'custom', width: 320, height: 180, fps: 30, assets: {{bg: {{path: 'bg.png'}}, plain: {{path: 'color:#224466'}}}},
                  main: [{{id: 'a', clip: 'bg', duration: 30}}, {{id: 'b', clip: 'plain', duration: 30}}], layers: [{layers}]}}"
            ),
        )
        .unwrap();
        p
    };
    let l = pipeline::load(&write(
        "{type: 'text', template: 'feature_list', params: {items: []}},
         {type: 'text', template: 'cta_link', text: 'Go'},
         {type: 'text', template: 'logo_wall', params: {logos: ['bg', 'nope']}},
         {type: 'text', template: 'compare', params: {left: {title: 'A', items: ['x']}}},
         {type: 'text', template: 'stat', params: {number: 'many'}},
         {type: 'text', template: 'title_card'}",
    ))
    .unwrap();
    let has = |code: &str, path: &str| l.diags.iter().any(|x| x.code == code && x.path == path);
    assert!(has("E_BAD_VALUE", "/layers/0/params/items"), "{:?}", l.diags);
    assert!(has("E_MISSING_FIELD", "/layers/1/params/url"), "{:?}", l.diags);
    assert!(has("E_UNKNOWN_ASSET", "/layers/2/params/logos/1"), "{:?}", l.diags);
    assert!(has("E_BAD_VALUE", "/layers/3/params/right"), "{:?}", l.diags);
    assert!(has("E_MISSING_FIELD", "/layers/4/params/number"), "{:?}", l.diags);
    assert!(has("E_MISSING_FIELD", "/layers/5/text"), "{:?}", l.diags);

    // section cards start chapters; a plain-color asset is a usable clip
    let l = pipeline::load(&write(
        "{type: 'text', template: 'section_card', text: 'Set up', start: 12, duration: 3},
         {type: 'text', template: 'section_card', text: 'Share', start: 30, duration: 3}",
    ))
    .unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    assert_eq!(l.timeline.as_ref().unwrap().total_frames, 1800);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, true).unwrap();
    let (ch, _) = cutline_qa_free_chapters(&pr);
    assert_eq!(ch, vec![(0.0, "Intro".to_string()), (12.0, "Set up".to_string()), (30.0, "Share".to_string())]);
}

/// The section-card chapters, without depending on the qa crate from here.
fn cutline_qa_free_chapters(pr: &pipeline::Prepared) -> (Vec<(f64, String)>, ()) {
    let tl = &pr.timeline;
    let mut v: Vec<(f64, String)> = tl
        .layers
        .iter()
        .filter_map(|l| match &l.layer {
            cutline_core::model::Layer::Text { template, text, .. } if template == "section_card" => {
                Some((tl.secs(l.start), text.clone().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    if v.first().is_some_and(|c| c.0 > 0.0) {
        v.insert(0, (0.0, "Intro".into()));
    }
    (v, ())
}
