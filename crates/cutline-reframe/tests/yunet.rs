//! Real YuNet detector on the public-domain fixture (feature `onnx`; skips without the model).
#![cfg(feature = "onnx")]

use cutline_reframe::Detector;

fn model() -> Option<std::path::PathBuf> {
    let m = std::env::var_os("CUTLINE_FACE_MODEL")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(cutline_reframe::model_path);
    if m.exists() {
        Some(m)
    } else {
        eprintln!("SKIP: face model not installed (cutline deps install face-model)");
        None
    }
}

#[test]
fn detects_face_in_fixture() {
    let Some(m) = model() else { return };
    let mut d = cutline_reframe::yunet::YuNet::load(&m).unwrap();
    eprintln!("{}", d.describe());
    let img =
        image::open(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/face_armstrong.jpg")).unwrap().to_rgb8();
    let faces = d.detect(img.as_raw(), img.width(), img.height()).unwrap();
    eprintln!("{faces:?}");
    assert_eq!(faces.len(), 1, "{faces:?}");
    let f = faces[0];
    assert!((f.cx() - 0.62).abs() < 0.06 && (f.cy() - 0.27).abs() < 0.06, "{f:?}");
    assert!(f.score > 0.8);
}
