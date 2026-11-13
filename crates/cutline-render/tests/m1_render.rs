//! M1 acceptance (real ffmpeg): exact frame mapping on a 5-clip render; segment cache.

use cutline_render::pipeline;
use cutline_render::RenderOptions;
use cutline_testkit::analyze::read_frames;
use cutline_testkit::gen::{self, FrameIdSource};
use cutline_testkit::{decode_frame_id, test_toolchain, FrameIdLayout};
use std::path::Path;

fn sources(dir: &Path) -> FrameIdLayout {
    let tc = test_toolchain().unwrap();
    let layout = FrameIdLayout::centered(1920, 200.0, 64.0);
    for (name, color, hz) in [("a.mp4", "0x3060a0", 440.0), ("b.mp4", "0xa06030", 660.0)] {
        let mut s = FrameIdSource::new(1920, 1080, 30, 8.0);
        s.color = color.into();
        s.tone_hz = Some(hz);
        s.layout = layout;
        gen::frameid_video(tc, &dir.join(name), &s).unwrap();
    }
    layout
}

fn write_project(dir: &Path, clip3_out: f64) -> std::path::PathBuf {
    let p = dir.join("p.json5");
    std::fs::write(
        &p,
        format!(
            r#"{{version: 1, name: "fm", target: "shorts",
              assets: {{a: {{path: "a.mp4"}}, b: {{path: "b.mp4"}}}},
              main: [
                {{id: "c1", clip: "a", in: 1.0, out: 2.0}},
                {{id: "c2", clip: "b", in: "f:45", out: 2.5}},
                {{transition: {{type: "fade", duration: 0.2}}}},
                {{id: "c3", clip: "a", in: 4.0, out: {clip3_out}}},
                {{id: "c4", clip: "b", in: 3.0, out: 5.0, speed: 2}},
                {{id: "c5", clip: "a", in: "00:06.500", out: 7.5}},
              ]}}"#
        ),
    )
    .unwrap();
    p
}

#[test]
fn exact_frame_mapping_and_segment_cache() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let layout = sources(d.path());
    let pp = write_project(d.path(), 5.0);
    let l = pipeline::load(&pp).unwrap();
    assert!(l.diags.is_empty(), "{:?}", l.diags);
    let (name, e) = pipeline::exports(&l.project).remove(0);
    let pr = pipeline::prepare(&l, name.clone(), &e, false).unwrap();
    let opts = RenderOptions { jobs: 3, ..Default::default() };
    let (plan, rep) = pipeline::render(&l, &pr, &opts).unwrap();
    assert_eq!(rep.cache_misses, 5);
    let tl = &pr.timeline;
    // expected source frame for each timeline frame outside transitions
    let mut samples = vec![];
    for c in &tl.clips {
        let in_frame = (c.src_in * 30.0).round() as i64;
        for j in [0, 1, c.frames / 2, c.frames - 2, c.frames - 1] {
            let f = c.start + j;
            let in_trans = tl.transitions.iter().any(|t| f >= t.start && f < t.start + t.frames);
            if in_trans || j < 0 || j >= c.frames {
                continue;
            }
            let src = in_frame + (j as f64 * c.speed).round() as i64;
            samples.push((f as u64, src as u16, c.id.clone()));
        }
    }
    samples.sort();
    samples.dedup_by_key(|s| s.0);
    let idx: Vec<u64> = samples.iter().map(|s| s.0).collect();
    let frames = read_frames(tc, &plan.output, &idx).unwrap();
    assert_eq!(frames.len(), idx.len());
    let lo = layout.after_cover(1920, 1080, 1080, 1920);
    for (img, (f, want, id)) in frames.iter().zip(&samples) {
        let got = decode_frame_id(img, &lo);
        assert_eq!(got, Some(*want), "timeline frame {f} (clip {id}) decoded {got:?}, expected source frame {want}");
    }
    assert_eq!(tl.total_frames, 30 + 30 + 30 - 6 + 30 + 30);

    // change only clip 3 → exactly one segment re-renders
    write_project(d.path(), 5.5);
    let l2 = pipeline::load(&pp).unwrap();
    let pr2 = pipeline::prepare(&l2, name, &e, false).unwrap();
    let (_, rep2) = pipeline::render(&l2, &pr2, &opts).unwrap();
    assert_eq!(rep2.cache_hits, 4, "{rep2:?}");
    assert_eq!(rep2.cache_misses, 1);
    assert_eq!(rep2.rendered_segments, vec!["c3".to_string()]);
}

/// Paths with spaces, commas, quotes, brackets and non-ASCII characters survive filter
/// escaping (`movie=` source reads the file through a filter option).
#[test]
fn filter_path_escaping_real_ffmpeg() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let weird = d.path().join("ü dir, it's [x];y");
    std::fs::create_dir_all(&weird).unwrap();
    let img = weird.join("pic 1.png");
    gen::still(tc, &img, 64, 64, "red").unwrap();
    let script = d.path().join("g.filter");
    std::fs::write(&script, format!("movie={}[m];[m]null[out]", cutline_render::escape::filter_path(&img))).unwrap();
    let out = d.path().join("o.png");
    let mut c = tc.ffmpeg_cmd();
    c.args(["-f", "lavfi", "-i", "color=c=black:s=64x64:d=0.1"]);
    c.args(tc.filter_complex_file_args(&script));
    c.args(["-map", "[out]", "-frames:v", "1"]).arg(&out);
    gen::run(c).unwrap();
    let px = image::open(&out).unwrap().to_rgb8();
    assert!(px.get_pixel(10, 10)[0] > 200);
}
