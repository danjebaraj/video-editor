//! Narration gaps: an asset's `gaps` (`{before: "Need more room", add: 0.7}`) insert silence
//! into the narration at word boundaries found by phrase in its transcript. This runs on the
//! raw project before cues are resolved: the padded narration is written to the project's
//! cache with a transcript shifted to match, and the asset is pointed at both, so cues,
//! captions, ducking and everything else see an ordinary, longer narration.

use cutline_core::diag::{ptr_token, Diagnostic};
use cutline_core::model::{Asset, Cue, NarrationGap};
use cutline_transcribe::text::{find_phrase, norm};
use cutline_transcribe::{Transcript, Word};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A gap as it was applied (reported by `cutline cues`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ResolvedGap {
    pub asset: String,
    /// `before` or `after`
    pub side: String,
    pub phrase: String,
    /// The transcript words the phrase matched.
    pub matched: String,
    /// Seconds of silence added.
    pub add: f64,
    /// Where the silence starts in the padded narration (seconds in the asset).
    pub at: f64,
    /// Where the narration was cut, in the original file (`None`: at its very end).
    pub source_at: Option<f64>,
}

/// A cut in the original narration with the silence to insert there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cut {
    /// Seconds in the original; `f64::INFINITY` is the end of the file.
    pub at: f64,
    pub add: f64,
}

/// Where to cut for a gap in front of word `first`: the middle of the silence between it and
/// the word before (0 when it is the first word).
pub fn cut_before(words: &[Word], first: usize) -> f64 {
    let w = &words[first];
    match words[..first].iter().rev().find(|p| !norm(&p.w).is_empty()) {
        Some(p) => ((p.end + w.start) / 2.0).max(0.0),
        None => 0.0,
    }
}

/// Where to cut for a gap behind word `last`: the middle of the silence between it and the
/// next word (the end of the file when it is the last word).
pub fn cut_after(words: &[Word], last: usize) -> f64 {
    let w = &words[last];
    match words[last + 1..].iter().find(|n| !norm(&n.w).is_empty()) {
        Some(n) => (w.end + n.start) / 2.0,
        None => f64::INFINITY,
    }
}

/// Cuts in time order, with cuts at the same place merged.
pub fn merge_cuts(mut cuts: Vec<Cut>) -> Vec<Cut> {
    cuts.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut out: Vec<Cut> = vec![];
    for c in cuts {
        match out.last_mut() {
            Some(l) if (l.at - c.at).abs() < 1e-4 || (l.at.is_infinite() && c.at.is_infinite()) => l.add += c.add,
            _ => out.push(c),
        }
    }
    out
}

/// Seconds a moment `t` of the original moves later in the padded narration.
pub fn shift_at(cuts: &[Cut], t: f64) -> f64 {
    cuts.iter().filter(|c| c.at <= t).map(|c| c.add).sum()
}

/// The transcript of the padded narration: every word moves by the silence inserted before it.
pub fn shift_transcript(t: &Transcript, cuts: &[Cut]) -> Transcript {
    let mut out = t.clone();
    for w in &mut out.words {
        let s = shift_at(cuts, (w.start + w.end) / 2.0);
        w.start += s;
        w.end += s;
    }
    out.rebuild_segments();
    out
}

/// The ffmpeg filter graph that plays input 0's audio with the cuts' silences inserted, ending
/// in `[out]`: the narration is split at each cut (`atrim`), each piece but the last is padded
/// with its silence (`apad`), and the pieces are joined (`concat`). Silence in front of the
/// first sample is an `adelay`, silence after the last an `apad`.
pub fn filter_graph(cuts: &[Cut]) -> String {
    let f = |v: f64| format!("{v:.6}");
    let lead: f64 = cuts.iter().filter(|c| c.at <= 1e-4).map(|c| c.add).sum();
    let tail: f64 = cuts.iter().filter(|c| c.at.is_infinite()).map(|c| c.add).sum();
    let mid: Vec<&Cut> = cuts.iter().filter(|c| c.at > 1e-4 && c.at.is_finite()).collect();
    let n = mid.len() + 1;
    let mut g = String::new();
    if n > 1 {
        g.push_str(&format!("[0:a]asplit={n}{};", (0..n).map(|k| format!("[p{k}]")).collect::<String>()));
    }
    for k in 0..n {
        let input = if n > 1 { format!("[p{k}]") } else { "[0:a]".to_string() };
        let mut chain = vec![];
        let (from, to) = (k.checked_sub(1).map(|p| mid[p].at), mid.get(k).map(|c| c.at));
        match (from, to) {
            (None, None) => {}
            (None, Some(e)) => chain.push(format!("atrim=end={}", f(e))),
            (Some(s), None) => chain.push(format!("atrim=start={}", f(s))),
            (Some(s), Some(e)) => chain.push(format!("atrim=start={}:end={}", f(s), f(e))),
        }
        chain.push("asetpts=PTS-STARTPTS".into());
        if k == 0 && lead > 0.0 {
            chain.push(format!("adelay={:.3}:all=1", lead * 1000.0));
        }
        let pad = mid.get(k).map(|c| c.add).unwrap_or(tail);
        if pad > 0.0 {
            chain.push(format!("apad=pad_dur={}", f(pad)));
        }
        let label = if n > 1 { format!("[s{k}]") } else { "[out]".to_string() };
        g.push_str(&format!("{input}{}{label};", chain.join(",")));
    }
    if n > 1 {
        g.push_str(&format!("{}concat=n={n}:v=0:a=1[out];", (0..n).map(|k| format!("[s{k}]")).collect::<String>()));
    }
    g.trim_end_matches(';').to_string()
}

/// Apply every asset's `gaps` in the raw project `v`: the asset's `path` and `transcript`
/// are replaced by the padded narration and its transcript (cached under
/// `.cutline/cache/gaps`, keyed by the narration file and the cuts) and `gaps` is removed.
/// Errors (`E_GAP_NOT_FOUND`, `E_GAP_TRANSCRIPT`, …) stop the load.
pub fn apply(v: &mut Value, dir: &Path) -> (Vec<Diagnostic>, Vec<ResolvedGap>) {
    let (mut d, mut done) = (vec![], vec![]);
    let ids: Vec<String> = v
        .get("assets")
        .and_then(|a| a.as_object())
        .map(|o| {
            o.iter()
                .filter(|(_, a)| a.get("gaps").and_then(|g| g.as_array()).is_some_and(|g| !g.is_empty()))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        return (d, done);
    }
    let cues: BTreeMap<String, Cue> =
        v.get("cues").and_then(|c| serde_json::from_value(c.clone()).ok()).unwrap_or_default();
    let lang = v.pointer("/captions/lang").and_then(|l| l.as_str()).map(String::from);
    for id in ids {
        let base = format!("/assets/{}", ptr_token(&id));
        // a malformed asset is reported when the project is typed
        let Some(asset) = v.pointer(&base).and_then(|a| serde_json::from_value::<Asset>(a.clone()).ok()) else {
            continue;
        };
        match one(dir, &id, &asset, &cues, lang.as_deref(), &base, &mut d) {
            Some((audio, transcript, gaps)) => {
                let a = v.pointer_mut(&base).and_then(|a| a.as_object_mut()).expect("asset object");
                a.insert("path".into(), Value::String(audio.to_string_lossy().to_string()));
                a.insert("transcript".into(), Value::String(transcript.to_string_lossy().to_string()));
                a.remove("gaps");
                done.extend(gaps);
            }
            None => continue,
        }
    }
    (d, done)
}

/// Resolve and apply one asset's gaps. Returns the padded audio, its transcript and the gaps,
/// or `None` after reporting why not.
fn one(
    dir: &Path,
    id: &str,
    asset: &Asset,
    cues: &BTreeMap<String, Cue>,
    lang: Option<&str>,
    base: &str,
    d: &mut Vec<Diagnostic>,
) -> Option<(PathBuf, PathBuf, Vec<ResolvedGap>)> {
    let errors_before = d.iter().filter(|x| x.is_error()).count();
    let media = dir.join(&asset.path);
    if !media.is_file() {
        return None; // E_ASSET_NOT_FOUND comes from validation
    }
    for (field, file) in [("transcript", &asset.transcript), ("script", &asset.script)] {
        if file.as_ref().is_some_and(|f| !dir.join(f).exists()) {
            d.push(Diagnostic::error(
                "E_ASSET_NOT_FOUND",
                format!("{base}/{field}"),
                format!("{field} file not found: {}", file.as_deref().unwrap_or_default()),
            ));
            return None;
        }
    }
    let plain = Asset { gaps: vec![], ..asset.clone() };
    let load = |a: &Asset| crate::auto::load_transcript(dir, id, Some(a), lang, true);
    // the script's wording finds the phrases; the heard words are what gets shifted
    let (worded, heard) = match (load(&plain), load(&Asset { script: None, ..plain.clone() })) {
        (Ok(Some(w)), Ok(Some(h))) => (w, h),
        (Err(e), _) | (_, Err(e)) => {
            d.push(Diagnostic::error("E_GAP_TRANSCRIPT", format!("{base}/gaps"), format!("{e:#}")));
            return None;
        }
        _ => {
            d.push(
                Diagnostic::error(
                    "E_GAP_TRANSCRIPT",
                    format!("{base}/gaps"),
                    format!("no transcript for `{id}`, so its gaps cannot be placed"),
                )
                .hint("set the asset's `transcript`, run `cutline transcribe` on the file, or install a whisper model"),
            );
            return None;
        }
    };
    let mut placed: Vec<(Cut, &NarrationGap, String, String)> = vec![];
    for (gi, g) in asset.gaps.iter().enumerate() {
        let gbase = format!("{base}/gaps/{gi}");
        let (side, written) = match (&g.before, &g.after) {
            (Some(p), None) => ("before", p),
            (None, Some(p)) => ("after", p),
            _ => {
                d.push(
                    Diagnostic::error("E_FIELD_CONFLICT", gbase, "a gap needs exactly one of `before` and `after`")
                        .hint("e.g. {before: \"Need more room\", add: 0.7} or {after: \"cue:room\", add: 0.3}"),
                );
                continue;
            }
        };
        if !(g.add > 0.0 && g.add <= 10.0) {
            d.push(Diagnostic::error(
                "E_BAD_VALUE",
                format!("{gbase}/add"),
                "add is the seconds of silence to insert: above 0, at most 10",
            ));
            continue;
        }
        // a phrase, or a cue's phrase
        let (phrase, occurrence) = match written.trim().strip_prefix("cue:") {
            Some(name) => match cues.get(name.trim()) {
                Some(c) if c.asset().is_some_and(|a| a != id) => {
                    d.push(Diagnostic::error(
                        "E_CUE_ASSET",
                        format!("{gbase}/{side}"),
                        format!("cue `{name}` is a phrase in `{}`, not in `{id}`", c.asset().unwrap_or_default()),
                    ));
                    continue;
                }
                Some(c) => (c.phrase().to_string(), g.occurrence.unwrap_or(c.occurrence())),
                None => {
                    d.push(
                        Diagnostic::error(
                            "E_UNKNOWN_CUE",
                            format!("{gbase}/{side}"),
                            format!("`{written}` names no cue"),
                        )
                        .hint(format!("defined cues: {}", cues.keys().cloned().collect::<Vec<_>>().join(", "))),
                    );
                    continue;
                }
            },
            None => (written.clone(), g.occurrence.unwrap_or(1)),
        };
        let m = match find_phrase(&worded.words, &phrase, occurrence) {
            Ok(m) => m,
            Err(nf) => {
                let msg = if nf.found > 0 {
                    format!(
                        "\"{phrase}\" occurs {} time(s) in `{id}`, so occurrence {occurrence} does not exist",
                        nf.found
                    )
                } else {
                    format!("\"{phrase}\" was not found in the `{id}` transcript, so the gap has no place")
                };
                let mut diag = Diagnostic::error("E_GAP_NOT_FOUND", format!("{gbase}/{side}"), msg);
                if !nf.nearest.is_empty() {
                    diag = diag.hint(format!("closest: {}", nf.nearest.join("; ")));
                }
                d.push(diag);
                continue;
            }
        };
        let matched = worded.words[m.first..=m.last].iter().map(|w| w.w.as_str()).collect::<Vec<_>>().join(" ");
        if !m.exact() {
            d.push(Diagnostic::warning(
                "W_GAP_FUZZY",
                format!("{gbase}/{side}"),
                format!("the gap's \"{phrase}\" matched \"{matched}\" (not word for word)"),
            ));
        }
        let at = if side == "before" { cut_before(&worded.words, m.first) } else { cut_after(&worded.words, m.last) };
        placed.push((Cut { at, add: g.add }, g, side.to_string(), matched));
    }
    if d.iter().filter(|x| x.is_error()).count() > errors_before || placed.is_empty() {
        return None;
    }
    let cuts = merge_cuts(placed.iter().map(|p| p.0).collect());
    let fingerprint = match cutline_media::fingerprint_any(&media) {
        Ok(f) => f,
        Err(e) => {
            d.push(Diagnostic::error(
                "E_GAP_FAILED",
                format!("{base}/gaps"),
                format!("cannot read {}: {e:#}", media.display()),
            ));
            return None;
        }
    };
    let cut_text: String = cuts.iter().map(|c| format!("{:.6}+{:.6};", c.at, c.add)).collect();
    let words_text = serde_json::to_string(&heard.words).unwrap_or_default();
    let key =
        cutline_media::hash_key(&[fingerprint.as_bytes(), cut_text.as_bytes(), words_text.as_bytes(), b"gaps-v1"]);
    let cache = crate::cutline_dir(dir).join("cache").join("gaps");
    let (audio, tfile) = (cache.join(format!("{key}.wav")), cache.join(format!("{key}.json")));
    if !(audio.is_file() && tfile.is_file()) {
        if let Err(e) = build(&media, &cuts, &heard, &audio, &tfile) {
            d.push(Diagnostic::error(
                "E_GAP_FAILED",
                format!("{base}/gaps"),
                format!("adding the gaps to {} failed: {e:#}", media.display()),
            ));
            return None;
        }
    }
    let gaps = placed
        .into_iter()
        .map(|(c, g, side, matched)| ResolvedGap {
            asset: id.to_string(),
            side,
            phrase: g.before.clone().or(g.after.clone()).unwrap_or_default(),
            matched,
            add: g.add,
            // silences inserted earlier push this one later
            at: if c.at.is_finite() {
                c.at + cuts.iter().filter(|x| x.at < c.at - 1e-4).map(|x| x.add).sum::<f64>()
            } else {
                heard.words.last().map(|w| w.end + shift_at(&cuts, w.end)).unwrap_or(0.0)
            },
            source_at: c.at.is_finite().then_some(c.at),
        })
        .collect();
    Some((audio, tfile, gaps))
}

/// Write the padded narration (24-bit WAV) and its transcript.
fn build(media: &Path, cuts: &[Cut], heard: &Transcript, audio: &Path, tfile: &Path) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let tc = cutline_deps::toolchain()?;
    std::fs::create_dir_all(audio.parent().context("cache dir")?)?;
    let tmp = audio.with_extension("tmp.wav");
    let mut c = tc.ffmpeg_cmd();
    c.arg("-i")
        .arg(media)
        .args(["-filter_complex", &filter_graph(cuts), "-map", "[out]", "-c:a", "pcm_s24le"])
        .arg(&tmp);
    let o = c.output().context("running ffmpeg")?;
    anyhow::ensure!(o.status.success(), "ffmpeg failed: {}", String::from_utf8_lossy(&o.stderr).trim());
    std::fs::write(tfile, serde_json::to_vec_pretty(&shift_transcript(heard, cuts))?)?;
    std::fs::rename(&tmp, audio)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(spec: &[(&str, f64, f64)]) -> Vec<Word> {
        spec.iter().map(|(w, s, e)| Word { w: w.to_string(), start: *s, end: *e, conf: 1.0, speaker: None }).collect()
    }

    #[test]
    fn cuts_sit_in_the_middle_of_the_pause() {
        let w = words(&[("Stay", 0.2, 0.5), ("organized.", 0.5, 1.1), ("Need", 1.5, 1.8), ("more", 1.8, 2.0)]);
        assert!((cut_before(&w, 2) - 1.3).abs() < 1e-9);
        assert!((cut_after(&w, 1) - 1.3).abs() < 1e-9);
        assert_eq!(cut_before(&w, 0), 0.0, "before the first word: silence at the very start");
        assert_eq!(cut_after(&w, 3), f64::INFINITY, "after the last word: silence at the end");
    }

    #[test]
    fn cuts_merge_and_shift_words() {
        let cuts = merge_cuts(vec![
            Cut { at: 1.3, add: 0.7 },
            Cut { at: 0.0, add: 0.2 },
            Cut { at: 1.3, add: 0.3 },
            Cut { at: f64::INFINITY, add: 1.0 },
        ]);
        assert_eq!(cuts.len(), 3);
        assert!((cuts[1].add - 1.0).abs() < 1e-9);
        let t = Transcript {
            words: words(&[("Stay", 0.2, 0.5), ("organized.", 0.5, 1.1), ("Need", 1.5, 1.8)]),
            ..Default::default()
        };
        let s = shift_transcript(&t, &cuts);
        assert!((s.words[0].start - 0.4).abs() < 1e-9 && (s.words[1].end - 1.3).abs() < 1e-9);
        assert!((s.words[2].start - 2.7).abs() < 1e-9, "{:?}", s.words[2]);
        assert_eq!(s.segments.len(), 2, "{:?}", s.segments);
    }

    #[test]
    fn filter_graph_splits_pads_and_joins() {
        let g = filter_graph(&[Cut { at: 1.3, add: 0.7 }, Cut { at: 4.0, add: 0.3 }]);
        assert_eq!(
            g,
            "[0:a]asplit=3[p0][p1][p2];\
             [p0]atrim=end=1.300000,asetpts=PTS-STARTPTS,apad=pad_dur=0.700000[s0];\
             [p1]atrim=start=1.300000:end=4.000000,asetpts=PTS-STARTPTS,apad=pad_dur=0.300000[s1];\
             [p2]atrim=start=4.000000,asetpts=PTS-STARTPTS[s2];\
             [s0][s1][s2]concat=n=3:v=0:a=1[out]"
        );
        // only at the ends: no split
        let g = filter_graph(&[Cut { at: 0.0, add: 0.5 }, Cut { at: f64::INFINITY, add: 1.0 }]);
        assert_eq!(g, "[0:a]asetpts=PTS-STARTPTS,adelay=500.000:all=1,apad=pad_dur=1.000000[out]");
    }
}
