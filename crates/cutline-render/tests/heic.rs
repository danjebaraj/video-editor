//! HEIC/HEIF photos (FEAT-0004): probed as whole upright pictures, decoded once into cached
//! stills, rendered like PNGs, and reported as `E_IMAGE_DECODE` when they can't be read.

use cutline_media::cache::Cache;
use cutline_media::still;
use cutline_render::pipeline;
use cutline_testkit::analyze::{golden_compare, mean_rgb};
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/heic").join(name)
}

/// The toolchain, when it can compose HEIF tile grids (ffmpeg ≥ 7.1).
fn heif_toolchain() -> Option<&'static cutline_deps::Toolchain> {
    let tc = test_toolchain()?;
    if cutline_deps::tools::parse_version(&tc.version) < cutline_deps::tools::HEIF_MIN_VERSION {
        eprintln!("SKIP: ffmpeg {} can't compose HEIF tile grids", tc.version);
        return None;
    }
    Some(tc)
}

fn near(c: [f64; 3], want: [f64; 3]) -> bool {
    c.iter().zip(want).all(|(a, b)| (a - b).abs() < 40.0)
}

const BLUE: [f64; 3] = [26.0, 51.0, 230.0];
const RED: [f64; 3] = [230.0, 26.0, 26.0];
const YELLOW: [f64; 3] = [242.0, 217.0, 26.0];
const GREEN: [f64; 3] = [26.0, 204.0, 51.0];

#[test]
fn probe_reports_the_whole_picture_upright() {
    let Some(tc) = heif_toolchain() else { return };
    let i = cutline_media::probe(tc, &fixture("portrait-tiled.heic"), None).unwrap();
    assert_eq!(i.kind, "image");
    let v = i.video.as_ref().unwrap();
    // 48 tiles of 512×512, composed to 4032×3024 and shown rotated
    assert_eq!((v.width, v.height), (4032, 3024));
    assert_eq!((v.display_width, v.display_height), (3024, 4032));
    assert_eq!(i.duration, 0.0);
    let s = cutline_media::probe(tc, &fixture("single.heif"), None).unwrap();
    assert_eq!(s.kind, "image");
    let v = s.video.as_ref().unwrap();
    assert_eq!((v.display_width, v.display_height), (800, 600));
}

#[test]
fn extensions_are_images_in_any_case() {
    use cutline_core::model::AssetKind;
    use cutline_core::timeline::default_kind_from_ext;
    for p in ["a.heic", "b.HEIC", "c.heif", "d.HeIf"] {
        assert_eq!(default_kind_from_ext(p), AssetKind::Image, "{p}");
    }
}

#[test]
fn decode_still_composes_tiles_upright_and_caches() {
    let Some(tc) = heif_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let cache = Cache::new(d.path().join("still"));
    let src = fixture("portrait-tiled.heic");
    let png = still::decode_still(tc, &src, &cache).unwrap();
    let img = image::open(&png).unwrap();
    assert_eq!((img.width(), img.height()), (3024, 4032));
    assert!(matches!(img, image::DynamicImage::ImageRgb8(_)), "8-bit RGB without alpha: {:?}", img.color());
    let img = img.to_rgb8();
    // stored top-left red; orientation 6 turns it a quarter clockwise, so blue is top-left
    for ((x, y), want, name) in [
        ((300, 300), BLUE, "top-left"),
        ((2424, 300), RED, "top-right"),
        ((300, 3432), YELLOW, "bottom-left"),
        ((2424, 3432), GREEN, "bottom-right"),
    ] {
        let c = mean_rgb(&img, x, y, 200, 200);
        assert!(near(c, want), "{name} {c:?}");
    }
    let tl = image::imageops::crop_imm(&img, 0, 0, 64, 64).to_image();
    let gold = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/heic/portrait-tiled-topleft.png");
    golden_compare(&tl, &gold, 0.97).unwrap();

    // the second call is a cache hit: no ffmpeg
    let before = still::decode_count();
    assert_eq!(still::decode_still(tc, &src, &cache).unwrap(), png);
    assert_eq!(still::decode_count(), before);
}

fn project(d: &Path, photo: &Path, layer_photo: &Path) -> PathBuf {
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'heic', target: 'shorts', fps: 30,
              assets: {{photo: {{path: '{}'}}, small: {{path: '{}'}}}},
              audio: {{voice_chain: 'raw'}},
              main: [{{id: 'a', clip: 'photo', duration: 3, reframe: {{mode: 'fit_blur'}}}}],
              layers: [{{type: 'image', asset: 'small', start: 0, end: 3, position: {{x: 0.5, y: 0.8}}, scale: 0.4}}]}}",
            photo.display(),
            layer_photo.display()
        ),
    )
    .unwrap();
    p
}

#[test]
fn heic_assets_load_from_one_decode_and_keep_their_own_path() {
    let Some(_tc) = heif_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    // copies, so this test's decodes are its own (the cache is the project's)
    let (a, b) = (d.path().join("IMG_0001.HEIC"), d.path().join("single.heif"));
    std::fs::copy(fixture("portrait-tiled.heic"), &a).unwrap();
    std::fs::copy(fixture("single.heif"), &b).unwrap();
    let pp = project(d.path(), &a, &b);
    let before = still::decode_count();
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let m = l.media.get("photo").unwrap();
    assert_eq!(m.facts.kind, cutline_core::model::AssetKind::Image);
    assert_eq!((m.facts.width, m.facts.height), (3024, 4032));
    assert_eq!(m.path.extension().and_then(|e| e.to_str()), Some("png"), "renders read the decoded still");
    assert_eq!(
        m.source.as_deref().map(|s| std::fs::canonicalize(s).unwrap()),
        Some(std::fs::canonicalize(&a).unwrap())
    );
    assert_eq!(m.fingerprint, cutline_media::fingerprint(&a).unwrap(), "the photo's own fingerprint");
    assert!(l.media.get("small").unwrap().source.is_some());
    // the project keeps the user's file (credits.txt lists it from there)
    assert!(l.project.assets["photo"].path.ends_with("IMG_0001.HEIC"));
    let after_first = still::decode_count();
    assert!(after_first >= before + 2);
    // loading again decodes nothing (other tests may decode concurrently, so compare the files)
    let l2 = pipeline::load(&pp).unwrap();
    assert_eq!(l2.media.get("photo").unwrap().path, m.path);
    let mtime = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
    assert_eq!(mtime(&l2.media.get("photo").unwrap().path), mtime(&m.path));
}

#[test]
fn an_undecodable_photo_is_e_image_decode_at_its_asset() {
    let Some(_tc) = heif_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let bytes = std::fs::read(fixture("portrait-tiled.heic")).unwrap();
    let bad = d.path().join("broken.heic");
    std::fs::write(&bad, &bytes[..10 * 1024]).unwrap();
    let pp = project(d.path(), &bad, &fixture("single.heif"));
    match pipeline::load(&pp) {
        Err(pipeline::LoadError::Invalid(diags)) => {
            let e = &diags[0];
            assert_eq!(e.code, "E_IMAGE_DECODE");
            assert_eq!(e.path, "/assets/photo");
            assert!(e.message.contains("broken.heic"), "{}", e.message);
            assert_eq!(e.hint.as_deref(), Some(still::DECODE_HINT));
        }
        Err(e) => panic!("expected E_IMAGE_DECODE, got {e}"),
        Ok(_) => panic!("a truncated photo loaded"),
    }
}
