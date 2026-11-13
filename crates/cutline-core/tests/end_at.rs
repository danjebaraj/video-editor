//! `end_at`: clips fitted so the cut to the next clip lands on a given timeline time.

use cutline_core::model::AssetKind;
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

fn run(main: &str) -> (Option<Timeline>, Vec<(String, String)>) {
    let (tl, d) = run_all(main);
    (tl, d.into_iter().filter(|x| x.is_error()).map(|x| (x.code, x.path)).collect())
}

fn run_all(main: &str) -> (Option<Timeline>, Vec<cutline_core::diag::Diagnostic>) {
    let mut m = StaticLookup::default();
    m.0.insert("a".into(), vid(20.0));
    m.0.insert("b".into(), vid(6.0));
    m.0.insert("pic".into(), MediaFacts { kind: AssetKind::Image, has_audio: false, ..vid(0.04) });
    let text = format!(
        "{{version: 1, target: 'long_1080p', fps: 30, assets: {{a: {{path: 'a.mp4'}}, b: {{path: 'b.mp4'}}, pic: {{path: 'p.png'}}}}, main: [{main}]}}"
    );
    let p = cutline_core::parse::parse_project(&text).unwrap();
    resolve(&p, &m, &Context::default())
}

/// Timeline seconds of the cut between clip `i` and `i + 1` (transition midpoint or boundary).
fn cut(tl: &Timeline, i: usize) -> f64 {
    match tl.transitions.iter().find(|t| t.after == i) {
        Some(t) => tl.secs(t.start) + tl.secs(t.frames) / 2.0,
        None => tl.secs(tl.clips[i].end()),
    }
}

#[test]
fn trims_out_point_to_land_the_cut() {
    // no `out`: the out point is computed at speed 1.5
    let (tl, e) = run("{id: 'x', clip: 'a', in: 2, speed: 1.5, end_at: 4}, {id: 'y', clip: 'b'}");
    assert!(e.is_empty(), "{e:?}");
    let tl = tl.unwrap();
    assert!((cut(&tl, 0) - 4.0).abs() < 1e-9);
    let x = &tl.clips[0];
    assert!((x.src_out - (2.0 + 4.0 * 1.5)).abs() < 1e-6, "src_out {}", x.src_out);
    assert_eq!(x.clip.out.map(|o| o.seconds(tl.fps, 0.0)), Some(8.0));
}

#[test]
fn in_and_out_trim_at_normal_speed_by_default() {
    // 9 s of footage, 6 s slot: plays at 1× and the tail is trimmed, never sped up
    let (tl, d) = run_all("{id: 'x', clip: 'a', in: 0, out: 9, end_at: 6}, {id: 'y', clip: 'b'}");
    assert!(d.is_empty(), "{d:?}");
    let tl = tl.unwrap();
    assert_eq!(tl.clips[0].speed, 1.0);
    assert_eq!(tl.clips[0].frames, 180);
    assert!((tl.clips[0].src_out - 6.0).abs() < 1e-6, "src_out {}", tl.clips[0].src_out);
}

#[test]
fn short_footage_holds_its_last_frame_with_a_warning() {
    // 1 s of footage in a 5 s slot (explicit out), and a 6 s asset asked for 7 s (no out)
    for (main, out) in [
        ("{id: 'x', clip: 'a', in: 0, out: 1, end_at: 5}", 1.0),
        ("{id: 'x', clip: 'b', in: 2, end_at: 7}", 6.0),
    ] {
        let (tl, d) = run_all(main);
        assert!(d.iter().all(|x| !x.is_error()), "{d:?}");
        assert!(d.iter().any(|x| x.code == "W_FIT_HOLD" && x.path == "/main/0/end_at"), "{main}: {d:?}");
        let tl = tl.unwrap();
        assert_eq!(tl.clips[0].speed, 1.0);
        assert!((tl.clips[0].src_out - out).abs() < 1e-6);
        assert!((tl.duration() - if out == 1.0 { 5.0 } else { 7.0 }).abs() < 1e-9, "the slot keeps its length");
    }
}

#[test]
fn fit_speed_computes_the_speed_when_in_and_out_are_fixed() {
    let (tl, e) = run("{id: 'x', clip: 'a', in: 0, out: 9, end_at: 6, fit: 'speed'}, {id: 'y', clip: 'b'}");
    assert!(e.is_empty(), "{e:?}");
    let tl = tl.unwrap();
    assert!((tl.clips[0].speed - 1.5).abs() < 1e-9, "speed {}", tl.clips[0].speed);
    assert_eq!(tl.clips[0].frames, 180);
}

#[test]
fn transitions_on_both_sides_center_the_cut() {
    // fade 0.4 s (12 frames) into x, fade 0.6 s (18 frames) out of x
    let main = "{id: 'w', clip: 'b', in: 0, out: 3}, {transition: {type: 'fade', duration: 0.4}},
                {id: 'x', clip: 'a', in: 1, end_at: 7.5}, {transition: {type: 'fade', duration: 0.6}},
                {id: 'y', clip: 'pic', end_at: 10}";
    let (tl, e) = run(main);
    assert!(e.is_empty(), "{e:?}");
    let tl = tl.unwrap();
    assert!((cut(&tl, 1) - 7.5).abs() < 1e-9, "cut {}", cut(&tl, 1));
    // the last clip (a still) ends at 10 exactly
    assert!((tl.secs(tl.clips[2].end()) - 10.0).abs() < 1e-9);
    assert!((tl.duration() - 10.0).abs() < 1e-9);
}

#[test]
fn freezes_count_toward_the_fit() {
    let (tl, e) =
        run("{id: 'x', clip: 'a', end_at: 5, effects: [{type: 'freeze', at: 1, duration: 1}]}, {id: 'y', clip: 'b'}");
    assert!(e.is_empty(), "{e:?}");
    let tl = tl.unwrap();
    assert!((cut(&tl, 0) - 5.0).abs() < 1e-9);
    assert!((tl.clips[0].src_out - 4.0).abs() < 1e-6, "4 s of footage + 1 s freeze");
}

#[test]
fn errors_point_at_end_at() {
    let cases = [
        // cut before the clip can start
        (
            "{id: 'w', clip: 'b', in: 0, out: 3}, {id: 'x', clip: 'a', end_at: 2}",
            "E_END_AT_BEFORE_START",
            "/main/1/end_at",
        ),
        // 1 s of footage cannot fill 5 s
        ("{id: 'x', clip: 'a', in: 0, out: 1, end_at: 5, fit: 'speed'}", "E_FIT_SPEED", "/main/0/end_at"),
        ("{id: 'x', clip: 'a', duration: 2, end_at: 5}", "E_FIELD_CONFLICT", "/main/0/end_at"),
        ("{id: 'x', clip: 'a', end_at: 'end'}", "E_BAD_TIME", "/main/0/end_at"),
    ];
    for (main, code, path) in cases {
        let (_, e) = run(main);
        assert!(e.iter().any(|(c, p)| c == code && p == path), "{main}: want {code} at {path}, got {e:?}");
    }
}

#[test]
fn screencast_dissolves_between_recordings_warn() {
    let mut m = StaticLookup::default();
    m.0.insert("a".into(), vid(20.0));
    let fade = "{transition: {type: 'fade', duration: 0.3}}";
    let text = format!(
        "{{version: 1, target: 'long_1080p', fps: 30, profile: 'screencast', assets: {{a: {{path: 'a.mp4'}}}},
          main: [{{id: 'x', clip: 'a', in: 0, out: 3}}, {fade}, {{id: 'y', clip: 'a', in: 4, out: 7}}, {fade},
                 {{id: 'z', clip: 'a', in: 8, out: 11}}]}}"
    );
    let p = cutline_core::parse::parse_project(&text).unwrap();
    let (_, d) = resolve(&p, &m, &Context::default());
    assert!(d.iter().any(|x| x.code == "W_SCREEN_DISSOLVES"), "{d:?}");
    // back/spring cannot drive a transition
    let (_, e) = run("{id: 'x', clip: 'a', in: 0, out: 3}, {transition: {type: 'fade', duration: 0.3, ease: 'spring'}}, {id: 'y', clip: 'b'}");
    assert!(e.iter().any(|(c, p)| c == "E_BAD_VALUE" && p == "/main/1/transition/ease"), "{e:?}");
}
