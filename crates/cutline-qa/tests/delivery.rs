//! Delivery for business channels: WebM, silent loops, size caps, cut-outs, caption delivery,
//! the 4:5 target, WebVTT and poster files.

use cutline_qa::deliver::render_all;
use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::{dims, read_frames};
use cutline_testkit::gen::{self, FrameIdSource};
use cutline_testkit::test_toolchain;
use std::path::{Path, PathBuf};

/// A 6 s clip whose picture changes all the time (frame ids over a moving test pattern), with
/// a voice-over transcript so captions exist.
fn setup(d: &Path) {
    let tc = test_toolchain().unwrap();
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=6", "-f", "lavfi", "-i", "sine=f=330:d=6"])
        .args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(d.join("v.mp4"));
    gen::run(c).unwrap();
    let words: Vec<_> = ["These", "words", "are", "the", "captions", "of", "this", "test"]
        .iter()
        .enumerate()
        .map(|(i, w)| serde_json::json!({"w": w, "start": 0.5 + i as f64 * 0.6, "end": 1.0 + i as f64 * 0.6}))
        .collect();
    std::fs::write(d.join("v.json"), serde_json::json!({"asset": "v", "words": words}).to_string()).unwrap();
    let _ = FrameIdSource::new(16, 16, 30, 0.1);
}

fn project(d: &Path, exports: &str) -> PathBuf {
    let p = d.join("p.json5");
    std::fs::write(
        &p,
        format!(
            "{{version: 1, name: 'd', target: 'custom', width: 640, height: 360, fps: 30,
              assets: {{v: {{path: 'v.mp4', transcript: 'v.json'}}}},
              main: [{{id: 'a', clip: 'v'}}], captions: {{source: 'auto', style: 'clean_sentence'}},
              audio: {{voice_chain: 'raw'}}, exports: [{exports}]}}"
        ),
    )
    .unwrap();
    p
}

fn finding<'a>(r: &'a cutline_qa::deliver::ExportResult, check: &str) -> &'a cutline_qa::qa::Finding {
    r.qa.findings
        .iter()
        .find(|f| f.check == check)
        .unwrap_or_else(|| panic!("no `{check}` finding: {:?}", r.qa.findings))
}

fn probe(p: &Path) -> serde_json::Value {
    let tc = test_toolchain().unwrap();
    let o = tc.ffprobe_cmd().args(["-print_format", "json", "-show_format", "-show_streams"]).arg(p).output().unwrap();
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn webm_silent_loop_capped_and_cut_out_exports() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path());
    let pp = project(
        d.path(),
        "{name: 'full'},
         {name: 'web', format: 'webm'},
         {name: 'site', audio: false, loop: true, max_bytes: '150KB', range: [1, 5]},
         {name: 'li', target: 'portrait_4x5', captions: 'sidecar', poster: 2.0}",
    );
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    assert!(!cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    let res = render_all(&l, &RenderOptions { jobs: 3, ..Default::default() }, None).unwrap();
    assert_eq!(res.len(), 4);
    for r in &res {
        assert!(r.qa.ok, "{:?}: {}", r.export, cutline_qa::qa::summary(&r.qa));
    }
    let by = |n: &str| res.iter().find(|r| r.export.as_deref() == Some(n)).unwrap();

    // full: MP4 with its side files
    let full = by("full");
    assert!(full.output.ends_with("d-full.mp4"));
    let vtt = std::fs::read_to_string(full.output.with_file_name("d-full.vtt")).unwrap();
    assert!(vtt.starts_with("WEBVTT\n\n1\n00:00:00.500 --> "), "{vtt}");
    assert!(full.output.with_file_name("d-full.poster.jpg").exists());

    // web: VP9 + Opus in WebM, same length
    let web = by("web");
    assert!(web.output.ends_with("d-web.webm"));
    let j = probe(&web.output);
    let codecs: Vec<&str> =
        j["streams"].as_array().unwrap().iter().map(|s| s["codec_name"].as_str().unwrap()).collect();
    assert!(codecs.contains(&"vp9") && codecs.contains(&"opus"), "{codecs:?}");
    assert!(finding(web, "container_codec").ok);
    assert!((web.duration - 6.0).abs() < 0.05);

    // site: 1–5 s cut out, looped (0.5 s shorter), silent, under the cap
    let site = by("site");
    let j = probe(&site.output);
    assert_eq!(j["streams"].as_array().unwrap().len(), 1, "no audio stream");
    assert!((site.duration - 3.5).abs() < 0.04, "{}", site.duration);
    let size = std::fs::metadata(&site.output).unwrap().len();
    assert!(size <= 150_000, "{size} bytes");
    assert!(size > 60_000, "uses its budget: {size} bytes");
    assert!(finding(site, "file_size").ok && finding(site, "loop_seam").ok, "{}", cutline_qa::qa::summary(&site.qa));
    // captions are burned in, so the silent export still reads
    assert!(finding(site, "muted").ok);
    // the last frame leads into the first: both are about 1.5 s into the edit
    let n = (site.duration * 30.0).round() as u64;
    let f = read_frames(tc, &site.output, &[0, n - 1]).unwrap();
    let diff: f64 = f[0].pixels().zip(f[1].pixels()).map(|(a, b)| (a[1] as f64 - b[1] as f64).abs()).sum::<f64>()
        / (640.0 * 360.0 * 255.0);
    assert!(diff < 0.06, "seam difference {diff}");

    // li: 4:5, captions only as files
    let li = by("li");
    assert_eq!(dims(tc, &li.output).unwrap(), (1080, 1350));
    assert!(std::fs::read_to_string(&li.srt_path).unwrap().contains("These words"));
    let pr = pipeline::prepare(&l, Some("li".into()), &l.project.exports[3], false).unwrap();
    assert!(pr.extras.ass_docs.iter().all(|(n, _)| n != "captions.ass"), "sidecar captions are not burned");
    assert!(!pr.extras.srt.is_empty());
}

#[test]
fn silent_export_without_captions_is_flagged_and_bad_exports_are_errors() {
    let Some(_) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    setup(d.path());
    let pp = project(d.path(), "{name: 'quiet', audio: false, captions: 'none'}");
    let l = pipeline::load(&pp).unwrap();
    let res = render_all(&l, &RenderOptions { jobs: 3, ..Default::default() }, None).unwrap();
    let m = finding(&res[0], "muted");
    assert!(!m.ok && m.message.contains("no audio"), "{m:?}");

    let pp = project(
        d.path(),
        "{name: 'a', loop: true}, {name: 'b', max_bytes: 'big'}, {name: 'c', range: [4, 2]}, {name: 'e', range: [0, 99]}",
    );
    let l = pipeline::load(&pp).unwrap();
    let has = |code: &str, path: &str| l.diags.iter().any(|x| x.code == code && x.path == path);
    assert!(has("E_FIELD_CONFLICT", "/exports/0/loop"), "{:?}", l.diags);
    assert!(has("E_BAD_VALUE", "/exports/1/max_bytes"), "{:?}", l.diags);
    assert!(has("E_TIME_OUT_OF_RANGE", "/exports/2/range"), "{:?}", l.diags);
    assert!(has("E_TIME_OUT_OF_RANGE", "/exports/3/range"), "{:?}", l.diags);
}
