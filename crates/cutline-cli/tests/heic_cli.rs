//! HEIC/HEIF photos from the command line (FEAT-0004): probe, `still`, validate, dry run,
//! render with QA, and doctor.

mod common;
use common::*;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/heic").join(name)
}

fn heif_ok() -> bool {
    match cutline_testkit::test_toolchain() {
        Some(tc) if cutline_deps::tools::parse_version(&tc.version) >= cutline_deps::tools::HEIF_MIN_VERSION => true,
        Some(tc) => {
            eprintln!("SKIP: ffmpeg {} can't compose HEIF tile grids", tc.version);
            false
        }
        None => false,
    }
}

#[test]
fn probe_and_still() {
    if !heif_ok() {
        return;
    }
    let (code, v, err) = run_json(bin().arg("probe").arg(fixture("portrait-tiled.heic")));
    assert_eq!(code, 0, "{err}");
    assert_eq!(v["kind"], "image");
    assert_eq!(v["video"]["display_width"], 3024);
    assert_eq!(v["video"]["display_height"], 4032);

    let d = tempfile::tempdir().unwrap();
    let jpg = d.path().join("x.jpg");
    let (code, v, err) = run_json(
        bin().arg("still").arg(fixture("portrait-tiled.heic")).arg("--out").arg(&jpg).args(["--max-side", "1568"]),
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(1176), Some(1568)));
    let img = image::open(&jpg).expect("a JPEG the image crate decodes");
    assert_eq!((img.width(), img.height()), (1176, 1568));
    assert_eq!(image::ImageFormat::from_path(&jpg).unwrap(), image::ImageFormat::Jpeg);

    // no --max-side: full size; PNG by extension; never upscaled
    let png = d.path().join("y.png");
    let (code, v, _) =
        run_json(bin().arg("still").arg(fixture("single.heif")).arg("--out").arg(&png).args(["--max-side", "5000"]));
    assert_eq!(code, 0);
    assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(800), Some(600)));
    assert!(matches!(image::open(&png).unwrap(), image::DynamicImage::ImageRgb8(_)), "no alpha when opaque");

    // other images work too
    let src = d.path().join("plain.png");
    image::RgbImage::from_pixel(40, 20, image::Rgb([10, 200, 30])).save(&src).unwrap();
    let (code, v, _) =
        run_json(bin().arg("still").arg(&src).arg("--out").arg(d.path().join("p.jpg")).args(["--max-side", "10"]));
    assert_eq!(code, 0);
    assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(10), Some(5)));

    // an undecodable input: exit 1 with E_IMAGE_DECODE; a bad extension is a usage error
    let bad = d.path().join("bad.heic");
    std::fs::write(&bad, &std::fs::read(fixture("portrait-tiled.heic")).unwrap()[..10 * 1024]).unwrap();
    let out = bin().arg("still").arg(&bad).arg("--out").arg(d.path().join("z.jpg")).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.trim().lines().last().unwrap().contains("E_IMAGE_DECODE"), "{err}");
    assert!(!d.path().join("z.jpg").exists());
    let out = bin().arg("still").arg(fixture("single.heif")).arg("--out").arg(d.path().join("z.gif")).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

fn write_project(d: &Path, photo: &Path) -> PathBuf {
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'heic', target: 'shorts', fps: 30,
              assets: {{photo: {{path: '{}'}}, small: {{path: '{}'}}}},
              audio: {{voice_chain: 'raw'}},
              main: [{{id: 'a', clip: 'photo', duration: 3, reframe: {{mode: 'fit_blur'}}}}],
              layers: [{{type: 'image', asset: 'small', start: 0, end: 3, position: {{x: 0.5, y: 0.85}}, scale: 0.4}}]}}",
            photo.display(),
            fixture("single.heif").display()
        ),
    )
    .unwrap();
    p
}

#[test]
fn validate_render_and_qa_a_shorts_project() {
    if !heif_ok() {
        return;
    }
    let Some(tc) = cutline_testkit::test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let photo = d.path().join("IMG_4821.HEIC");
    std::fs::copy(fixture("portrait-tiled.heic"), &photo).unwrap();
    let pf = write_project(d.path(), &photo);
    let (code, v, err) = run_json(bin().arg("validate").arg(&pf));
    assert_eq!(code, 0, "{err} {v}");

    // the decode is listed in the dry run like other preparatory commands
    let (code, v, _) = run_json(bin().args(["render", "--dry-run"]).arg(&pf));
    assert_eq!(code, 0);
    let stills = v["stills"].as_array().unwrap();
    assert_eq!(stills.len(), 2, "{v}");
    assert!(stills.iter().all(|s| s["command"].as_str().unwrap().contains("-frames:v 1")));
    let out = bin().args(["render", "--dry-run"]).arg(&pf).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("# decode still photo"));

    let (code, v, err) = run_json(bin().arg("render").arg(&pf));
    assert_eq!(code, 0, "{err} {v}");
    let r = &v["exports"][0];
    assert_eq!(r["qa"]["ok"], true, "{}", r["qa"]);
    // credits name the photos, not the decoded stills
    let credits = std::fs::read_to_string(r["credits_path"].as_str().unwrap()).unwrap();
    assert!(credits.contains("IMG_4821.HEIC") && credits.contains("single.heif"), "{credits}");
    assert!(!credits.contains(".png"), "{credits}");

    // upright in the delivered frame: the photo's top half is blue (left) and red (right)
    let video = PathBuf::from(r["output"].as_str().unwrap());
    let f = &cutline_testkit::analyze::read_frames(tc, &video, &[45]).unwrap()[0];
    assert_eq!(f.dimensions(), (1080, 1920));
    // fit_blur: the 3:4 photo fills the width (1080×1440) centred, from y = 240
    let tl = cutline_testkit::analyze::mean_rgb(f, 120, 400, 200, 200);
    let tr = cutline_testkit::analyze::mean_rgb(f, 760, 400, 200, 200);
    assert!(tl[2] > 180.0 && tl[0] < 80.0, "top-left blue {tl:?}");
    assert!(tr[0] > 180.0 && tr[2] < 80.0, "top-right red {tr:?}");

    // a truncated photo: exit 1, E_IMAGE_DECODE at its asset
    std::fs::write(&photo, &std::fs::read(fixture("portrait-tiled.heic")).unwrap()[..10 * 1024]).unwrap();
    let (code, v, _) = run_json(bin().arg("validate").arg(&pf));
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["errors"][0]["code"], "E_IMAGE_DECODE");
    assert_eq!(v["errors"][0]["path"], "/assets/photo");
}

#[test]
fn doctor_reports_heic_support() {
    if !heif_ok() {
        return;
    }
    let (_, v, _) = run_json(bin().arg("doctor"));
    assert_eq!(v["heic"]["supported"], true, "{}", v["heic"]);
}
