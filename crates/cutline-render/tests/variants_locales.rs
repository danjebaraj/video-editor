//! Variants (cuts of the edit) and locales (languages) from one project.

use cutline_core::model::Layer;
use cutline_render::pipeline::{self, LoadError};
use cutline_render::select::Selection;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

fn words(d: &Path, name: &str, w: &[(&str, f64, f64)]) {
    let v: Vec<_> = w.iter().map(|(t, s, e)| serde_json::json!({"w": t, "start": s, "end": e})).collect();
    std::fs::write(d.join(name), serde_json::json!({"asset": "vo", "words": v}).to_string()).unwrap();
}

fn setup(d: &Path) -> PathBuf {
    let tc = test_toolchain().unwrap();
    gen::still(tc, &d.join("bg.png"), 320, 180, "0x223344").unwrap();
    for (f, len) in [("vo.wav", 12), ("vo.de.wav", 14), ("vo.short.wav", 6)] {
        let mut c = tc.ffmpeg_cmd();
        c.args(["-f", "lavfi", "-i", &format!("sine=f=220:d={len}"), "-ac", "2"]).arg(d.join(f));
        gen::run(c).unwrap();
    }
    // the same script in two languages, and a short cut of it
    words(
        d,
        "vo.json",
        &[
            ("Meet", 0.2, 0.5),
            ("Glance.", 0.5, 1.0),
            ("World", 4.0, 4.3),
            ("clocks", 4.3, 4.8),
            ("Try", 9.0, 9.3),
            ("it.", 9.3, 9.6),
        ],
    );
    words(
        d,
        "vo.de.json",
        &[
            ("Das", 0.2, 0.4),
            ("ist", 0.4, 0.6),
            ("Glance.", 0.6, 1.1),
            ("Weltuhren", 5.5, 6.3),
            ("Jetzt", 11.0, 11.4),
            ("testen.", 11.4, 12.0),
        ],
    );
    words(d, "vo.short.json", &[("Meet", 0.2, 0.5), ("Glance.", 0.5, 1.0), ("Try", 3.0, 3.3), ("it.", 3.3, 3.6)]);
    std::fs::write(d.join("strings.de.json5"), "{title: 'Weltuhren', cta: 'Jetzt testen'}").unwrap();
    let p = d.join("demo.json5");
    std::fs::write(
        &p,
        "{version: 1, target: 'custom', width: 320, height: 180, fps: 30,
          assets: {bg: {path: 'bg.png'}, vo: {path: 'vo.wav', role: 'voice', transcript: 'vo.json'}},
          cues: {clocks: 'World clocks', cta: 'Try it'},
          strings: {title: 'World clocks', cta: 'Try it'},
          variants: {
            full: {},
            teaser: {exclude: ['clocks'], max_duration: 5, exports: ['li'], assets: {vo: {path: 'vo.short.wav', transcript: 'vo.short.json'}}},
          },
          locales: {de: {assets: {vo: {path: 'vo.de.wav', transcript: 'vo.de.json'}}, cues: {clocks: 'Weltuhren', cta: 'Jetzt testen'},
                         strings: 'strings.de.json5', lang: 'de'}},
          main: [
            {id: 'intro', clip: 'bg', end_at: 'cue:clocks', except: ['teaser']},
            {id: 'intro_short', clip: 'bg', end_at: 'cue:cta', only: ['teaser']},
            {id: 'clocks', clip: 'bg', end_at: 'cue:cta'},
            {id: 'end', clip: 'bg', duration: 2},
          ],
          layers: [
            {type: 'audio', asset: 'vo', start: 0},
            {type: 'text', template: 'lower_third', text: '@title', start: 'cue:clocks', duration: 2, except: ['teaser']},
            {type: 'text', template: 'callout', text: '@cta', anchor: 'end'},
          ],
          exports: [{name: 'yt'}, {name: 'li', target: 'square'}]}",
    )
    .unwrap();
    p
}

fn sel(variant: Option<&str>, locale: Option<&str>) -> Selection {
    Selection { variant: variant.map(str::to_string), locale: locale.map(str::to_string) }
}

fn texts(l: &pipeline::Loaded) -> Vec<String> {
    l.project.layers.iter().filter_map(|x| if let Layer::Text { text, .. } = x { text.clone() } else { None }).collect()
}

#[test]
fn one_project_gives_a_full_cut_a_teaser_and_a_german_version() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let pp = setup(d.path());

    // default: the first variant by name, base language
    let full = pipeline::load_with(&pp, &sel(None, None)).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&full.diags), "{:?}", full.diags);
    assert_eq!(full.selection, sel(Some("full"), None));
    assert_eq!(
        (full.variants.clone(), full.locales.clone()),
        (vec!["full".to_string(), "teaser".to_string()], vec!["de".to_string()])
    );
    let tl = full.timeline.as_ref().unwrap();
    assert_eq!(tl.clips.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["intro", "clocks", "end"]);
    assert_eq!(
        (tl.clips[1].start, tl.clips[2].start),
        (120, 270),
        "cuts on \"World clocks\" (4.0 s) and \"Try it\" (9.0 s)"
    );
    assert_eq!(texts(&full), vec!["World clocks", "Try it"]);
    assert_eq!(full.project.name.as_deref(), Some("demo-full"));
    assert_eq!(full.project.exports.len(), 2);

    // teaser: another cut, a shorter voice-over, one export; the cue lands where the short script says
    let teaser = pipeline::load_with(&pp, &sel(Some("teaser"), None)).unwrap_or_else(|e| panic!("{e}"));
    let tl = teaser.timeline.as_ref().unwrap();
    assert_eq!(tl.clips.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["intro_short", "end"]);
    assert_eq!(tl.clips[1].start, 90, "cut on \"Try it\" at 3.0 s of the short voice-over");
    assert_eq!(texts(&teaser), vec!["Try it"]);
    assert_eq!(teaser.project.exports.iter().map(|e| e.name.clone().unwrap()).collect::<Vec<_>>(), vec!["li"]);
    assert!(teaser.project.assets["vo"].path.ends_with("vo.short.wav"));
    assert_eq!(teaser.project.name.as_deref(), Some("demo-teaser"));
    // 5.0 s of edit is within its limit of 5
    assert!(!teaser.diags.iter().any(|x| x.code == "W_VARIANT_TOO_LONG"), "{:?}", teaser.diags);

    // German: its narration moves the cuts, its strings replace the text
    let de = pipeline::load_with(&pp, &sel(Some("full"), Some("de"))).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&de.diags), "{:?}", de.diags);
    let tl = de.timeline.as_ref().unwrap();
    assert_eq!(
        (tl.clips[1].start, tl.clips[2].start),
        (165, 330),
        "cuts on \"Weltuhren\" (5.5 s) and \"Jetzt testen\" (11.0 s)"
    );
    assert_eq!(texts(&de), vec!["Weltuhren", "Jetzt testen"]);
    assert_eq!(
        de.project.captions.as_ref().and_then(|c| c.lang.clone()),
        None,
        "no captions block to set a language on"
    );
    assert_eq!(de.project.name.as_deref(), Some("demo-full-de"));
    // the lower third follows its cue in German too
    assert_eq!(tl.layers.iter().find(|l| matches!(l.layer, Layer::Text { .. })).unwrap().start, 165);
}

#[test]
fn a_variant_over_its_limit_warns_and_unknown_selections_fail() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let pp = setup(d.path());
    let text = std::fs::read_to_string(&pp).unwrap().replace("max_duration: 5", "max_duration: 4");
    std::fs::write(&pp, text).unwrap();
    let teaser = pipeline::load_with(&pp, &sel(Some("teaser"), None)).unwrap();
    let w =
        teaser.diags.iter().find(|x| x.code == "W_VARIANT_TOO_LONG").unwrap_or_else(|| panic!("{:?}", teaser.diags));
    assert_eq!(w.path, "/variants/teaser/max_duration");
    for s in [sel(Some("nope"), None), sel(None, Some("fr"))] {
        let Err(LoadError::Invalid(diags)) = pipeline::load_with(&pp, &s) else {
            panic!("expected an error for {s:?}")
        };
        assert!(diags.iter().any(|x| x.code.starts_with("E_NO_SUCH_")), "{diags:?}");
    }
}
