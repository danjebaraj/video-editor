use cutline_core::model::AssetKind;
use cutline_core::time::Fps;
use cutline_core::timeline::{resolve, Context, MediaFacts, StaticLookup};
use cutline_transcribe::mapping::{map_clip, map_layer, speech_spans};
use cutline_transcribe::Word;

fn tl(main: &str) -> cutline_core::Timeline {
    let mut m = StaticLookup::default();
    m.0.insert(
        "a".into(),
        MediaFacts {
            exists: true,
            kind: AssetKind::Video,
            duration: 20.0,
            has_video: true,
            has_audio: true,
            alpha: false,
            width: 1920,
            height: 1080,
            fps: Some(Fps::new(30, 1)),
            rotation: 0,
        },
    );
    let p = cutline_core::parse::parse_project(&format!(
        "{{version: 1, target: 'long_1080p', fps: 30, assets: {{a: {{path: 'a.mp4'}}}}, main: [{main}]}}"
    ))
    .unwrap();
    let (t, d) = resolve(&p, &m, &Context::default());
    assert!(d.is_empty(), "{d:?}");
    t.unwrap()
}

fn words() -> Vec<Word> {
    // one word per second: w0 [0.1, 0.6], w1 [1.1, 1.6], ...
    (0..20)
        .map(|i| Word { w: format!("w{i}"), start: i as f64 + 0.1, end: i as f64 + 0.6, conf: 1.0, speaker: None })
        .collect()
}

fn all(t: &cutline_core::Timeline) -> Vec<Word> {
    t.clips.iter().flat_map(|c| map_clip(&words(), c, t)).collect()
}

#[test]
fn trims_drop_words_outside() {
    let t = tl("{id: 'c', clip: 'a', in: 2, out: 5}");
    let m = all(&t);
    assert_eq!(m.iter().map(|w| w.w.as_str()).collect::<Vec<_>>(), vec!["w2", "w3", "w4"]);
    assert!((m[0].start - 0.1).abs() < 1e-9 && (m[2].end - 2.6).abs() < 1e-9);
}

#[test]
fn speed_change_compresses_time() {
    let t = tl("{id: 'c', clip: 'a', in: 4, out: 8, speed: 2}");
    let m = all(&t);
    assert_eq!(m.len(), 4);
    assert!((m[1].start - (5.1 - 4.0) / 2.0).abs() < 1e-9, "{:?}", m[1]);
    assert!((m[3].end - (7.6 - 4.0) / 2.0).abs() < 1e-9);
}

#[test]
fn jump_cuts_remove_words_and_close_gaps() {
    // jump-cut style: same asset, source gap 3..6 removed
    let t = tl("{id: 'a1', clip: 'a', in: 0, out: 3}, {id: 'a2', clip: 'a', in: 6, out: 9}");
    let m = all(&t);
    let names: Vec<_> = m.iter().map(|w| w.w.as_str()).collect();
    assert_eq!(names, vec!["w0", "w1", "w2", "w6", "w7", "w8"]);
    // w6 at source 6.1 → timeline 3.1
    assert!((m[3].start - 3.1).abs() < 1e-9);
    assert!(m.windows(2).all(|p| p[0].start <= p[1].start));
}

#[test]
fn transitions_shift_following_clip() {
    let t = tl("{id: 'a1', clip: 'a', in: 0, out: 3}, {transition: {type: 'fade', duration: 0.5}}, {id: 'a2', clip: 'a', in: 10, out: 12}");
    let m = all(&t);
    let w10 = m.iter().find(|w| w.w == "w10").unwrap();
    assert!((w10.start - (2.5 + 0.1)).abs() < 1e-9, "{w10:?}");
}

#[test]
fn freeze_and_reverse() {
    let t = tl("{id: 'c', clip: 'a', in: 0, out: 4, effects: [{type: 'freeze', at: 1.0, duration: 2.0}]}");
    let m = all(&t);
    let w2 = m.iter().find(|w| w.w == "w2").unwrap();
    assert!((w2.start - 4.1).abs() < 1e-9, "{w2:?}");
    let t = tl("{id: 'c', clip: 'a', in: 0, out: 4, reverse: true}");
    assert!(all(&t).is_empty());
}

#[test]
fn layer_mapping_and_spans() {
    let m = map_layer(&words()[..5], 10.0, 12.5, 1.0);
    assert_eq!(m.iter().map(|w| w.w.as_str()).collect::<Vec<_>>(), vec!["w1", "w2", "w3"]);
    assert!((m[2].end - 12.5).abs() < 1e-9, "clamped to layer end");
    assert!((m[0].start - 10.1).abs() < 1e-9);
    let s = speech_spans(&m, 0.6);
    assert_eq!(s.len(), 1);
}
