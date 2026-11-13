//! `.tape` assets are recorded with vhs at load time and cached (skipped when vhs is missing).

use cutline_render::pipeline::{self, LoadError};
use cutline_testkit::test_toolchain;

const TAPE: &str = "Output ignored.gif\nSet Shell bash\nSet Width 640\nSet Height 360\nSet FontSize 18\nType \"echo hello from cutline\"\nEnter\nSleep 1.5s\n";

fn diags(r: Result<pipeline::Loaded, LoadError>) -> Vec<(String, String)> {
    match r {
        Ok(l) => l.diags.into_iter().map(|d| (d.code, d.path)).collect(),
        Err(LoadError::Invalid(d)) => d.into_iter().map(|d| (d.code, d.path)).collect(),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn tapes_record_once_and_report_failures() {
    let Some(_) = test_toolchain() else { return };
    let Some(vhs) = cutline_render::terminal::vhs_binary() else {
        eprintln!("SKIP: vhs not found (set CUTLINE_VHS)");
        return;
    };
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("demo.tape"), TAPE).unwrap();
    let pp = d.path().join("p.json5");
    std::fs::write(
        &pp,
        "{version: 1, target: 'custom', width: 640, height: 360, fps: 30,
          assets: {term: {path: 'demo.tape'}}, main: [{id: 't', clip: 'term'}], audio: {voice_chain: 'raw'}}",
    )
    .unwrap();
    let l = pipeline::load(&pp).unwrap_or_else(|e| panic!("{e}"));
    let m = l.media.get("term").expect("probed");
    assert!(m.path.extension().is_some_and(|e| e == "mp4"), "{}", m.path.display());
    assert_eq!((m.facts.width, m.facts.height), (640, 360));
    assert!(m.facts.has_video && m.facts.duration > 1.0, "{:?}", m.facts);
    assert!(l.timeline.is_some() && !cutline_core::diag::has_errors(&l.diags), "{:?}", l.diags);
    // the second load reuses the recording
    let modified = std::fs::metadata(&m.path).unwrap().modified().unwrap();
    let l2 = pipeline::load(&pp).unwrap();
    let m2 = l2.media.get("term").unwrap();
    assert_eq!(m2.path, m.path);
    assert_eq!(std::fs::metadata(&m2.path).unwrap().modified().unwrap(), modified);

    // `cutline record` re-records on demand (fresh live data), replacing the cached file
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let again = cutline_render::terminal::rerecord(&pp, Some("term")).unwrap();
    assert_eq!(again, vec![("term".to_string(), m.path.clone())]);
    assert!(std::fs::metadata(&m.path).unwrap().modified().unwrap() > modified, "recorded again");
    assert!(cutline_render::terminal::rerecord(&pp, Some("nope")).is_err());

    // a broken tape is reported on the asset
    std::fs::write(d.path().join("demo.tape"), "Set Shell bash\nFrobnicate 3\n").unwrap();
    let ds = diags(pipeline::load(&pp));
    assert!(ds.contains(&("E_TAPE_FAILED".into(), "/assets/term/path".into())), "{ds:?}");

    // without vhs the asset says how to get it
    std::env::set_var("CUTLINE_VHS", d.path().join("no-such-vhs"));
    let ds = diags(pipeline::load(&pp));
    std::env::set_var("CUTLINE_VHS", &vhs);
    assert!(ds.contains(&("E_TOOL_MISSING".into(), "/assets/term/path".into())), "{ds:?}");
}
