//! Auto directives for demos: fast_forward_idle, zoom_on_clicks, cursor.

use cutline_core::model::{Effect, Layer, MainItem};
use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::Path;

fn expand(l: &pipeline::Loaded) -> (cutline_core::Project, Vec<cutline_core::Diagnostic>) {
    let brand = cutline_render::extras::load_brand(l, &l.project).unwrap();
    cutline_render::auto::expand(l, &l.project, &brand).unwrap()
}

#[test]
fn idle_stretches_play_fast_and_anchored_layers_stay_on_their_moment() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // 2 s of motion, a 5 s freeze, 2 more seconds of motion
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=4,loop=loop=150:size=1:start=60,setpts=N/30/TB"])
        .args(["-pix_fmt", "yuv420p"])
        .arg(d.path().join("rec.mp4"));
    gen::run(c).unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, name: 'f', target: 'custom', width: 320, height: 180, fps: 30, profile: 'screencast',
          assets: {rec: {path: 'rec.mp4'}}, audio: {voice_chain: 'raw'},
          main: [{id: 'a', clip: 'rec'}],
          layers: [{type: 'highlight', rect: {x: 0.1, y: 0.1, w: 0.2, h: 0.2}, anchor: 'a', offset: 8.0, duration: 0.5},
                   {type: 'highlight', rect: {x: 0.5, y: 0.5, w: 0.2, h: 0.2}, anchor: 'a', offset: 0.5, duration: 0.5}],
          auto: {fast_forward_idle: {min_idle: 2.5, speed: 4, keep: 0.4}}}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    assert!((l.timeline.as_ref().unwrap().duration() - 9.0).abs() < 0.05);
    let (q, notes) = expand(&l);
    let clips: Vec<_> = q.clips().collect();
    assert_eq!(clips.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["a", "a~2", "a~3"], "{notes:?}");
    assert_eq!(clips[1].speed, Some(4.0));
    assert_eq!(clips[1].audio.as_deref(), Some("mute"));
    assert!(clips[0].speed.is_none() && clips[2].speed.is_none());
    // the freeze is 2.0–7.0 s; 0.4 s stays at normal speed on each side
    let secs = |t: &Option<cutline_core::time::TimeValue>| t.unwrap().seconds(cutline_core::time::Fps::new(30, 1), 0.0);
    assert!((secs(&clips[1].in_) - 2.4).abs() < 0.11 && (secs(&clips[1].out) - 6.6).abs() < 0.11, "{:?}", clips[1]);
    assert_eq!(secs(&clips[0].out), secs(&clips[1].in_));
    assert_eq!(secs(&clips[1].out), secs(&clips[2].in_));
    // the layer at 8.0 s of footage now belongs to the third piece, at the same moment of it
    let Layer::Highlight { at, .. } = &q.layers[0] else { panic!() };
    assert_eq!(at.anchor.as_deref(), Some("a~3"));
    assert!((at.offset.unwrap() - (8.0 - secs(&clips[2].in_))).abs() < 1e-6);
    let Layer::Highlight { at, .. } = &q.layers[1] else { panic!() };
    assert_eq!((at.anchor.as_deref(), at.offset), (Some("a"), Some(0.5)));
    // a speed badge over the fast piece
    let badge = q.layers.iter().find_map(|x| match x {
        Layer::Badge { text, at, .. } => Some((text.clone(), at.anchor.clone())),
        _ => None,
    });
    assert_eq!(badge, Some(("4×".to_string(), Some("a~2".to_string()))));
    // the render is shorter by three quarters of the fast stretch
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let fast = secs(&clips[1].out) - secs(&clips[1].in_);
    assert!((pr.timeline.duration() - (9.0 - fast * 0.75)).abs() < 0.1, "{}", pr.timeline.duration());
    pipeline::render(&l, &pr, &RenderOptions { jobs: 3, ..Default::default() }).unwrap();
}

const APP: &str = r#"<!doctype html><html><head><meta charset="utf-8"><style>
html, body { margin: 0; background: #ffffff; }
button { position: absolute; width: 120px; height: 40px; border: 0; background: rgb(0, 0, 255); }
#a { left: 60px; top: 60px; } #b { left: 560px; top: 380px; } #c { left: 300px; top: 200px; }
</style></head><body><button id="a"></button><button id="b"></button><button id="c"></button><input id="i" style="position:absolute;left:60px;top:300px"></body></html>"#;

fn setup_demo(d: &Path) -> std::path::PathBuf {
    std::fs::write(d.join("index.html"), APP).unwrap();
    std::fs::write(
        d.join("app.demo.json5"),
        "{viewport: {w: 800, h: 500, scale: 1}, url: 'index.html', settle: 0.3,
          steps: [{wait: 0.6}, {click: '#a', name: 'a', after: 1.0}, {click: '#b', name: 'b', after: 1.0}, {press: 'Meta+K', after: 5.5},
                  {click: '#c', name: 'c', after: 2.5}]}",
    )
    .unwrap();
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        "{version: 1, name: 'z', target: 'custom', width: 800, height: 500, fps: 30, profile: 'screencast',
          assets: {app: {path: 'app.demo.json5'}}, audio: {voice_chain: 'raw'},
          main: [{id: 'm', clip: 'app'}], auto: {zoom_on_clicks: {}, cursor: {}}}",
    )
    .unwrap();
    p
}

#[test]
fn clicks_get_zooms_and_a_pointer_from_the_recording() {
    if cutline_html::chrome().is_none() || test_toolchain().is_none() {
        eprintln!("SKIP: no Chrome or ffmpeg");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let pp = setup_demo(d.path());
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let ev = &l.demos["app"];
    let (q, notes) = expand(&l);
    let MainItem::Clip(clip) = &q.main[0] else { panic!() };
    // a and b follow closely: one zoom in on a, a pan to b, zoom out; c comes after a pause: its own zoom
    let kinds: Vec<&str> = clip
        .effects
        .iter()
        .map(|e| match e {
            Effect::ZoomTo { .. } => "zoom_to",
            Effect::Zoom { .. } => "pan",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, vec!["zoom_to", "pan", "zoom_to"], "{:?} {notes:?}", clip.effects);
    let Effect::ZoomTo { rect, start, end, .. } = &clip.effects[0] else { panic!() };
    let sa = ev.step("a").unwrap();
    assert!((rect.x - 0.075).abs() < 0.004 && (rect.w - 0.15).abs() < 0.004, "{rect:?}");
    assert!((start - (sa.t0 - 0.1).max(0.0)).abs() < 1e-3);
    let sb = ev.step("b").unwrap();
    assert!((end.unwrap() - (sb.t1 + 1.2)).abs() < 1e-3, "zooms out 1.2 s after b: {end:?}");
    let sc = ev.step("c").unwrap();
    assert!(matches!(&clip.effects[2], Effect::ZoomTo { start, .. } if (start - (sc.t0 - 0.1)).abs() < 1e-3));

    // the pointer layer covers the clip and clicks where the recording clicked
    let (path, at) = q
        .layers
        .iter()
        .find_map(|x| match x {
            Layer::Cursor { path, at, .. } => Some((path.clone(), at.clone())),
            _ => None,
        })
        .expect("cursor layer");
    assert_eq!(at.anchor.as_deref(), Some("m"));
    let clicks: Vec<_> = path.iter().filter(|k| k.click).collect();
    assert_eq!(clicks.len(), 3);
    assert!(
        (clicks[0].t - sa.t).abs() < 1e-3 && (clicks[0].x - 0.15).abs() < 0.003 && (clicks[0].y - 0.16).abs() < 0.003
    );
    // the shortcut gets a keys badge
    let keys = q.layers.iter().find_map(|x| match x {
        Layer::Keys { keys, .. } => Some(keys.clone()),
        _ => None,
    });
    assert_eq!(keys, Some(vec!["cmd".to_string(), "K".to_string()]));

    // and the whole thing renders
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, true).unwrap();
    assert!(!pr.extras.annot.is_empty());
    pipeline::render(&l, &pr, &RenderOptions { jobs: 3, draft: true, skip_loudness: true, ..Default::default() })
        .unwrap();
}
