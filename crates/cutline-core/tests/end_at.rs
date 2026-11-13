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
    let mut m = StaticLookup::default();
    m.0.insert("a".into(), vid(20.0));
    m.0.insert("b".into(), vid(6.0));
    m.0.insert("pic".into(), MediaFacts { kind: AssetKind::Image, has_audio: false, ..vid(0.04) });
    let text = format!(
        "{{version: 1, target: 'long_1080p', fps: 30, assets: {{a: {{path: 'a.mp4'}}, b: {{path: 'b.mp4'}}, pic: {{path: 'p.png'}}}}, main: [{main}]}}"
    );
    let p = cutline_core::parse::parse_project(&text).unwrap();
    let (tl, d) = resolve(&p, &m, &Context::default());
    (tl, d.into_iter().filter(|x| x.is_error()).map(|x| (x.code, x.path)).collect())
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
fn computes_speed_when_in_and_out_are_fixed() {
    let (tl, e) = run("{id: 'x', clip: 'a', in: 0, out: 9, end_at: 6}, {id: 'y', clip: 'b'}");
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
        ("{id: 'x', clip: 'a', in: 0, out: 1, end_at: 5}", "E_FIT_SPEED", "/main/0/end_at"),
        // the trimmed out point runs past the 6 s asset
        ("{id: 'x', clip: 'b', in: 2, end_at: 5}", "E_TIME_OUT_OF_RANGE", "/main/0/end_at"),
        ("{id: 'x', clip: 'a', duration: 2, end_at: 5}", "E_FIELD_CONFLICT", "/main/0/end_at"),
        ("{id: 'x', clip: 'a', end_at: 'end'}", "E_BAD_TIME", "/main/0/end_at"),
    ];
    for (main, code, path) in cases {
        let (_, e) = run(main);
        assert!(e.iter().any(|(c, p)| c == code && p == path), "{main}: want {code} at {path}, got {e:?}");
    }
}
