//! QA writes its scratch frames in the project, never in the system temp folder: in Codex's
//! sandbox a run may write only its own folder. One test in its own binary, because it points
//! `TMPDIR` at a regular file (nothing can be created under it) for the whole process.

use cutline_qa::qa::QaContext;
use cutline_qa::review::{grab, grab_held, grab_source};
use cutline_testkit::{gen, test_toolchain};

#[test]
fn frame_grabs_work_without_a_writable_temp_folder_and_leave_nothing_behind() {
    let Some(tc) = test_toolchain() else { return };
    let d = tempfile::tempdir().unwrap();
    let proj = d.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    // a sparse capture (red, then blue from 2 s; frames stored at 0, 0.5 and 2 s), so the
    // held-frame path, which writes a file, is the one that answers
    let rec = proj.join("sparse.mov");
    let mut c = tc.ffmpeg_cmd();
    c.args([
        "-f",
        "lavfi",
        "-i",
        "color=c=red:s=160x90:r=30:d=4[a];color=c=blue:s=160x90:r=30:d=4[b];[a][b]overlay=enable='gte(t,2)'",
    ])
    .args(["-vf", "select='eq(n,0)+eq(n,15)+eq(n,60)'", "-fps_mode", "passthrough", "-c:v", "libx264", "-bf", "0"])
    .args(["-pix_fmt", "yuv420p", "-video_track_timescale", "600"])
    .arg(&rec);
    gen::run(c).unwrap();

    let blocked = d.path().join("tmp-is-a-file");
    std::fs::write(&blocked, b"").unwrap();
    std::env::set_var("TMPDIR", &blocked);
    std::env::set_var("TMP", &blocked);
    std::env::set_var("TEMP", &blocked);
    if cfg!(unix) {
        assert!(tempfile::tempdir().is_err(), "the system temp folder should be unusable here");
    }

    let work = cutline_render::cutline_dir(&proj).join("tmp");
    let px = |img: image::RgbImage| img.get_pixel(80, 45).0;
    assert!(px(grab_held(tc, &rec, 1.5, &work).unwrap())[0] > 200, "red is on screen at 1.5 s");
    assert!(px(grab_source(tc, &rec, 3.0, true, &work).unwrap())[2] > 200, "blue is on screen at 3 s");
    assert!(grab(tc, &rec, 1.0, &work).is_ok());
    // the scratch folders are gone
    let left: Vec<_> = std::fs::read_dir(&work).unwrap().flatten().map(|e| e.path()).collect();
    assert!(left.is_empty(), "{left:?}");

    // QA without a project folder writes beside the checked file
    let ctx = QaContext::default();
    assert_eq!(ctx.work_dir_for(&proj.join("out/v.mp4")), proj.join("out/.cutline/tmp"));
    let ctx = QaContext { work_dir: Some(work.clone()), ..Default::default() };
    assert_eq!(ctx.work_dir_for(&proj.join("out/v.mp4")), work);
}
