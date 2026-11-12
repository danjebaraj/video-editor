//! Golden snapshots of generated filter scripts / commands for fixture projects.

use cutline_core::model::AssetKind;
use cutline_core::time::Fps;
use cutline_core::timeline::{resolve, Context, MediaFacts};
use cutline_deps::tools::{ToolSource, Toolchain};
use cutline_media::assets::{AssetMedia, ProjectMedia};
use cutline_render::{build_plan, dry_run_text, PlanExtras, RenderOptions};
use std::path::{Path, PathBuf};

fn tc() -> Toolchain {
    Toolchain { ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), version: "7.1".into(), source: ToolSource::Path }
}

fn media(items: &[(&str, &str, AssetKind, f64, u32, u32, bool)]) -> ProjectMedia {
    let mut m = ProjectMedia::default();
    for (id, path, kind, dur, w, h, audio) in items {
        m.assets.insert(
            id.to_string(),
            AssetMedia {
                id: id.to_string(),
                path: PathBuf::from("/proj").join(path),
                fingerprint: format!("fp-{id}"),
                info: None,
                facts: MediaFacts {
                    exists: true,
                    kind: *kind,
                    duration: *dur,
                    has_video: *kind != AssetKind::Audio,
                    has_audio: *audio,
                    alpha: false,
                    width: *w,
                    height: *h,
                    fps: Some(Fps::new(30, 1)),
                    rotation: 0,
                },
            },
        );
    }
    m
}

fn snapshot_for(project: &str, m: &ProjectMedia, draft: bool) -> String {
    let p = cutline_core::parse::parse_project(project).unwrap();
    let (tl, d) = resolve(&p, m, &Context::default());
    assert!(!cutline_core::diag::has_errors(&d), "{d:?}");
    let tl = tl.unwrap();
    let opts = RenderOptions { draft, ..Default::default() };
    let extras = PlanExtras { pad_color: "0x101014".into(), accent: "0xFFC400".into(), ..Default::default() };
    let tc = tc();
    let plan = build_plan(&tc, &p, Path::new("/proj/p.json5"), m, &tl, &opts, None, &extras).unwrap();
    let txt = dry_run_text(&tc, &plan);
    txt.replace(&plan.work_dir.to_string_lossy().to_string(), "<work>")
}

#[test]
fn shorts_three_clips_fade() {
    let m = media(&[
        ("a", "a.mp4", AssetKind::Video, 10.0, 1920, 1080, true),
        ("b", "b.mov", AssetKind::Video, 8.0, 1080, 1920, false),
    ]);
    let s = snapshot_for(
        r#"{version: 1, name: "snap", target: "shorts", assets: {a: {path: "a.mp4"}, b: {path: "b.mov"}},
            main: [{id: "hook", clip: "a", in: 1, out: 3},
                   {transition: {type: "fade", duration: 0.5}},
                   {id: "mid", clip: "b", in: 0, out: 2, speed: 2},
                   {id: "end", clip: "a", in: "00:05.000", out: 7, reframe: {mode: "fit_blur"}}]}"#,
        &m,
        false,
    );
    insta::assert_snapshot!(s);
}

#[test]
fn long_form_fit_pad_and_still() {
    let m = media(&[
        ("v", "talk.mp4", AssetKind::Video, 60.0, 1080, 1920, true),
        ("img", "pic.png", AssetKind::Image, 0.04, 800, 800, false),
    ]);
    let s = snapshot_for(
        r#"{version: 1, name: "long", target: "long_1080p", assets: {v: {path: "talk.mp4"}, img: {path: "pic.png"}},
            main: [{id: "intro", clip: "img", duration: 2, reframe: {mode: "fit_pad"}},
                   {transition: {type: "wipeleft", duration: 0.3, audio: "cut"}},
                   {id: "talk", clip: "v", in: 10, out: 20, gain_db: -3}]}"#,
        &m,
        false,
    );
    insta::assert_snapshot!(s);
}

#[test]
fn draft_geometry() {
    let m = media(&[("a", "a.mp4", AssetKind::Video, 10.0, 1920, 1080, true)]);
    let s = snapshot_for(
        r#"{version: 1, name: "d", assets: {a: {path: "a.mp4"}}, main: [{id: "x", clip: "a", out: 1}]}"#,
        &m,
        true,
    );
    insta::assert_snapshot!(s);
}
