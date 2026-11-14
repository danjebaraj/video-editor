//! Effects on retimed clips: zooms the clip's end cuts off (`W_EFFECT_UNSEEN`), source-relative
//! effect times (`src_start`, `src_at`), `{speed}` in badge text, and annotations that start
//! where their anchor clip ends (`W_LAYER_PAST_CLIP`).

use cutline_core::diag::Diagnostic;
use cutline_core::model::{speed_text, AssetKind, Effect, Layer};
use cutline_core::time::Fps;
use cutline_core::timeline::{resolve, Context, MediaFacts, StaticLookup, Timeline};

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

fn parse(body: &str) -> Result<cutline_core::model::Project, Vec<Diagnostic>> {
    let text = format!(
        "{{version: 1, target: 'long_1080p', fps: 30, assets: {{a: {{path: 'a.mp4'}}, b: {{path: 'b.mp4'}}, pic: {{path: 'p.png'}}}}, {body}}}"
    );
    cutline_core::parse::parse_project(&text)
}

fn run(body: &str) -> (Option<Timeline>, Vec<Diagnostic>) {
    let mut m = StaticLookup::default();
    m.0.insert("a".into(), vid(60.0));
    m.0.insert("b".into(), vid(6.0));
    m.0.insert("pic".into(), MediaFacts { kind: AssetKind::Image, has_audio: false, ..vid(0.04) });
    match parse(body) {
        Ok(p) => resolve(&p, &m, &Context::default()),
        Err(d) => (None, d),
    }
}

fn codes(d: &[Diagnostic]) -> Vec<(&str, &str)> {
    d.iter().map(|x| (x.code.as_str(), x.path.as_str())).collect()
}

const RECT: &str = "rect: {x: 0.5, y: 0.1, w: 0.4, h: 0.4}";

#[test]
fn zoom_cut_off_by_the_clip_end_warns() {
    // the real case: a 3.2 s clip with a zoom_to that starts at 3.15 and takes 0.25 s
    let (tl, d) = run(&format!(
        "main: [{{id: 'ask', clip: 'a', in: 2.3, out: 5.5, effects: [{{type: 'zoom_to', {RECT}, start: 3.15, duration: 0.25}}]}}]"
    ));
    assert!(tl.is_some());
    assert_eq!(codes(&d), [("W_EFFECT_UNSEEN", "/main/0/effects/0/start")], "{d:?}");
    assert!(d[0].message.contains("never seen complete"), "{}", d[0].message);
    assert!(d[0].hint.as_deref().unwrap().contains("src_start"), "{:?}", d[0].hint);
    // arrives, but is held for less than 0.3 s
    let (_, d) = run(&format!(
        "main: [{{id: 'ask', clip: 'a', out: 3.2, effects: [{{type: 'zoom_to', {RECT}, start: 2.5, duration: 0.5}}]}}]"
    ));
    assert_eq!(codes(&d), [("W_EFFECT_UNSEEN", "/main/0/effects/0/start")], "{d:?}");
    assert!(d[0].message.contains("held for only 0.20s"), "{}", d[0].message);
    // held for 0.3 s or more: fine
    let (_, d) = run(&format!(
        "main: [{{id: 'ask', clip: 'a', out: 3.2, effects: [{{type: 'zoom_to', {RECT}, start: 2.4, duration: 0.5}}]}}]"
    ));
    assert!(d.is_empty(), "{d:?}");
    // an instant zoom at the very start of a short clip is seen
    let (_, d) = run(&format!(
        "main: [{{id: 'ask', clip: 'a', out: 0.5, effects: [{{type: 'zoom_to', {RECT}, start: 0, duration: 0}}]}}]"
    ));
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn outgoing_transition_and_other_zooms_count() {
    // 1 s dissolve after the clip: the cut is 0.5 s before the clip's end
    let fx = format!("effects: [{{type: 'zoom_to', {RECT}, start: 2.0, duration: 0.5}}]");
    let (_, d) = run(&format!("main: [{{id: 'x', clip: 'a', out: 3.2, {fx}}}, {{id: 'y', clip: 'b'}}]"));
    assert!(d.is_empty(), "{d:?}");
    let (_, d) = run(&format!(
        "main: [{{id: 'x', clip: 'a', out: 3.2, {fx}}}, {{transition: {{type: 'fade', duration: 1}}}}, {{id: 'y', clip: 'b'}}]"
    ));
    assert_eq!(codes(&d), [("W_EFFECT_UNSEEN", "/main/0/effects/0/start")], "{d:?}");
    // punch_in and a continuous zoom
    let (_, d) = run("main: [{id: 'x', clip: 'a', out: 3, effects: [{type: 'punch_in', at: 2.8}]}]");
    assert_eq!(codes(&d), [("W_EFFECT_UNSEEN", "/main/0/effects/0/at")], "{d:?}");
    let (_, d) = run("main: [{id: 'x', clip: 'a', out: 3, effects: [{type: 'zoom', to: 1.3, start: 2.9, end: 6}]}]");
    assert_eq!(codes(&d), [("W_EFFECT_UNSEEN", "/main/0/effects/0/start")], "{d:?}");
    let (_, d) =
        run("main: [{id: 'x', clip: 'a', out: 3, effects: [{type: 'zoom', to: 1.3}, {type: 'punch_in', at: 1}]}]");
    assert!(d.is_empty(), "{d:?}");
}

fn zoom_start(tl: &Timeline, clip: usize) -> (f64, Option<f64>) {
    match &tl.clips[clip].clip.effects[0] {
        Effect::ZoomTo { start, end, src_start, src_end, .. } => {
            assert!(src_start.is_none() && src_end.is_none(), "resolved effects carry clip times only");
            (*start, *end)
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn src_start_follows_the_footage_through_retiming() {
    let fx = format!("effects: [{{type: 'zoom_to', {RECT}, src_start: 8.0, src_end: 11.0, duration: 0.4}}]");
    // normal speed: 8 s in the asset is 6 s into a clip that starts at 2
    let (tl, d) = run(&format!("main: [{{id: 'x', clip: 'a', in: 2, out: 14, {fx}}}]"));
    assert!(d.is_empty(), "{d:?}");
    assert_eq!(zoom_start(&tl.unwrap(), 0), (6.0, Some(9.0)));
    // speed 2
    let (tl, d) = run(&format!("main: [{{id: 'x', clip: 'a', in: 2, out: 14, speed: 2, {fx}}}]"));
    assert!(d.is_empty(), "{d:?}");
    assert_eq!(zoom_start(&tl.unwrap(), 0), (3.0, Some(4.5)));
    // fit: "speed": 12 s of footage in a 4 s slot plays at 3×
    let (tl, d) = run(&format!(
        "main: [{{id: 'x', clip: 'a', in: 2, out: 14, fit: 'speed', end_at: 4, {fx}}}, {{id: 'y', clip: 'b'}}]"
    ));
    assert!(d.is_empty(), "{d:?}");
    let tl = tl.unwrap();
    assert!((tl.clips[0].speed - 3.0).abs() < 1e-9);
    assert_eq!(zoom_start(&tl, 0), (2.0, Some(3.0)));
    // the same edit with a later cut: the zoom is still on second 8 of the footage
    let (tl, _) = run(&format!(
        "main: [{{id: 'x', clip: 'a', in: 2, out: 14, fit: 'speed', end_at: 6, {fx}}}, {{id: 'y', clip: 'b'}}]"
    ));
    assert_eq!(zoom_start(&tl.unwrap(), 0), (3.0, Some(4.5)));
    // moving `in` moves the clip-relative time, not the moment
    let (tl, _) = run(&format!("main: [{{id: 'x', clip: 'a', in: 5, out: 14, {fx}}}]"));
    assert_eq!(zoom_start(&tl.unwrap(), 0), (3.0, Some(6.0)));
}

#[test]
fn src_at_on_punch_in_and_reverse() {
    let (tl, d) =
        run("main: [{id: 'x', clip: 'a', in: 4, out: 10, speed: 1.5, effects: [{type: 'punch_in', src_at: 7}]}]");
    assert!(d.is_empty(), "{d:?}");
    assert!(
        matches!(&tl.unwrap().clips[0].clip.effects[0], Effect::PunchIn { at, src_at: None, .. } if (at - 2.0).abs() < 1e-9)
    );
    // reversed: source second 9 comes 1 s after the start (the clip plays 10 → 4)
    let (tl, _) =
        run("main: [{id: 'x', clip: 'a', in: 4, out: 10, reverse: true, effects: [{type: 'flash', src_at: 9}]}]");
    assert!(matches!(&tl.unwrap().clips[0].clip.effects[0], Effect::Flash { at, .. } if (at - 1.0).abs() < 1e-9));
}

#[test]
fn src_times_are_checked() {
    // both forms of the same time
    let (_, d) =
        run(&format!("main: [{{id: 'x', clip: 'a', effects: [{{type: 'zoom_to', {RECT}, start: 1, src_start: 8}}]}}]"));
    assert_eq!(codes(&d), [("E_FIELD_CONFLICT", "/main/0/effects/0/src_start")], "{d:?}");
    let (_, d) = run("main: [{id: 'x', clip: 'a', effects: [{type: 'punch_in', at: 1, src_at: 8}]}]");
    assert_eq!(codes(&d), [("E_FIELD_CONFLICT", "/main/0/effects/0/src_at")], "{d:?}");
    let (_, d) = run(&format!(
        "main: [{{id: 'x', clip: 'a', effects: [{{type: 'zoom_to', {RECT}, src_start: 8, end: 3, src_end: 12}}]}}]"
    ));
    assert_eq!(codes(&d), [("E_FIELD_CONFLICT", "/main/0/effects/0/src_end")], "{d:?}");
    // a serialized project writes the default beside the source time: that is not a conflict,
    // and `expand` drops it
    let p = parse(&format!(
        "main: [{{id: 'x', clip: 'a', in: 2, effects: [{{type: 'zoom_to', {RECT}, src_start: 8}}, {{type: 'punch_in', src_at: 9}}]}}]"
    ))
    .unwrap();
    let mut v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["main"][0]["effects"][0]["start"], 0.0);
    assert_eq!(cutline_core::parse::parse_value(&v).unwrap(), p);
    cutline_core::parse::strip_shadowed_effect_times(&mut v);
    assert!(v["main"][0]["effects"][0].get("start").is_none() && v["main"][0]["effects"][1].get("at").is_none());
    assert_eq!(v["main"][0]["effects"][0]["src_start"], 8.0);
    assert_eq!(cutline_core::parse::parse_value(&v).unwrap(), p);
    // neither
    let (_, d) = run("main: [{id: 'x', clip: 'a', effects: [{type: 'punch_in'}]}]");
    assert_eq!(codes(&d), [("E_MISSING_FIELD", "/main/0/effects/0/at")], "{d:?}");
    // a source time outside the part of the asset the clip plays
    let (_, d) = run(&format!(
        "main: [{{id: 'x', clip: 'a', in: 10, out: 14, effects: [{{type: 'zoom_to', {RECT}, src_start: 8}}]}}]"
    ));
    assert_eq!(codes(&d), [("E_EFFECT_OUT_OF_RANGE", "/main/0/effects/0/src_start")], "{d:?}");
    assert!(d[0].message.contains("source time 8.000s"), "{}", d[0].message);
    // a source-timed zoom too close to the cut is warned about on its own field
    let (_, d) = run(&format!(
        "main: [{{id: 'x', clip: 'a', in: 10, out: 14, effects: [{{type: 'zoom_to', {RECT}, src_start: 13.5}}]}}]"
    ));
    assert_eq!(codes(&d), [("W_EFFECT_UNSEEN", "/main/0/effects/0/src_start")], "{d:?}");
}

#[test]
fn speed_placeholder_in_badge_and_text() {
    assert_eq!(speed_text(3.11), "3.1");
    assert_eq!(speed_text(3.0), "3");
    assert_eq!(speed_text(2.72), "2.7");
    assert_eq!(speed_text(2.96), "3");
    assert_eq!(speed_text(10.0), "10");
    assert_eq!(speed_text(0.5), "0.5");
    assert_eq!(speed_text(0.25), "0.25");
    let layers = "layers: [{type: 'badge', text: '{speed}×', anchor: 'x', duration: 2}, \
                  {type: 'text', template: 'callout', text: 'Sped up {speed}×', anchor: 'x', duration: 2}]";
    let (tl, d) = run(&format!(
        "main: [{{id: 'x', clip: 'a', in: 2.3, out: 13.6, fit: 'speed', end_at: 3.633333}}, {{id: 'y', clip: 'b'}}], {layers}"
    ));
    assert!(d.is_empty(), "{d:?}");
    let tl = tl.unwrap();
    assert!((tl.clips[0].speed - 3.11).abs() < 0.01, "{}", tl.clips[0].speed);
    let texts: Vec<String> = tl
        .layers
        .iter()
        .map(|l| match &l.layer {
            Layer::Badge { text, .. } => text.clone(),
            Layer::Text { text, .. } => text.clone().unwrap(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(texts, ["3.1×", "Sped up 3.1×"]);
    // a whole speed has no decimals
    let (tl, _) = run(
        "main: [{id: 'x', clip: 'a', out: 12, speed: 3}], layers: [{type: 'badge', text: '{speed}×', anchor: 'x'}]",
    );
    assert!(matches!(&tl.unwrap().layers[0].layer, Layer::Badge { text, .. } if text == "3×"));
    // without an anchor there is no clip to take the speed from
    let (_, d) = run("main: [{id: 'x', clip: 'a', out: 12}], layers: [{type: 'badge', text: '{speed}×', start: 1}]");
    assert_eq!(codes(&d), [("E_SPEED_NO_ANCHOR", "/layers/0/text")], "{d:?}");
}

#[test]
fn annotation_starting_at_its_anchor_clips_end_warns() {
    let hl = |offset: f64| {
        format!(
            "main: [{{id: 'x', clip: 'a', out: 3.2}}, {{id: 'y', clip: 'b'}}], \
             layers: [{{type: 'highlight', rect: {{x: 0.2, y: 0.2, w: 0.2, h: 0.1}}, anchor: 'x', offset: {offset}, duration: 2}}]"
        )
    };
    let (_, d) = run(&hl(3.0));
    assert_eq!(codes(&d), [("W_LAYER_PAST_CLIP", "/layers/0/offset")], "{d:?}");
    let (_, d) = run(&hl(4.0));
    assert_eq!(codes(&d), [("W_LAYER_PAST_CLIP", "/layers/0/offset")], "{d:?}");
    assert!(d[0].message.contains("after its anchor clip"), "{}", d[0].message);
    // mostly on its clip, or ending with it: fine
    let (_, d) = run(&hl(2.0));
    assert!(d.is_empty(), "{d:?}");
    let (_, d) = run("main: [{id: 'x', clip: 'a', out: 3.2}, {id: 'y', clip: 'b'}], \
         layers: [{type: 'badge', text: '2×', anchor: 'x', offset: 3.0, duration: 0.2}]");
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn gap_and_script_check_settings_are_validated() {
    let p = |gaps: &str, audio: &str| {
        let text = format!(
            "{{version: 1, target: 'long_1080p', fps: 30, assets: {{a: {{path: 'a.mp4', gaps: {gaps}}}}}, main: [{{id: 'x', clip: 'a'}}], audio: {audio}}}"
        );
        let mut m = StaticLookup::default();
        m.0.insert("a".into(), vid(60.0));
        match cutline_core::parse::parse_project(&text) {
            Ok(p) => resolve(&p, &m, &Context::default()).1,
            Err(d) => d,
        }
    };
    assert!(p("[{before: 'Need more room', add: 0.7}, {after: 'cue:room', add: 0.3, occurrence: 2}]", "{}").is_empty());
    assert_eq!(codes(&p("[{before: 'a', after: 'b', add: 0.7}]", "{}")), [("E_FIELD_CONFLICT", "/assets/a/gaps/0")]);
    assert_eq!(codes(&p("[{add: 0.7}]", "{}")), [("E_FIELD_CONFLICT", "/assets/a/gaps/0")]);
    assert_eq!(codes(&p("[{before: 'a', add: 0}]", "{}")), [("E_BAD_VALUE", "/assets/a/gaps/0/add")]);
    assert_eq!(codes(&p("[{before: 'a', add: 11}]", "{}")), [("E_BAD_VALUE", "/assets/a/gaps/0/add")]);
    assert_eq!(codes(&p("[{before: 'a', add: 1, pause: 2}]", "{}"))[0].0, "E_UNKNOWN_FIELD");
    assert!(
        p("[]", "{script_check: {enabled: false, similarity: 0.9, min_span_words: 2, ignore: ['Glance']}}").is_empty()
    );
    assert_eq!(
        codes(&p("[]", "{script_check: {similarity: 1.5}}")),
        [("E_BAD_VALUE", "/audio/script_check/similarity")]
    );
    assert_eq!(
        codes(&p("[]", "{script_check: {min_span_words: 0}}")),
        [("E_BAD_VALUE", "/audio/script_check/min_span_words")]
    );
}
