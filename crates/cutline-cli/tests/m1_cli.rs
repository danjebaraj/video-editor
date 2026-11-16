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
    // several stills from one run: a comma list and a repeated --at (the same time once)
    let (code, v, err) =
        run_json(bin().arg("preview").arg(&pf).args(["--at", "0.5,1.25,00:02.000", "--at", "2.5", "--at", "0.5"]));
    assert_eq!(code, 0, "{err}");
    let names: Vec<String> = v["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| std::path::Path::new(f.as_str().unwrap()).file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, ["at_00-00.500.png", "at_00-01.250.png", "at_00-02.000.png", "at_00-02.500.png"]);
    // a bad time is a usage error
    let (code, _, err) = run_json(bin().arg("preview").arg(&pf).args(["--at", "0.5,soon"]));
    assert_eq!(code, 2, "{err}");

    // review packs go to review/<project name>/, so two projects in a folder keep their own
    let teaser = proj.join("teaser.json5");
    std::fs::write(&teaser, std::fs::read_to_string(&pf).unwrap().replace("name: \"proj\"", "name: \"proj teaser\""))
        .unwrap();
    let (code, v, err) = run_json(bin().arg("review").arg(&pf).args(["--ocr", "off"]));
    assert_eq!(code, 0, "{err} {v}");
    assert_eq!(
        std::path::Path::new(v["dir"].as_str().unwrap()),
        proj.canonicalize().unwrap().join("review").join("proj")
    );
    let (code, v, err) = run_json(bin().arg("review").arg(&teaser).args(["--ocr", "off"]));
    assert_eq!(code, 0, "{err} {v}");
    assert!(std::path::Path::new(v["dir"].as_str().unwrap()).ends_with("review/proj teaser"), "{v}");
    for sub in ["proj", "proj teaser"] {
        let dir = proj.join("review").join(sub);
        assert!(dir.join("summary.md").exists() && dir.join("contact_sheet.jpg").exists(), "{sub}");
        assert!(dir.join("cuts").is_dir() && dir.join("annotations").is_dir() && dir.join("reframe").is_dir());
    }
    assert!(!proj.join("review").join("summary.md").exists());
    // --out still names the folder itself, from where the command is run (as `preview --out` does)
    let (code, v, err) =
        run_json(bin().current_dir(&proj).arg("review").arg(&pf).args(["--ocr", "off", "--out", "review-x"]));
    assert_eq!(code, 0, "{err} {v}");
    assert!(proj.join("review-x").join("summary.md").exists());
}

#[test]
fn deps_path_prints_tool_paths() {
    let Some(tc) = tc() else { return };
    for (what, want) in [("ffmpeg", &tc.ffmpeg), ("ffprobe", &tc.ffprobe)] {
        let out = bin().args(["deps", "path", what]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        // the bare path, ready for `$(cutline deps path ffmpeg)`
        let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(std::path::Path::new(&printed).is_file(), "{printed}");
        assert_eq!(
            std::fs::canonicalize(&printed).unwrap(),
            std::fs::canonicalize(want).unwrap(),
            "the same binary cutline runs"
        );
        let (code, v, _) = run_json(bin().args(["deps", "path", what]));
        assert_eq!(code, 0);
        assert_eq!(v["path"].as_str().unwrap(), printed);
    }
    let (code, v, _) = run_json(bin().args(["deps", "path", "tools"]));
    assert_eq!(code, 0);
    assert!(v["path"].as_str().unwrap().ends_with("tools"), "{v}");
    let (code, _, _) = run_json(bin().args(["deps", "path", "sox"]));
    assert_eq!(code, 2);
}

#[test]
fn schema_command_prints_schema() {
    let (code, v, _) = run_json(bin().arg("schema"));
    assert_eq!(code, 0);
    assert_eq!(v["title"], "Project");
}
