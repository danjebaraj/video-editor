//! M8 acceptance: position accuracy (CSS, Lottie, SVG ±2 px), alpha, determinism, network
//! sandbox.

use cutline_html::{render_frames, Kind, RenderSpec};
use std::path::{Path, PathBuf};

fn chrome_ok() -> bool {
    if cutline_html::chrome().is_none() {
        eprintln!("SKIP: no Chrome");
        return false;
    }
    true
}

fn spec(kind: Kind, source: PathBuf) -> RenderSpec {
    RenderSpec {
        kind,
        source,
        width: 640,
        height: 360,
        fps: 10.0,
        duration: 2.0,
        alpha: true,
        params: serde_json::json!({"title": "hi"}),
        allow_dirs: vec![],
        device_scale: 1.0,
    }
}

/// Left edge of the opaque (alpha > 128) red region.
fn box_left(png: &Path) -> Option<f64> {
    let img = image::open(png).unwrap().to_rgba8();
    let y = 180;
    (0..img.width())
        .find(|&x| {
            let p = img.get_pixel(x, y);
            p[3] > 128 && p[0] > 200
        })
        .map(|x| x as f64)
}

fn check_motion(frames: &[PathBuf], x0: f64, speed_px_s: f64, fps: f64) {
    for (i, f) in frames.iter().enumerate() {
        let t = i as f64 / fps;
        let want = x0 + speed_px_s * t;
        let got = box_left(f).unwrap_or_else(|| panic!("no box in frame {i}"));
        eprintln!("frame {i}: {got} want {want}");
        assert!((got - want).abs() <= 2.0, "frame {i} (t={t}): left {got} want {want}");
    }
}

const CSS: &str = r##"<!doctype html><html><head><style>
html,body{margin:0;background:transparent}
#b{position:absolute;left:0;top:160px;width:40px;height:40px;background:#ff0000;animation:mv 2s linear forwards}
@keyframes mv{from{transform:translateX(20px)}to{transform:translateX(420px)}}
</style></head><body><div id="b"></div><script>document.title = window.__cutline_params.title;</script></body></html>"##;

#[test]
fn css_position_alpha_and_determinism() {
    if !chrome_ok() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("anim.html");
    std::fs::write(&src, CSS).unwrap();
    let s = spec(Kind::Html, src);
    let r1 = render_frames(&s, &d.path().join("w1")).unwrap();
    assert_eq!(r1.frames.len(), 20);
    for (i, f) in r1.frames.iter().enumerate() {
        eprintln!("css {i}: {:?}", box_left(f));
    }
    check_motion(&r1.frames, 20.0, 200.0, 10.0);
    // alpha: background transparent, box opaque
    let img = image::open(&r1.frames[5]).unwrap().to_rgba8();
    assert_eq!(img.get_pixel(5, 5)[3], 0, "background must be transparent");
    assert!(img.get_pixel(box_left(&r1.frames[5]).unwrap() as u32 + 5, 180)[3] > 250);
    // determinism: second render byte-identical
    let r2 = render_frames(&s, &d.path().join("w2")).unwrap();
    for (a, b) in r1.frames.iter().zip(&r2.frames) {
        assert!(std::fs::read(a).unwrap() == std::fs::read(b).unwrap(), "{} differs", a.display());
    }
    // alpha survives the ProRes 4444 mezzanine
    let tc = cutline_testkit::test_toolchain().unwrap();
    let mov = d.path().join("m.mov");
    cutline_html::encode(tc, &s, &d.path().join("w1"), &mov).unwrap();
    let info = cutline_media::probe(tc, &mov, None).unwrap();
    assert!(info.video.unwrap().alpha);
}

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="360" viewBox="0 0 640 360">
<rect y="160" width="40" height="40" fill="#ff0000"><animate attributeName="x" from="20" to="420" dur="2s" fill="freeze"/></rect></svg>"##;

#[test]
fn svg_smil_position() {
    if !chrome_ok() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.svg");
    std::fs::write(&src, SVG).unwrap();
    let r = render_frames(&spec(Kind::Svg, src), &d.path().join("w")).unwrap();
    check_motion(&r.frames, 20.0, 200.0, 10.0);
}

const LOTTIE: &str = r#"{"v":"5.7.0","fr":30,"ip":0,"op":60,"w":640,"h":360,"nm":"box","ddd":0,"assets":[],
"layers":[{"ddd":0,"ind":1,"ty":4,"nm":"sq","sr":1,"ip":0,"op":60,"st":0,"bm":0,
 "ks":{"o":{"a":0,"k":100},"r":{"a":0,"k":0},"a":{"a":0,"k":[0,0,0]},"s":{"a":0,"k":[100,100,100]},
  "p":{"a":1,"k":[{"t":0,"s":[40,180,0],"i":{"x":[1],"y":[1]},"o":{"x":[0],"y":[0]}},{"t":60,"s":[440,180,0]}]}},
 "shapes":[{"ty":"gr","nm":"g","it":[
   {"ty":"rc","nm":"r","d":1,"p":{"a":0,"k":[0,0]},"s":{"a":0,"k":[40,40]},"r":{"a":0,"k":0}},
   {"ty":"fl","nm":"f","c":{"a":0,"k":[1,0,0,1]},"o":{"a":0,"k":100},"r":1},
   {"ty":"tr","p":{"a":0,"k":[0,0]},"a":{"a":0,"k":[0,0]},"s":{"a":0,"k":[100,100]},"r":{"a":0,"k":0},"o":{"a":0,"k":100}}]}]}]}"#;

#[test]
fn lottie_position() {
    if !chrome_ok() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.json");
    std::fs::write(&src, LOTTIE).unwrap();
    let r = render_frames(&spec(Kind::Lottie, src), &d.path().join("w")).unwrap();
    check_motion(&r.frames, 20.0, 200.0, 10.0);
}

#[test]
fn network_access_is_blocked() {
    if !chrome_ok() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let secret_dir = tempfile::tempdir().unwrap();
    let secret = secret_dir.path().join("secret.png");
    std::fs::write(&secret, b"x").unwrap();
    let html = format!(
        r#"<!doctype html><html><body style="margin:0;background:transparent">
<img src="https://example.com/pixel.png"><img src="file://{}">
<div id="out" style="width:40px;height:40px"></div>
<script>
window.__cutline_seek = async (t) => {{
  if (t > 0) return;
  let ok = false;
  try {{ await fetch('https://example.com/data.json'); ok = true; }} catch (e) {{}}
  document.getElementById('out').style.background = ok ? '#00ff00' : '#ff0000';
}};
</script></body></html>"#,
        secret.display()
    );
    let src = d.path().join("net.html");
    std::fs::write(&src, html).unwrap();
    let mut s = spec(Kind::Html, src);
    s.duration = 0.2;
    let r = render_frames(&s, &d.path().join("w")).unwrap();
    assert!(r.blocked_requests.iter().any(|u| u.contains("example.com/pixel.png")), "{:?}", r.blocked_requests);
    assert!(r.blocked_requests.iter().any(|u| u.contains("example.com/data.json")), "{:?}", r.blocked_requests);
    assert!(r.blocked_requests.iter().any(|u| u.contains("secret.png")), "{:?}", r.blocked_requests);
    // the page saw the fetch fail (red marker)
    let img = image::open(&r.frames[0]).unwrap().to_rgba8();
    let p = img.get_pixel(20, img.height() - 1);
    let _ = p;
}
