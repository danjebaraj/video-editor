mod common;
use common::*;

fn tc() -> Option<&'static cutline_deps::Toolchain> {
    cutline_testkit::test_toolchain()
}

#[test]
fn init_validate_dryrun_preview() {
    let Some(tc) = tc() else { return };
    let d = tempfile::tempdir().unwrap();
    let foot = d.path().join("in");
    std::fs::create_dir_all(&foot).unwrap();
    let mut s = cutline_testkit::gen::FrameIdSource::new(1280, 720, 30, 3.0);
    s.tone_hz = Some(440.0);
    cutline_testkit::gen::frameid_video(tc, &foot.join("take1.mp4"), &s).unwrap();
    let proj = d.path().join("proj");
    let (code, v, err) = run_json(bin().args(["init"]).arg(&proj).arg("--assets").arg(&foot));
    assert_eq!(code, 0, "{err} {v}");
    let pf = proj.join("project.json5");
    assert!(pf.exists());
    let (code, v, _) = run_json(bin().arg("validate").arg(&pf));
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["ok"], true);

    // invalid edit → exit 1 with code + JSON pointer
    let text = std::fs::read_to_string(&pf)
        .unwrap()
        .replace("{ id: \"clip1\", clip: \"a1\" }", "{ id: \"clip1\", clip: \"a1\", out: 99 }");
    let bad = proj.join("bad.json5");
    std::fs::write(&bad, text).unwrap();
    let (code, v, _) = run_json(bin().arg("validate").arg(&bad));
    assert_eq!(code, 1);
    assert_eq!(v["errors"][0]["code"], "E_TIME_OUT_OF_RANGE");
    assert_eq!(v["errors"][0]["path"], "/main/0/out");

    // syntax error is a validation error too
    std::fs::write(&bad, "{ version: 1, main: [").unwrap();
    let (code, v, _) = run_json(bin().arg("validate").arg(&bad));
    assert_eq!(code, 1);
    assert_eq!(v["errors"][0]["code"], "E_PARSE");

    // dry run prints commands and filter scripts, renders nothing
    let out = bin().args(["render", "--dry-run"]).arg(&pf).output().unwrap();
    assert!(out.status.success());
    let txt = String::from_utf8_lossy(&out.stdout);
    assert!(txt.contains("prores_ks") && txt.contains("final.filter") && txt.contains("libx264"), "{txt}");
    assert!(!proj.join("out").join("proj.mp4").exists());

    // preview --at produces PNGs
    let (code, v, err) = run_json(bin().arg("preview").arg(&pf).args(["--at", "0.5", "00:02.000", "--safe-zones"]));
    assert_eq!(code, 0, "{err}");
    let frames = v["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 2);
    for f in frames {
        let p = std::path::PathBuf::from(f.as_str().unwrap());
        let img = image::open(&p).unwrap();
        assert_eq!((img.width(), img.height()), (540, 960));
    }
}

#[test]
fn schema_command_prints_schema() {
    let (code, v, _) = run_json(bin().arg("schema"));
    assert_eq!(code, 0);
    assert_eq!(v["title"], "Project");
}
