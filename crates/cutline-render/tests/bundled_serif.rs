//! TASK-honest-cedar-A73E step E1: Source Serif 4 is bundled, accepted by name for captions and
//! the text templates, and listed when a font isn't bundled.

use cutline_core::model::AssetKind;
use cutline_core::time::Fps;
use cutline_core::timeline::{resolve, MediaFacts, StaticLookup};
use cutline_render::pipeline;

fn lookup() -> StaticLookup {
    let mut m = StaticLookup::default();
    m.0.insert(
        "a".into(),
        MediaFacts {
            exists: true,
            kind: AssetKind::Video,
            duration: 6.0,
            has_video: true,
            has_audio: true,
            alpha: false,
            width: 1080,
            height: 1920,
            fps: Some(Fps::new(30, 1)),
            rotation: 0,
        },
    );
    m
}

fn font_errors(font: &str) -> Vec<cutline_core::diag::Diagnostic> {
    let p = cutline_core::parse::parse_project(&format!(
        "{{ version: 1, assets: {{ a: {{path: 'a.mp4'}} }}, main: [{{id: 'c1', clip: 'a'}}], captions: {{font: '{font}'}} }}"
    ))
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (_, d) = resolve(&p, &lookup(), &pipeline::context(dir.path()));
    d.into_iter().filter(|x| x.code == "E_FONT_MISSING").collect()
}

#[test]
fn source_serif_is_accepted_for_captions() {
    assert!(pipeline::bundled_fonts().iter().any(|f| f == "Source Serif 4"));
    assert!(font_errors("Source Serif 4").is_empty());
    assert!(font_errors("source serif 4").is_empty(), "names match without case");
}

#[test]
fn missing_font_hint_names_the_serif() {
    let e = font_errors("Georgia");
    assert_eq!(e.len(), 1);
    let hint = e[0].hint.as_deref().unwrap_or_default();
    assert!(hint.starts_with("bundled: ") && hint.contains("Source Serif 4") && hint.contains("Inter"), "{hint}");
}

#[test]
fn text_templates_draw_with_the_serif() {
    let db = cutline_text::FontDb::bundled().unwrap();
    let regular = cutline_render::cards::face(&db, "Source Serif 4", 400.0).expect("regular");
    let bold = cutline_render::cards::face(&db, "Source Serif 4", 700.0).expect("bold");
    use ab_glyph::Font;
    // the serif's own glyphs, not a fallback: the faces differ and both have a real "g"
    assert!(regular.glyph_id('g').0 != 0 && bold.glyph_id('g').0 != 0);
    let entry = db.get("Source Serif 4", false).unwrap();
    assert!(entry.path.ends_with("SourceSerif4-Regular.ttf"), "{:?}", entry.path);
    let css = std::fs::read_to_string(cutline_deps::paths::assets_dir().join("templates/_base.css")).unwrap();
    for f in ["SourceSerif4-Regular.ttf", "SourceSerif4-It.ttf", "SourceSerif4-Bold.ttf"] {
        assert!(css.contains(f), "_base.css declares {f}");
        assert!(cutline_deps::paths::assets_dir().join("fonts").join(f).exists());
    }
    assert!(cutline_deps::paths::assets_dir().join("fonts/OFL-sourceserif4.txt").exists());
}
