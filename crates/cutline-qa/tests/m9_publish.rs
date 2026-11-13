//! M9 acceptance: paths with spaces and non-ASCII characters, multi-export, chapters,
//! thumbnails.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::gen;
use cutline_testkit::test_toolchain;

#[test]
fn unicode_and_space_paths_render_everything() {
    let Some(tc) = test_toolchain() else { return };
    let root = tempfile::tempdir().unwrap();
    // project dir and asset dirs with ü, spaces, apostrophes, commas and brackets
    let proj = root.path().join("Projekt ü, Dan's [v1]");
    let clips = proj.join("my clips ü");
    std::fs::create_dir_all(&clips).unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=1280x720:r=30:d=4",
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:d=4",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
    ])
    .arg(clips.join("take 1 ü.mp4"));
    gen::run(c).unwrap();
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../cutline-transcribe/tests/fixtures/scribe_sample.json"),
        clips.join("vo ü.scribe.json"),
    )
    .unwrap();
    gen::still(tc, &clips.join("logo ü.png"), 128, 128, "0xFFC400").unwrap();
    // a project-local LUT with a weird name
    std::fs::copy(cutline_deps::paths::assets_dir().join("luts/warm_film.cube"), clips.join("look, ü.cube")).unwrap();
    let pp = proj.join("projekt ü.json5");
    std::fs::write(
        &pp,
        r#"{version: 1, name: "ünicode test", target: "shorts",
          assets: {v: {path: "my clips ü/take 1 ü.mp4", transcript: "my clips ü/vo ü.scribe.json"}, logo: {path: "my clips ü/logo ü.png"}},
          main: [{id: "a", clip: "v", in: 0, out: 2, color: {lut: "my clips ü/look, ü.cube"}},
                 {transition: {type: "fade", duration: 0.3}},
                 {id: "b", clip: "v", in: 2, out: 4, reframe: {mode: "manual", keys: [{t: 0, x: 0.3, y: 0.5}, {t: 1.5, x: 0.7, y: 0.5, zoom: 1.2}]}}],
          layers: [{type: "image", asset: "logo", start: 0, end: "end"},
                   {type: "text", template: "hook_title", text: "Größer {braces}, C:\\path", start: 0, duration: 1.5}],
          captions: {style: "bold_pop"}}"#,
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let res = cutline_qa::deliver::render_all(&l, &RenderOptions::default(), None).unwrap();
    let r = &res[0];
    assert!(r.output.exists());
    assert!(r.output.to_string_lossy().contains("ünicode test"));
    assert!(r.qa.errors == 0, "{:#?}", r.qa.findings.iter().filter(|f| !f.ok).collect::<Vec<_>>());
    assert!(std::fs::read_to_string(&r.srt_path).unwrap().contains("knife"));
}

#[test]
fn multi_export_chapters_thumbnail() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=1920x1080:r=30:d=40",
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:d=40",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
    ])
    .arg(d.path().join("v.mp4"));
    gen::run(c).unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        r#"{version: 1, name: "multi", target: "long_1080p",
          assets: {v: {path: "v.mp4"}},
          main: [{id: "intro", clip: "v", in: 0, out: 12, chapter: "Intro"},
                 {id: "steps", clip: "v", in: 12, out: 26, chapter: "The steps"},
                 {id: "outro", clip: "v", in: 26, out: 38, chapter: "Wrap up"}],
          exports: [{target: "long_1080p"}, {name: "vertical", target: "shorts"}]}"#,
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap();
    let res = cutline_qa::deliver::render_all(&l, &RenderOptions { draft: true, ..Default::default() }, None).unwrap();
    assert_eq!(res.len(), 2);
    let dims = |p: &std::path::Path| {
        let v = cutline_media::probe(tc, p, None).unwrap().video.unwrap();
        (v.width, v.height)
    };
    assert_eq!(dims(&res[0].output), (960, 540));
    assert_eq!(dims(&res[1].output), (540, 960));
    assert!(res[1].output.to_string_lossy().contains("vertical"));
    // only one export
    let one =
        cutline_qa::deliver::render_all(&l, &RenderOptions { draft: true, ..Default::default() }, Some("vertical"))
            .unwrap();
    assert_eq!(one.len(), 1);
    // chapters
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name, &e, false).unwrap();
    let (ch, warn) = cutline_qa::publish::chapters(&pr);
    assert_eq!(cutline_qa::publish::chapters_text(&ch), "00:00 Intro\n00:12 The steps\n00:26 Wrap up");
    assert!(warn.is_empty(), "{warn:?}");
    // thumbnail 1280x720 with text
    let out = d.path().join("t.jpg");
    cutline_qa::publish::thumbnail(&l, &pr, 15.0, Some("Sharpen like a pro"), "hook_title", &out).unwrap();
    assert_eq!(image::image_dimensions(&out).unwrap(), (1280, 720));
}
