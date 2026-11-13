//! `align: end` backtimes an audio layer so the asset's ending lands on the layer end.

use cutline_core::model::{AssetKind, Layer};
use cutline_core::time::Fps;
use cutline_core::timeline::{resolve, Context, MediaFacts, StaticLookup, Timeline};

fn facts(kind: AssetKind, d: f64) -> MediaFacts {
    MediaFacts {
        exists: true,
        kind,
        duration: d,
        has_video: kind != AssetKind::Audio,
        has_audio: true,
        alpha: false,
        width: 1920,
        height: 1080,
        fps: Some(Fps::new(30, 1)),
        rotation: 0,
    }
}

fn run(layers: &str) -> (Option<Timeline>, Vec<(String, String)>) {
    let mut m = StaticLookup::default();
    m.0.insert("v".into(), facts(AssetKind::Video, 20.0));
    m.0.insert("long".into(), facts(AssetKind::Audio, 95.0));
    m.0.insert("short".into(), facts(AssetKind::Audio, 4.0));
    let text = format!(
        "{{version: 1, target: 'long_1080p', fps: 30, assets: {{v: {{path: 'v.mp4'}}, long: {{path: 'l.mp3', role: 'music'}}, short: {{path: 's.mp3', role: 'music'}}}},
          main: [{{id: 'c', clip: 'v', in: 0, out: 10}}], layers: [{layers}]}}"
    );
    let p = cutline_core::parse::parse_project(&text).unwrap();
    let (tl, d) = resolve(&p, &m, &Context::default());
    (tl, d.into_iter().filter(|x| x.is_error()).map(|x| (x.code, x.path)).collect())
}

fn src_in(tl: &Timeline) -> f64 {
    match &tl.layers[0].layer {
        Layer::Audio { src_in, .. } => src_in.map(|t| t.seconds(tl.fps, 0.0)).unwrap_or(0.0),
        _ => panic!("not audio"),
    }
}

#[test]
fn long_music_is_trimmed_at_the_head() {
    let (tl, e) = run("{type: 'audio', asset: 'long', align: 'end'}");
    assert!(e.is_empty(), "{e:?}");
    let tl = tl.unwrap();
    // runs to the timeline end (10 s) and starts 85 s into the 95 s track
    assert_eq!((tl.layers[0].start, tl.layers[0].end), (0, 300));
    assert!((src_in(&tl) - 85.0).abs() < 1e-6);
}

#[test]
fn explicit_end_and_short_music() {
    let (tl, _) = run("{type: 'audio', asset: 'long', align: 'end', start: 2, end: 8}");
    let tl = tl.unwrap();
    assert!((src_in(&tl) - 89.0).abs() < 1e-6, "6 s span from a 95 s track");
    // a 4 s sting backtimed to the end starts at 6 s instead of being stretched
    let (tl, _) = run("{type: 'audio', asset: 'short', align: 'end'}");
    let tl = tl.unwrap();
    assert_eq!((tl.layers[0].start, tl.layers[0].end), (180, 300));
    assert_eq!(src_in(&tl), 0.0);
}

#[test]
fn align_conflicts_with_src_in_and_loop() {
    let (_, e) = run("{type: 'audio', asset: 'long', align: 'end', src_in: 3}");
    assert!(e.contains(&("E_FIELD_CONFLICT".into(), "/layers/0/align".into())), "{e:?}");
    let (_, e) = run("{type: 'audio', asset: 'long', align: 'end', loop: true}");
    assert!(e.contains(&("E_FIELD_CONFLICT".into(), "/layers/0/align".into())), "{e:?}");
}
