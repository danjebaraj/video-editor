//! Browser recordings as assets, and editing by named step.

use cutline_core::model::{Effect, Layer};
use cutline_render::pipeline::{self, LoadError};
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{mean_rgb, read_frames};
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

fn ready() -> bool {
    if cutline_html::chrome().is_none() || test_toolchain().is_none() {
        eprintln!("SKIP: no Chrome or ffmpeg");
        return false;
    }
    true
}

/// A page with a blue button at (100, 80) 200×60 that turns the panel at (400, 80) green.
const APP: &str = r#"<!doctype html><html><head><meta charset="utf-8"><style>
html, body { margin: 0; background: #ffffff; }
#go { position: absolute; left: 100px; top: 80px; width: 200px; height: 60px; border: 0; background: rgb(0, 0, 255); }
#panel { position: absolute; left: 400px; top: 80px; width: 300px; height: 200px; background: rgb(255, 0, 0); }
#panel.on { background: rgb(0, 200, 0); }
</style></head><body><button id="go"></button><div id="panel"></div>
<script>document.getElementById('go').addEventListener('click', () => document.getElementById('panel').classList.add('on'));</script>
</body></html>"#;

fn setup(d: &Path, body: &str) -> PathBuf {
    std::fs::write(d.join("index.html"), APP).unwrap();
    std::fs::write(
        d.join("app.demo.json5"),
        "{viewport: {w: 800, h: 500, scale: 1}, url: 'index.html', settle: 0.5,
          steps: [{wait: 1.0}, {click: '#go', name: 'export', after: 1.5}]}",
    )
    .unwrap();
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'b', target: 'custom', width: 800, height: 500, fps: 30, profile: 'screencast',
              assets: {{app: {{path: 'app.demo.json5'}}}}, audio: {{voice_chain: 'raw'}}, {body}}}"
        ),
    )
    .unwrap();
    p
}

#[test]
fn a_demo_asset_is_recorded_and_edited_by_step() {
    if !ready() {
        return;
    }
    let tc = test_toolchain().unwrap();
    let d = tempfile::tempdir().unwrap();
    let pp = setup(
        d.path(),
        "main: [{id: 'a', clip: 'app', in: 'step:export-1.0', effects: [{type: 'zoom_to', step: 'export', duration: 0.3, padding: 0.1}]}],
         layers: [{type: 'highlight', step: 'export', color: '#FF00FF', enter: 'none', width: 6}]",
    );
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let ev = &l.demos["app"];
    let st = ev.step("export").unwrap();
    let tl = l.timeline.as_ref().unwrap();
    let c = &tl.clips[0];
    // `in: step:export-1.0` → one second before the click
    assert!((c.src_in - (st.t - 1.0)).abs() < 1e-4, "{} vs {}", c.src_in, st.t);
    // the zoom got the button's rect and starts as the pointer sets off
    let Effect::ZoomTo { rect, start, .. } = &c.clip.effects[0] else { panic!("{:?}", c.clip.effects) };
    assert!(
        (rect.x - 0.125).abs() < 0.004 && (rect.y - 0.16).abs() < 0.004 && (rect.w - 0.25).abs() < 0.004,
        "{rect:?}"
    );
    assert!((start - (st.t0 - c.src_in)).abs() < 1e-3);
    // the highlight is anchored to the clip for the length of the step plus a second
    let Layer::Highlight { rect: hr, at, .. } = &tl.layers[0].layer else { panic!() };
    assert_eq!(hr, rect);
    assert_eq!(at.anchor.as_deref(), Some("a"));

    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let out = pipeline::render(&l, &pr, &RenderOptions { jobs: 3, ..Default::default() }).unwrap().0.output;
    // 0.6 s after the click: zoomed on the button (blue in the middle), outlined in magenta
    let click = st.t - c.src_in;
    let frames = read_frames(tc, &out, &[((click + 0.6) * 30.0).round() as u64]).unwrap();
    let f = &frames[0];
    let mid = mean_rgb(f, 380, 230, 40, 40);
    assert!(mid[2] > 200.0 && mid[0] < 60.0, "button fills the centre: {mid:?}");
    let magenta = (0..500).any(|y| {
        (0..800).any(|x| {
            let p = f.get_pixel(x, y);
            p[0] > 200 && p[1] < 80 && p[2] > 200
        })
    });
    assert!(magenta, "highlight outline on screen");

    // loading again reuses the recording (same video path)
    let l2 = pipeline::load(&pp).unwrap();
    assert_eq!(l2.project.assets["app"].path, l.project.assets["app"].path);
}

#[test]
fn unknown_steps_and_broken_scripts_are_load_errors() {
    if !ready() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let pp = setup(d.path(), "main: [{id: 'a', clip: 'app', in: 'step:nope'}]");
    let Err(LoadError::Invalid(diags)) = pipeline::load(&pp) else { panic!("expected a load error") };
    assert!(diags.iter().any(|x| x.code == "E_UNKNOWN_STEP" && x.path == "/main/0/in"), "{diags:?}");

    std::fs::write(
        d.path().join("app.demo.json5"),
        "{viewport: {w: 800, h: 500, scale: 1}, url: 'index.html', steps: [{click: '#gone', timeout: 0.4}]}",
    )
    .unwrap();
    let Err(LoadError::Invalid(diags)) = pipeline::load(&pp) else { panic!("expected a load error") };
    let e = diags.iter().find(|x| x.code == "E_DEMO_STEP_FAILED").unwrap_or_else(|| panic!("{diags:?}"));
    assert!(e.path == "/assets/app/path" && e.message.contains("#gone"), "{e:?}");
}
