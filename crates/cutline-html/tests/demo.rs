//! Browser recordings: a scripted page session captured to video with an events sidecar.

use cutline_html::demo::{cached, record_cached};
use cutline_testkit::analyze::{dims, mean_rgb, read_frames};
use cutline_testkit::test_toolchain;
use std::path::Path;

fn chrome_ok() -> bool {
    if cutline_html::chrome().is_none() {
        eprintln!("SKIP: no Chrome");
        return false;
    }
    true
}

/// A tiny app: a button at (100, 80) 200×60 that turns the panel at (400, 80) 300×200 from red
/// to green, an input at (100, 200), and a far-down target that needs scrolling.
const APP: &str = r#"<!doctype html><html><head><meta charset="utf-8"><style>
html, body { margin: 0; background: #ffffff; font-family: sans-serif; }
#go { position: absolute; left: 100px; top: 80px; width: 200px; height: 60px; border: 0; background: #2563eb; color: white; font-size: 18px; }
#go:hover { background: #1d4ed8; }
#panel { position: absolute; left: 400px; top: 80px; width: 300px; height: 200px; background: rgb(255, 0, 0); }
#panel.on { background: rgb(0, 200, 0); }
#email { position: absolute; left: 100px; top: 200px; width: 240px; height: 32px; font-size: 16px; }
#echo { position: absolute; left: 100px; top: 260px; width: 240px; height: 30px; }
#far { position: absolute; left: 100px; top: 1500px; width: 300px; height: 100px; background: rgb(0, 0, 255); }
#end { position: absolute; top: 2400px; height: 10px; width: 10px; }
</style></head><body>
<button id="go">Export</button><div id="panel"></div>
<input id="email"><div id="echo"></div><div id="far">Invoices</div><div id="end"></div>
<script>
document.getElementById('go').addEventListener('click', () => document.getElementById('panel').classList.add('on'));
document.getElementById('email').addEventListener('input', e => { document.getElementById('echo').textContent = e.target.value; });
document.getElementById('email').addEventListener('keydown', e => { if (e.key === 'Enter') document.body.dataset.sent = '1'; });
</script></body></html>"#;

fn setup(d: &Path, steps: &str) -> std::path::PathBuf {
    std::fs::write(d.join("index.html"), APP).unwrap();
    let p = d.join("app.demo.json5");
    std::fs::write(
        &p,
        format!("{{viewport: {{w: 800, h: 500, scale: 1}}, url: 'index.html', settle: 0.4, steps: [{steps}]}}"),
    )
    .unwrap();
    p
}

#[test]
fn records_a_session_with_steps_cursor_and_rects() {
    if !chrome_ok() {
        return;
    }
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let script = setup(
        d.path(),
        "{wait: 0.4},
         {click: \"button:has-text('Export')\", name: 'export'},
         {type: {into: '#email', text: 'ada@example.com', delay: 20}, name: 'email'},
         {wait: 'text=ada@example.com', timeout: 2},
         {press: 'Enter'},
         {scroll: {to: 'text=Invoices', duration: 0.5}, name: 'invoices'}",
    );
    let cache = d.path().join("cache");
    assert!(cached(&script, &cache).unwrap().is_none());
    let rec = record_cached(tc, &script, &cache, false).unwrap();
    let ev = &rec.events;
    assert_eq!((ev.width, ev.height), (800, 500));
    assert_eq!(dims(tc, &rec.video).unwrap(), (800, 500));
    assert_eq!(ev.names(), vec!["export", "email", "invoices"]);

    // the click step: rect within 2 px of the DOM rect, the pointer arrives at its centre and clicks
    let ex = ev.step("export").unwrap();
    let r = ex.rect.unwrap();
    for (got, want) in [(r[0] * 800.0, 100.0), (r[1] * 500.0, 80.0), (r[2] * 800.0, 200.0), (r[3] * 500.0, 60.0)] {
        assert!((got - want).abs() <= 2.0, "rect {r:?}");
    }
    assert!(ex.t0 < ex.t && ex.t - ex.t0 < 2.0, "{ex:?}");
    let click = ev.cursor.iter().find(|c| c.click).unwrap();
    assert!((click.t - ex.t).abs() < 1e-3);
    assert!((click.x * 800.0 - 200.0).abs() < 1.5 && (click.y * 500.0 - 110.0).abs() < 1.5, "{click:?}");
    assert!(ev.cursor.windows(2).all(|w| w[1].t >= w[0].t), "cursor times increase");
    assert_eq!(ev.keys.len(), 1);
    assert_eq!(ev.keys[0].keys, vec!["Enter"]);
    // the scroll target ends up centred in the viewport
    let inv = ev.step("invoices").unwrap().rect.unwrap();
    assert!(((inv[1] + inv[3] / 2.0) - 0.5).abs() < 0.03, "{inv:?}");

    // the video shows it: the panel is red before the click and green after
    let dur = ev.duration;
    assert!(dur > 3.0 && dur < 20.0, "{dur}");
    let fr = |t: f64| (t * 30.0).round() as u64;
    let f = read_frames(tc, &rec.video, &[fr((ex.t - 0.3).max(0.0)), fr(ex.t + 0.35)]).unwrap();
    let before = mean_rgb(&f[0], 450, 120, 200, 120);
    let after = mean_rgb(&f[1], 450, 120, 200, 120);
    assert!(before[0] > 200.0 && before[1] < 60.0, "red before the click: {before:?}");
    assert!(after[1] > 150.0 && after[0] < 60.0, "green after the click: {after:?}");
    // typed text reached the page (echoed below the field) by the end of the type step
    let em = ev.step("email").unwrap();
    assert!(em.t1 - em.t > 0.2, "typing takes time: {em:?}");

    // the second call is the cache; a forced one records again and matches at settled points
    let again = record_cached(tc, &script, &cache, false).unwrap();
    assert_eq!(again.video, rec.video);
    assert_eq!(&again.events, ev);
    let fresh = record_cached(tc, &script, &cache, true).unwrap();
    let ex2 = fresh.events.step("export").unwrap();
    assert_eq!(ex2.rect, ex.rect, "same rect on a fresh recording");
    let g = read_frames(tc, &fresh.video, &[fr(ex2.t + 0.35)]).unwrap();
    let after2 = mean_rgb(&g[0], 450, 120, 200, 120);
    assert!((after2[1] - after[1]).abs() < 6.0 && (after2[0] - after[0]).abs() < 6.0, "{after:?} vs {after2:?}");
}

#[test]
fn a_failing_step_names_itself() {
    if !chrome_ok() {
        return;
    }
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let script = setup(d.path(), "{click: '#go'}, {click: '#missing', timeout: 0.5}");
    let e = record_cached(tc, &script, &d.path().join("cache"), false).unwrap_err();
    let msg = format!("{e:#}");
    assert!(msg.contains("step 1") && msg.contains("#missing"), "{msg}");
}
