//! Whisper's work folder and the transcript cache stay inside the project: in Codex's sandbox a
//! run may write only its own folder, not the system temp folder or cutline's shared cache. One
//! test in its own binary, because it points `TMPDIR` at a regular file for the whole process.
//! Needs a TTS engine and whisper.cpp + model; skips with a message otherwise.

use cutline_media::cache::Cache;
use cutline_transcribe::{whisper, Source};

#[test]
fn whisper_runs_and_caches_in_the_project_when_nothing_else_is_writable() {
    let Some(tc) = cutline_testkit::test_toolchain() else { return };
    let model = whisper::FAST_MODEL;
    if let Err(e) = whisper::available(model) {
        eprintln!("SKIP: {e:#}");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let proj = d.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let wav = proj.join("vo.wav");
    if !whisper::synthesize("Sharpen the knife before you start.", &wav, tc).unwrap() {
        eprintln!("SKIP: no TTS engine (say / SAPI / espeak-ng)");
        return;
    }
    let fp = cutline_media::fingerprint(&wav).unwrap();

    let blocked = d.path().join("is-a-file");
    std::fs::write(&blocked, b"").unwrap();
    std::env::set_var("TMPDIR", &blocked);
    std::env::set_var("TMP", &blocked);
    std::env::set_var("TEMP", &blocked);
    // the shared cache can't be written either
    let cache = Cache { root: blocked.join("transcripts"), ..Cache::shared("transcripts", &proj) };
    let work = proj.join(".cutline").join("tmp");

    let (t, src) = cutline_transcribe::get(tc, "vo", &wav, &fp, None, model, Some("en"), &cache, &work, true)
        .unwrap()
        .expect("a transcript");
    assert!(matches!(src, Source::Whisper(_)), "{src:?}");
    assert!(t.text().to_lowercase().contains("knife"), "{}", t.text());
    let left: Vec<_> = std::fs::read_dir(&work).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    assert!(left.is_empty(), "work folder left behind: {left:?}");
    // kept in the project, so the next run finds it
    let (_, src) = cutline_transcribe::get(tc, "vo", &wav, &fp, None, model, Some("en"), &cache, &work, false)
        .unwrap()
        .expect("cached");
    assert_eq!(src, Source::Cache);
}
