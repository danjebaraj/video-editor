//! M1 acceptance: invalid-project suite. Each case asserts its error code and JSON pointer.

use cutline_core::model::AssetKind;
use cutline_core::time::Fps;
use cutline_core::timeline::{resolve, Context, MediaFacts, StaticLookup};

fn vid(d: f64) -> MediaFacts {
    MediaFacts {
        exists: true,
        kind: AssetKind::Video,
        duration: d,
        has_video: true,
        has_audio: true,
        alpha: false,
        width: 1920,
        height: 1080,
        fps: Some(Fps::new(30, 1)),
        rotation: 0,
    }
}

fn lookup() -> StaticLookup {
    let mut m = StaticLookup::default();
    m.0.insert("a".into(), vid(12.0));
    m.0.insert("b".into(), vid(4.0));
    m.0.insert(
        "music".into(),
        MediaFacts { kind: AssetKind::Audio, has_video: false, width: 0, height: 0, fps: None, ..vid(30.0) },
    );
    m.0.insert(
        "logo".into(),
        MediaFacts { kind: AssetKind::Image, alpha: false, duration: 0.04, has_audio: false, ..vid(0.04) },
    );
    m.0.insert("gone".into(), MediaFacts { exists: false, ..vid(0.0) });
    m
}

const ASSETS: &str = r#"assets: { a: {path: "a.mp4"}, b: {path: "b.mp4"}, music: {path: "m.mp3", role: "music"}, logo: {path: "logo.png"} }"#;

/// Returns (code, path) of all errors (warnings included when `warn`).
fn check(body: &str) -> Vec<(String, String)> {
    let text = format!("{{ version: 1, {ASSETS}, {body} }}");
    let p = match cutline_core::parse::parse_project(&text) {
        Ok(p) => p,
        Err(d) => return d.into_iter().map(|x| (x.code, x.path)).collect(),
    };
    let (_, d) = resolve(
        &p,
        &lookup(),
        &Context {
            fonts: vec!["Anton".into(), "Inter".into()],
            sfx_aliases: vec!["whoosh".into()],
            ..Default::default()
        },
    );
    d.into_iter().map(|x| (x.code, x.path)).collect()
}

fn expect(body: &str, code: &str, path: &str) {
    let got = check(body);
    assert!(got.iter().any(|(c, p)| c == code && p == path), "expected {code} at {path}, got {got:?}\nbody: {body}");
}

const OK_MAIN: &str = r#"main: [{id: "c1", clip: "a", in: 0, out: 2}, {id: "c2", clip: "b"}]"#;

#[test]
fn valid_baseline_has_no_errors() {
    let got = check(OK_MAIN);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn e01_parse_error() {
    let d = cutline_core::parse::parse_project("{ version: 1, main: [ }").unwrap_err();
    assert_eq!(d[0].code, "E_PARSE");
}
#[test]
fn e02_unsupported_version() {
    let d = cutline_core::parse::parse_project("{ version: 2 }").unwrap_err();
    assert_eq!((d[0].code.as_str(), d[0].path.as_str()), ("E_UNSUPPORTED_VERSION", "/version"));
}
#[test]
fn e03_unknown_field_clip() {
    expect(r#"main: [{id: "c1", clip: "a", inpoint: 1}]"#, "E_UNKNOWN_FIELD", "/main/0/inpoint");
}
#[test]
fn e04_unknown_field_layer() {
    expect(
        r#"main: [{id: "c1", clip: "a"}], layers: [{type: "image", asset: "logo", colour: "red"}]"#,
        "E_UNKNOWN_FIELD",
        "/layers/0/colour",
    );
}
#[test]
fn e05_missing_field() {
    expect(r#"main: [{clip: "a"}]"#, "E_MISSING_FIELD", "/main/0");
}
#[test]
fn e06_unknown_target() {
    expect(r#"target: "tiktok", main: [{id: "c1", clip: "a"}]"#, "E_UNKNOWN_VARIANT", "/target");
}
#[test]
fn e07_bad_time() {
    expect(r#"main: [{id: "c1", clip: "a", out: "1:xx"}]"#, "E_BAD_TIME", "/main/0/out");
}
#[test]
fn e08_unknown_asset() {
    expect(r#"main: [{id: "c1", clip: "nope"}]"#, "E_UNKNOWN_ASSET", "/main/0/clip");
}
#[test]
fn e09_asset_not_found() {
    let text = r#"{ version: 1, assets: {gone: {path: "x.mp4"}, a: {path: "a.mp4"}}, main: [{id: "c", clip: "a"}] }"#;
    let p = cutline_core::parse::parse_project(text).unwrap();
    let (_, d) = resolve(&p, &lookup(), &Context::default());
    assert!(d.iter().any(|x| x.code == "E_ASSET_NOT_FOUND" && x.path == "/assets/gone/path"), "{d:?}");
}
#[test]
fn e10_out_beyond_duration() {
    expect(r#"main: [{id: "c1", clip: "b", in: 1, out: 9}]"#, "E_TIME_OUT_OF_RANGE", "/main/0/out");
}
#[test]
fn e11_in_beyond_duration() {
    expect(r#"main: [{id: "c1", clip: "b", in: "00:05.000"}]"#, "E_TIME_OUT_OF_RANGE", "/main/0/in");
}
#[test]
fn e12_empty_range() {
    expect(r#"main: [{id: "c1", clip: "a", in: 3, out: 2}]"#, "E_EMPTY_RANGE", "/main/0/out");
}
#[test]
fn e13_duplicate_id() {
    expect(r#"main: [{id: "c1", clip: "a", out: 1}, {id: "c1", clip: "b"}]"#, "E_DUPLICATE_ID", "/main/1/id");
}
#[test]
fn e14_transition_too_long() {
    expect(
        r#"main: [{id: "c1", clip: "a", out: 1}, {transition: {type: "fade", duration: 1.0}}, {id: "c2", clip: "b"}]"#,
        "E_TRANSITION_TOO_LONG",
        "/main/1/transition/duration",
    );
}
#[test]
fn e15_transition_position() {
    expect(
        r#"main: [{transition: {type: "fade", duration: 0.3}}, {id: "c1", clip: "a"}]"#,
        "E_TRANSITION_POSITION",
        "/main/0",
    );
    expect(
        r#"main: [{id: "c1", clip: "a"}, {transition: {type: "fade", duration: 0.3}}]"#,
        "E_TRANSITION_POSITION",
        "/main/1",
    );
}
#[test]
fn e16_unknown_transition() {
    expect(
        r#"main: [{id: "c1", clip: "a", out: 2}, {transition: {type: "spin3d", duration: 0.3}}, {id: "c2", clip: "b"}]"#,
        "E_UNKNOWN_TRANSITION",
        "/main/1/transition/type",
    );
}
#[test]
fn e17_unknown_anchor() {
    expect(
        &format!("{OK_MAIN}, layers: [{{type: \"image\", asset: \"logo\", anchor: \"zzz\"}}]"),
        "E_UNKNOWN_ANCHOR",
        "/layers/0/anchor",
    );
}
#[test]
fn e18_speed_range() {
    expect(r#"main: [{id: "c1", clip: "a", speed: 9}]"#, "E_SPEED_RANGE", "/main/0/speed");
}
#[test]
fn e19_empty_main() {
    expect(r#"main: []"#, "E_EMPTY_MAIN", "/main");
}
#[test]
fn e20_wrong_asset_kind() {
    expect(r#"main: [{id: "c1", clip: "music"}]"#, "E_WRONG_ASSET_KIND", "/main/0/clip");
}
#[test]
fn e21_layer_time() {
    expect(
        &format!("{OK_MAIN}, layers: [{{type: \"image\", asset: \"logo\", start: 3, end: 2}}]"),
        "E_LAYER_TIME",
        "/layers/0",
    );
}
#[test]
fn e22_unknown_template() {
    expect(
        &format!("{OK_MAIN}, layers: [{{type: \"text\", template: \"fancy\", text: \"x\"}}]"),
        "E_UNKNOWN_TEMPLATE",
        "/layers/0/template",
    );
}
#[test]
fn e23_alpha_not_present() {
    let text = r#"{ version: 1, assets: {a: {path: "a.mp4", alpha: true}}, main: [{id: "c", clip: "a"}] }"#;
    let p = cutline_core::parse::parse_project(text).unwrap();
    let (_, d) = resolve(&p, &lookup(), &Context::default());
    assert!(d.iter().any(|x| x.code == "E_ALPHA_NOT_PRESENT" && x.path == "/assets/a/alpha"), "{d:?}");
}
#[test]
fn e24_custom_size() {
    expect(r#"target: "custom", main: [{id: "c1", clip: "a"}]"#, "E_CUSTOM_SIZE", "/target");
}
#[test]
fn e25_effect_out_of_range() {
    expect(
        r#"main: [{id: "c1", clip: "a", out: 2, effects: [{type: "punch_in", at: 5}]}]"#,
        "E_EFFECT_OUT_OF_RANGE",
        "/main/0/effects/0/at",
    );
}
#[test]
fn e26_unknown_caption_style() {
    expect(&format!("{OK_MAIN}, captions: {{style: \"wobbly\"}}"), "E_UNKNOWN_CAPTION_STYLE", "/captions/style");
}
#[test]
fn e27_unknown_voice_chain() {
    expect(&format!("{OK_MAIN}, audio: {{voice_chain: \"robot\"}}"), "E_UNKNOWN_VOICE_CHAIN", "/audio/voice_chain");
}
#[test]
fn e28_font_missing() {
    expect(&format!("{OK_MAIN}, captions: {{font: \"Comic Sans\"}}"), "E_FONT_MISSING", "/captions/font");
}
#[test]
fn e29_unknown_sfx() {
    expect(
        r#"main: [{id: "c1", clip: "a", out: 2}, {transition: {type: "fade", duration: 0.3, sfx: "boing"}}, {id: "c2", clip: "b"}]"#,
        "E_UNKNOWN_SFX",
        "/main/1/transition/sfx",
    );
}
#[test]
fn e30_bad_opacity() {
    expect(
        &format!("{OK_MAIN}, layers: [{{type: \"image\", asset: \"logo\", opacity: 1.5}}]"),
        "E_BAD_VALUE",
        "/layers/0/opacity",
    );
}
#[test]
fn e31_unknown_color_preset() {
    expect(
        r#"main: [{id: "c1", clip: "a", color: {preset: "sepia9000"}}]"#,
        "E_UNKNOWN_COLOR_PRESET",
        "/main/0/color/preset",
    );
}
#[test]
fn e32_reverse_too_long() {
    expect(r#"main: [{id: "c1", clip: "a", reverse: true}]"#, "E_REVERSE_TOO_LONG", "/main/0/reverse");
}
#[test]
fn w01_shorts_over_limit() {
    let mut m = lookup();
    m.0.insert("long".into(), vid(400.0));
    let text = r#"{ version: 1, assets: {long: {path: "l.mp4"}}, main: [{id: "c", clip: "long"}] }"#;
    let p = cutline_core::parse::parse_project(text).unwrap();
    let (tl, d) = resolve(&p, &m, &Context::default());
    assert!(tl.is_some(), "warnings don't block");
    assert!(d.iter().any(|x| x.code == "W_SHORTS_OVER_LIMIT" && x.path == "/main"), "{d:?}");
}

#[test]
fn timeline_math_with_transitions() {
    let text = format!(
        "{{ version: 1, {ASSETS}, main: [{{id: 'c1', clip: 'a', in: 0, out: 2}}, {{transition: {{type: 'fade', duration: 0.5}}}}, {{id: 'c2', clip: 'b', in: 0, out: 3}}, {{transition: {{type: 'wipeleft', duration: 0.25}}}}, {{id: 'c3', clip: 'a', in: 5, out: 6, speed: 2}}] }}"
    );
    let p = cutline_core::parse::parse_project(&text).unwrap();
    let (tl, d) = resolve(&p, &lookup(), &Context::default());
    assert!(d.is_empty(), "{d:?}");
    let tl = tl.unwrap();
    // 60 + 90 + 15 frames − 15 − 8 (0.25 s rounds to 7.5 → 8)
    assert_eq!(tl.clips[2].frames, 15);
    assert_eq!(tl.total_frames, 60 + 90 + 15 - 15 - 8);
    assert_eq!(tl.clips[1].start, 45);
    assert_eq!(tl.clips[2].start, 45 + 90 - 8);
}

#[test]
fn anchored_layer_follows_ripple() {
    let mk = |out1: f64| {
        let text = format!(
            "{{ version: 1, {ASSETS}, main: [{{id: 'c1', clip: 'a', out: {out1}}}, {{id: 'c2', clip: 'b'}}], layers: [{{type: 'image', asset: 'logo', anchor: 'c2', offset: 0.5, duration: 1}}] }}"
        );
        let p = cutline_core::parse::parse_project(&text).unwrap();
        let (tl, d) = resolve(&p, &lookup(), &Context::default());
        assert!(d.is_empty(), "{d:?}");
        tl.unwrap().layers[0].start
    };
    assert_eq!(mk(2.0), 60 + 15);
    assert_eq!(mk(3.0), 90 + 15);
}

/// The checked-in schema must match the Rust types (bless with CUTLINE_BLESS=1).
#[test]
fn schema_is_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/project.schema.json");
    let now = cutline_core::schema_json();
    if std::env::var("CUTLINE_BLESS").as_deref() == Ok("1") {
        std::fs::write(&path, &now).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path).expect("schema/project.schema.json exists");
    assert!(
        on_disk == now,
        "schema/project.schema.json is stale: run `CUTLINE_BLESS=1 cargo test -p cutline-core schema_is_current`"
    );
}
