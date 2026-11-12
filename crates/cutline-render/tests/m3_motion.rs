//! M3 acceptance: transition durations, punch-in smoothness, alpha compositing accuracy,
//! golden images per transition.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{golden_compare, mean_rgb, read_all_frames, read_frames};
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

fn render(project: &Path) -> (PathBuf, pipeline::Prepared) {
    let l = pipeline::load(project).unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let opts = RenderOptions { jobs: 3, ..Default::default() };
    let (plan, _) = pipeline::render(&l, &pr, &opts).unwrap();
    (plan.output, pr)
}

fn two_sources(tc: &cutline_deps::Toolchain, d: &Path, w: u32, h: u32) {
    for (n, src) in [("a.mp4", "testsrc2"), ("b.mp4", "smptebars")] {
        let mut c = tc.ffmpeg_cmd();
        c.args(["-f", "lavfi", "-i", &format!("{src}=s={w}x{h}:r=30:d=3"), "-f", "lavfi", "-i", "sine=f=440:d=3"]);
        // explicit, tagged BT.709 conversion: implicit RGB→YUV defaults differ between ffmpeg versions
        c.args(["-vf", "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p"]);
        c.args(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv"]);
        c.args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "12", "-c:a", "aac"]).arg(d.join(n));
        gen::run(c).unwrap();
    }
}

fn nb_frames(tc: &cutline_deps::Toolchain, p: &Path) -> i64 {
    let o = tc
        .ffprobe_cmd()
        .args(["-count_frames", "-select_streams", "v:0", "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0"])
        .arg(p)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).trim().parse().unwrap()
}

fn all_transitions() -> Vec<&'static str> {
    let mut v: Vec<&str> = cutline_core::catalog::XFADE_TRANSITIONS.to_vec();
    v.extend(cutline_core::catalog::CUSTOM_TRANSITIONS.iter().filter(|t| **t != "cut"));
    v
}

/// One timeline chaining every transition type (alternating A/B clips, 0.6 s each, 0.3 s
/// transitions). Checks the exact frame count and the golden midpoint frame of each.
#[test]
fn every_transition_duration_and_golden_midpoint() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    two_sources(tc, d.path(), 320, 180);
    let ts = all_transitions();
    let mut main = vec![];
    for (i, t) in ts.iter().enumerate() {
        let a = if i % 2 == 0 { "a" } else { "b" };
        main.push(format!("{{id: 'c{i}', clip: '{a}', in: 1, out: 1.6}}"));
        main.push(format!("{{transition: {{type: '{t}', duration: 0.3}}}}"));
    }
    let a = if ts.len() % 2 == 0 { "a" } else { "b" };
    main.push(format!("{{id: 'last', clip: '{a}', in: 1, out: 1.6}}"));
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        format!(
            "{{version: 1, name: 'tr', target: 'custom', width: 320, height: 180, fps: 30, assets: {{a: {{path: 'a.mp4'}}, b: {{path: 'b.mp4'}}}}, main: [{}], audio: {{voice_chain: 'raw'}}}}",
            main.join(",")
        ),
    )
    .unwrap();
    let (out, pr) = render(&pp);
    let tl = &pr.timeline;
    let expected = 18 * (ts.len() as i64 + 1) - 9 * ts.len() as i64;
    assert_eq!(tl.total_frames, expected);
    assert_eq!(nb_frames(tc, &out), expected, "rendered frame count");
    let mids: Vec<u64> = tl.transitions.iter().map(|t| (t.start + t.frames / 2) as u64).collect();
    let frames = read_frames(tc, &out, &mids).unwrap();
    // xfade progress math differs between ffmpeg major versions, so goldens are per major
    // version (CI uses the lockfile-pinned build).
    let major = cutline_deps::tools::parse_version(&tc.version).0;
    let gold = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/golden/transitions/ffmpeg{major}"));
    if !gold.exists() && std::env::var("CUTLINE_BLESS_MISSING").is_err() && std::env::var("CUTLINE_BLESS").is_err() {
        eprintln!("SKIP golden compare: no goldens for ffmpeg {major} (bless with CUTLINE_BLESS_MISSING=1)");
        return;
    }
    let mut failures = vec![];
    for (t, img) in tl.transitions.iter().zip(frames.iter()) {
        let p = gold.join(format!("{}.png", t.transition.kind));
        if let Err(e) = golden_compare(img, &p, 0.97) {
            failures.push(format!("{}: {e}", t.transition.kind));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Average square-wave period (px) along a row: distance between first and last edge
/// crossings (sub-pixel) divided by the number of half periods.
fn period(row: &[f64]) -> Option<f64> {
    let m = (row.iter().cloned().fold(f64::MAX, f64::min) + row.iter().cloned().fold(f64::MIN, f64::max)) / 2.0;
    let mut xs = vec![];
    for i in 1..row.len() {
        let (a, b) = (row[i - 1] - m, row[i] - m);
        if a == 0.0 || a.signum() != b.signum() {
            xs.push((i - 1) as f64 + a / (a - b));
        }
    }
    (xs.len() >= 3).then(|| 2.0 * (xs[xs.len() - 1] - xs[0]) / (xs.len() - 1) as f64)
}

#[test]
fn punch_in_is_smooth_and_monotone() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    gen::grid_video(tc, &d.path().join("grid.mp4"), 1280, 720, 30, 3.0, 40).unwrap();
    let pp = d.path().join("p.json5");
    let (at, dur, scale) = (0.5, 1.0, 1.3);
    std::fs::write(
        &pp,
        format!(
            "{{version: 1, name: 'punch', target: 'custom', width: 1280, height: 720, fps: 30, assets: {{g: {{path: 'grid.mp4'}}}}, main: [{{id: 'c', clip: 'g', in: 0, out: 2.5, effects: [{{type: 'punch_in', at: {at}, scale: {scale}, duration: {dur}, ease: 'out_cubic'}}]}}]}}"
        ),
    )
    .unwrap();
    let (out, _) = render(&pp);
    // central horizontal band (rows through cell centers avoid edges)
    let frames = read_all_frames(tc, &out, Some((240, 340, 800, 40))).unwrap();
    let periods: Vec<f64> = frames
        .iter()
        .map(|f| {
            // average several rows for robustness
            let mut acc = 0.0;
            let mut n = 0.0;
            for y in [10u32, 20, 30] {
                let row: Vec<f64> = (0..f.width()).map(|x| f.get_pixel(x, y)[1] as f64).collect();
                if let Some(p) = period(&row) {
                    acc += p;
                    n += 1.0;
                }
            }
            acc / n
        })
        .collect();
    let p0 = periods[0];
    let scales: Vec<f64> = periods.iter().map(|p| p / p0).collect();
    // matches the ease curve within 1.5 %
    for (i, s) in scales.iter().enumerate() {
        let t = i as f64 / 30.0;
        let want =
            1.0 + (scale - 1.0) * cutline_render::effects::ease(cutline_core::model::Ease::OutCubic, (t - at) / dur);
        assert!((s - want).abs() / want < 0.015, "frame {i}: scale {s:.4} want {want:.4}");
    }
    // monotone non-decreasing: no reversals beyond measurement noise
    for w in scales.windows(2) {
        assert!(w[1] >= w[0] - 0.002, "scale reversal {:.4} → {:.4}", w[0], w[1]);
    }
    // jitter-free: second differences small during ease-out (no stair spikes)
    let start = (at * 30.0) as usize + 2;
    let end = ((at + dur) * 30.0) as usize;
    for w in scales[start..end].windows(3) {
        let d2 = (w[2] - 2.0 * w[1] + w[0]).abs();
        assert!(d2 < 0.006, "jitter {d2:.4}");
    }
}

#[test]
fn alpha_overlays_composite_correctly() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let bg = [60u8, 110, 170];
    gen::still(tc, &d.path().join("bg.png"), 640, 360, &format!("0x{:02x}{:02x}{:02x}", bg[0], bg[1], bg[2])).unwrap();
    let alpha = 192u8;
    // static centered circle, red, alpha 192
    let graph = format!("color=c=black@0:s=640x360:r=30:d=2,format=rgba,geq=r='255*lte(hypot(X-320,Y-180),100)':g='40*lte(hypot(X-320,Y-180),100)':b='40*lte(hypot(X-320,Y-180),100)':a='{alpha}*lte(hypot(X-320,Y-180),100)'");
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &graph, "-c:v", "prores_ks", "-profile:v", "4444", "-pix_fmt", "yuva444p10le"])
        .arg(d.path().join("o.mov"));
    gen::run(c).unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        &graph,
        "-c:v",
        "libvpx-vp9",
        "-pix_fmt",
        "yuva420p",
        "-crf",
        "4",
        "-b:v",
        "0",
        "-deadline",
        "good",
        "-auto-alt-ref",
        "0",
    ])
    .arg(d.path().join("o.webm"));
    gen::run(c).unwrap();
    std::fs::create_dir_all(d.path().join("seq")).unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", &graph, "-frames:v", "60", "-start_number", "0"]).arg(d.path().join("seq/%04d.png"));
    gen::run(c).unwrap();
    let _ = alpha;
    for (id, path) in [("prores", "o.mov"), ("vp9", "o.webm"), ("seq", "seq/%04d.png")] {
        let pp = d.path().join(format!("{id}.json5"));
        std::fs::write(
            &pp,
            format!(
                "{{version: 1, name: '{id}', target: 'custom', width: 640, height: 360, fps: 30, assets: {{bg: {{path: 'bg.png'}}, o: {{path: '{path}', alpha: true, fps: 30}}}}, main: [{{id: 'c', clip: 'bg', duration: 2}}], layers: [{{type: 'video', asset: 'o', start: 0, scale: 1.0, position: 'center'}}]}}"
            ),
        )
        .unwrap();
        // expectation from the decoded overlay itself (fixture encodes are lossy)
        let mut dc = std::process::Command::new(&tc.ffmpeg);
        dc.args(["-hide_banner", "-loglevel", "error"]);
        if path.ends_with(".webm") {
            dc.args(["-c:v", "libvpx-vp9"]);
        }
        if path.contains('%') {
            dc.args(["-framerate", "30"]);
        }
        let o = dc
            .arg("-i")
            .arg(d.path().join(path))
            .args([
                "-vf",
                "select=eq(n\\,30),crop=40:40:300:160",
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ])
            .output()
            .unwrap();
        let px = &o.stdout;
        assert_eq!(px.len(), 40 * 40 * 4, "{}", String::from_utf8_lossy(&o.stderr));
        let mut m = [0f64; 4];
        for c in px.chunks(4) {
            for i in 0..4 {
                m[i] += c[i] as f64 / 1600.0;
            }
        }
        let a = m[3] / 255.0;
        let want: Vec<f64> = (0..3).map(|i| a * m[i] + (1.0 - a) * bg[i] as f64).collect();
        let (out, _) = render(&pp);
        let f = &read_frames(tc, &out, &[30]).unwrap()[0];
        let inside = mean_rgb(f, 300, 160, 40, 40);
        let outside = mean_rgb(f, 20, 20, 40, 40);
        for i in 0..3 {
            assert!((inside[i] - want[i]).abs() <= 3.0, "{id}: composited {inside:?} want {want:?}");
            assert!((outside[i] - bg[i] as f64).abs() <= 3.0, "{id}: background {outside:?} want {bg:?}");
        }
    }
}
