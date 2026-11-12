//! WER and timing on synthesized speech. Needs a TTS engine (macOS `say`, Windows SAPI or
//! espeak-ng) and whisper.cpp + model; skips with a message otherwise.

use cutline_transcribe::{model::wer, whisper};

const SENTENCE: &str = "The quick brown fox jumps over the lazy dog while the chef sharpens a knife.";

#[test]
fn whisper_wer_and_timing_on_synthesized_speech() {
    let Some(tc) = cutline_testkit::test_toolchain() else { return };
    let model = std::env::var("CUTLINE_WHISPER_MODEL").unwrap_or_else(|_| whisper::FAST_MODEL.into());
    if let Err(e) = whisper::available(&model) {
        eprintln!("SKIP: {e:#}");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let wav = d.path().join("speech.wav");
    if !whisper::synthesize(SENTENCE, &wav, tc).unwrap() {
        eprintln!("SKIP: no TTS engine (say / SAPI / espeak-ng)");
        return;
    }
    let t = whisper::transcribe(tc, &wav, &model, Some("en"), &d.path().join("w")).unwrap();
    let w = wer(SENTENCE, &t.text());
    assert!(w <= 0.15, "WER {w:.3}: {}", t.text());
    // monotone, non-overlapping word timings covering the audio
    assert!(
        t.words.windows(2).all(|p| p[0].start <= p[1].start && p[0].end <= p[1].start + 0.05),
        "non-monotone timings"
    );
    let dur = cutline_media::probe(tc, &wav, None).unwrap().duration;
    let first = t.words.first().unwrap().start;
    let last = t.words.last().unwrap().end;
    assert!(first < 1.0 && last > dur * 0.7 && last <= dur + 0.2, "coverage {first:.2}–{last:.2} of {dur:.2}");
}
