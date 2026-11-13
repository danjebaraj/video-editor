//! Audio stage: role stems (dialogue/voice/music/sfx) → per-role chains → ducking → mix →
//! two-pass loudness → final WAV.

use crate::escape::f;
use crate::job::{base_args, Job};
use crate::segment::{samples_at, SAMPLE_RATE};
use crate::{assemble, Geometry, PlanExtras, RenderOptions};
use anyhow::{Context, Result};
use cutline_audio::{chains, envelope, loudness, vad, wav};
use cutline_core::model::{Duck, Layer, Project, Role};
use cutline_core::timeline::Timeline;
use cutline_deps::Toolchain;
use cutline_media::assets::ProjectMedia;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Envelope sample rate (Hz); upsampled by ffmpeg before multiplying.
pub const ENV_RATE: u32 = 1000;

#[derive(Debug, Clone, Serialize)]
pub struct DuckPlan {
    pub params: Duck,
    pub sidechain: bool,
    pub under: Vec<Role>,
    pub target: Vec<Role>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioPlan {
    /// Renders one WAV per present role.
    pub stems_job: Job,
    pub stems: BTreeMap<String, PathBuf>,
    /// Speech activity from transcripts (timeline seconds). Empty → VAD on `under` stems.
    pub transcript_speech: Vec<(f64, f64)>,
    pub duck: Option<DuckPlan>,
    pub env_wav: PathBuf,
    pub mix_job: Job,
    pub mix_wav: PathBuf,
    pub final_wav: PathBuf,
    /// (integrated LUFS, true peak dBTP, LRA)
    pub loudness: Option<(f64, f64, f64)>,
    pub duration: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LoudnessResult {
    pub input_i: f64,
    pub input_tp: f64,
    pub output_i: f64,
    pub output_tp: f64,
    pub output_lra: f64,
    pub correction_db: f64,
    pub speech_spans: usize,
    pub speech_source: String,
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Dialogue => "dialogue",
        Role::Voice => "voice",
        Role::Music => "music",
        Role::Sfx => "sfx",
    }
}

#[allow(clippy::too_many_arguments)]
pub fn plan(
    tc: &Toolchain,
    p: &Project,
    tl: &Timeline,
    g: &Geometry,
    mezz: &[PathBuf],
    media: &ProjectMedia,
    work: &Path,
    _dir: &Path,
    extras: &PlanExtras,
    opts: &RenderOptions,
) -> Result<AudioPlan> {
    let total = samples_at(tl.total_frames, g);
    let dur = total as f64 / SAMPLE_RATE as f64;
    let mut j = Job::new("audio stems");
    j.args(base_args(false));
    for m in mezz {
        j.args(["-i".to_string(), m.to_string_lossy().to_string()]);
    }
    let mut next = mezz.len();
    let mut lines: Vec<String> = vec![];
    let mut per_role: BTreeMap<Role, Vec<String>> = BTreeMap::new();

    // dialogue
    let any_clip_audio = tl.clips.iter().any(|c| media.get(&c.asset).map(|m| m.facts.has_audio).unwrap_or(false));
    if any_clip_audio {
        let (dl, out) = assemble::join_dialogue(tl, g, 0);
        lines.extend(dl);
        per_role.entry(Role::Dialogue).or_default().push(format!("[{out}]"));
    }

    // layer audio (audio/sfx layers, video/b-roll with audio, clip audio replacement, transition sfx)
    struct Src {
        path: PathBuf,
        role: Role,
        start_s: i64,
        len_s: i64,
        src_in: f64,
        gain: f64,
        fade_in: f64,
        fade_out: f64,
        looped: bool,
        file_dur: f64,
    }
    let mut srcs: Vec<Src> = vec![];
    let resolve_sfx = |name: &str| -> Option<(PathBuf, f64)> {
        if let Some(m) = media.get(name) {
            return Some((m.path.clone(), m.facts.duration));
        }
        extras.sfx_files.get(name).map(|p| (p.clone(), 0.0))
    };
    for l in &tl.layers {
        let start = samples_at(l.start, g);
        let len = samples_at(l.end, g) - start;
        match &l.layer {
            Layer::Audio { asset, src_in, gain_db, fade_in, fade_out, r#loop, role, .. } => {
                let Some(m) = media.get(asset) else { continue };
                let r = role.or(p.assets.get(asset).and_then(|a| a.role)).unwrap_or(Role::Music);
                srcs.push(Src {
                    path: m.path.clone(),
                    role: r,
                    start_s: start,
                    len_s: len,
                    src_in: src_in.map(|t| t.seconds(tl.fps, m.facts.duration)).unwrap_or(0.0),
                    gain: gain_db.unwrap_or(0.0),
                    fade_in: fade_in.unwrap_or(0.0),
                    fade_out: fade_out.unwrap_or(0.0),
                    looped: r#loop.unwrap_or(false),
                    file_dur: m.facts.duration,
                });
            }
            Layer::Sfx { asset, gain_db, pick_from, .. } => {
                let name = if pick_from.is_empty() {
                    asset.clone()
                } else {
                    // deterministic pick: hash of layer index
                    pick_from[l.index % pick_from.len()].clone()
                };
                let Some((path, d)) = resolve_sfx(&name) else {
                    tracing::warn!("sfx `{name}` has no file; skipped");
                    continue;
                };
                let r = p.assets.get(&name).and_then(|a| a.role).unwrap_or(Role::Sfx);
                srcs.push(Src {
                    path,
                    role: r,
                    start_s: start,
                    len_s: len.max(1),
                    src_in: 0.0,
                    gain: gain_db.unwrap_or(0.0),
                    fade_in: 0.0,
                    fade_out: 0.0,
                    looped: false,
                    file_dur: d,
                });
            }
            Layer::Video { asset, audio: Some(true), gain_db, src_in, .. }
            | Layer::Broll { asset, audio: Some(true), gain_db, src_in, .. } => {
                let Some(m) = media.get(asset) else { continue };
                if !m.facts.has_audio {
                    continue;
                }
                srcs.push(Src {
                    path: m.path.clone(),
                    role: Role::Dialogue,
                    start_s: start,
                    len_s: len,
                    src_in: src_in.map(|t| t.seconds(tl.fps, m.facts.duration)).unwrap_or(0.0),
                    gain: gain_db.unwrap_or(0.0),
                    fade_in: 0.01,
                    fade_out: 0.01,
                    looped: false,
                    file_dur: m.facts.duration,
                });
            }
            _ => {}
        }
    }
    for c in &tl.clips {
        if let Some(a) = &c.clip.audio {
            if !["keep", "mute", "duck"].contains(&a.as_str()) {
                if let Some(m) = media.get(a) {
                    let r = p.assets.get(a).and_then(|x| x.role).unwrap_or(Role::Voice);
                    let start = samples_at(c.start, g);
                    srcs.push(Src {
                        path: m.path.clone(),
                        role: r,
                        start_s: start,
                        len_s: samples_at(c.end(), g) - start,
                        src_in: 0.0,
                        gain: c.clip.gain_db.unwrap_or(0.0),
                        fade_in: 0.01,
                        fade_out: 0.01,
                        looped: false,
                        file_dur: m.facts.duration,
                    });
                }
            }
        }
    }
    for t in &tl.transitions {
        if let Some(s) = &t.transition.sfx {
            let Some((path, d)) = resolve_sfx(s) else {
                tracing::warn!("transition sfx `{s}` has no file; skipped");
                continue;
            };
            // centered on the transition midpoint (or the cut)
            let mid = samples_at(t.start, g) + (samples_at(t.start + t.frames, g) - samples_at(t.start, g)) / 2;
            let len = ((d.max(0.05)) * SAMPLE_RATE as f64) as i64;
            let start = (mid - len / 2).max(0);
            srcs.push(Src {
                path,
                role: Role::Sfx,
                start_s: start,
                len_s: len.min(total - start).max(1),
                src_in: 0.0,
                gain: t.transition.sfx_gain_db.unwrap_or(-3.0),
                fade_in: 0.0,
                fade_out: 0.0,
                looped: false,
                file_dur: d,
            });
        }
    }
    for (k, s) in srcs.iter().enumerate() {
        j.args(["-i".to_string(), s.path.to_string_lossy().to_string()]);
        let idx = next;
        next += 1;
        let mut ch: Vec<String> = vec![];
        ch.push(format!("aresample={SAMPLE_RATE}"));
        ch.push("aformat=sample_fmts=flt:channel_layouts=stereo".into());
        if s.looped && s.file_dur > 0.0 {
            ch.push(format!("aloop=loop=-1:size={}", (s.file_dur * SAMPLE_RATE as f64).round() as i64));
        }
        let start_sample = (s.src_in * SAMPLE_RATE as f64).round() as i64;
        ch.push(format!("atrim=start_sample={start_sample}:end_sample={}", start_sample + s.len_s));
        ch.push("asetpts=N/SR/TB".into());
        let secs = s.len_s as f64 / SAMPLE_RATE as f64;
        if s.gain.abs() > 1e-9 {
            ch.push(format!("volume={}dB", f(s.gain)));
        }
        if s.fade_in > 0.0 {
            ch.push(format!("afade=t=in:d={}", f(s.fade_in.min(secs / 2.0))));
        }
        if s.fade_out > 0.0 {
            let fo = s.fade_out.min(secs / 2.0);
            ch.push(format!("afade=t=out:st={}:d={}", f((secs - fo).max(0.0)), f(fo)));
        }
        if s.start_s > 0 {
            ch.push(format!("adelay=delays={}S:all=1", s.start_s));
        }
        ch.push(format!("apad=whole_len={total}"));
        ch.push(format!("atrim=end_sample={total}"));
        lines.push(format!("[{idx}:a]{}[src{k}]", ch.join(",")));
        per_role.entry(s.role).or_default().push(format!("[src{k}]"));
    }

    // per-role sum + chain
    let voice_chain = p.audio.as_ref().and_then(|a| a.voice_chain.clone()).unwrap_or_else(|| "clean_voice".into());
    let dialogue_chain = p.audio.as_ref().and_then(|a| a.dialogue_chain.clone()).unwrap_or_else(|| voice_chain.clone());
    let mut stems = BTreeMap::new();
    let mut maps: Vec<(String, PathBuf)> = vec![];
    for (role, ins) in &per_role {
        let name = role_name(*role);
        let summed = if ins.len() == 1 {
            ins[0].clone()
        } else {
            lines.push(format!("{}amix=inputs={}:normalize=0:duration=longest[{name}_sum]", ins.join(""), ins.len()));
            format!("[{name}_sum]")
        };
        let chain = match role {
            Role::Voice => chains::voice_chain(&voice_chain, None).unwrap_or_default(),
            Role::Dialogue => chains::voice_chain(&dialogue_chain, None).unwrap_or_default(),
            Role::Music => chains::music_chain(),
            Role::Sfx => String::new(),
        };
        let chain = if chain.is_empty() { "anull".to_string() } else { chain };
        lines.push(format!(
            "{summed}{chain},atrim=end_sample={total},aformat=sample_fmts=flt:channel_layouts=stereo[stem_{name}]"
        ));
        let path = work.join(format!("stem_{name}.wav"));
        maps.push((format!("[stem_{name}]"), path.clone()));
        stems.insert(name.to_string(), path);
    }
    if maps.is_empty() {
        lines.push(format!("anullsrc=r={SAMPLE_RATE}:cl=stereo,atrim=end_sample={total},aformat=sample_fmts=flt:channel_layouts=stereo[stem_silence]"));
        let path = work.join("stem_silence.wav");
        maps.push(("[stem_silence]".into(), path.clone()));
        stems.insert("silence".into(), path);
    }
    let script = work.join("audio_stems.filter");
    j.args(tc.filter_complex_file_args(&script));
    for (label, path) in &maps {
        j.args(["-map".to_string(), label.clone(), "-c:a".into(), "pcm_f32le".into(), "-ar".into(), "48000".into()]);
        j.arg(path.to_string_lossy().to_string());
        j.outputs.push(path.clone());
    }
    j.scripts.push((script, lines.join(";\n") + "\n"));

    // ducking
    let a = p.audio.clone().unwrap_or_default();
    let has_music = stems.contains_key("music");
    let duck = if has_music || a.duck.is_some() {
        let d = a.duck.clone().unwrap_or_default();
        let under: Vec<Role> = d.under.iter().copied().filter(|r| stems.contains_key(role_name(*r))).collect();
        let target: Vec<Role> = d.target.iter().copied().filter(|r| stems.contains_key(role_name(*r))).collect();
        (!under.is_empty() && !target.is_empty()).then(|| DuckPlan {
            sidechain: d.mode.as_deref() == Some("sidechain"),
            params: d,
            under,
            target,
        })
    } else {
        None
    };
    let env_wav = work.join("duck_env.wav");
    let mix_wav = work.join("mix.wav");
    let mut mj = Job::new("audio mix");
    mj.args(base_args(false));
    let names: Vec<String> = stems.keys().cloned().collect();
    for n in &names {
        mj.args(["-i".to_string(), stems[n].to_string_lossy().to_string()]);
    }
    let mut ml: Vec<String> = vec![];
    let mut mixed: Vec<String> = vec![];
    let env_idx = names.len();
    if let Some(dp) = &duck {
        if dp.sidechain {
            let under_idx: Vec<usize> =
                dp.under.iter().filter_map(|r| names.iter().position(|n| n == role_name(*r))).collect();
            let mut sc_labels = vec![];
            for (k, i) in under_idx.iter().enumerate() {
                ml.push(format!("[{i}:a]asplit=2[u{k}a][u{k}b]"));
                sc_labels.push(format!("[u{k}b]"));
            }
            let sc = if sc_labels.len() == 1 {
                sc_labels[0].clone()
            } else {
                ml.push(format!("{}amix=inputs={}:normalize=0[sc]", sc_labels.join(""), sc_labels.len()));
                "[sc]".to_string()
            };
            let ratio = (dp.params.amount_db.abs() / 2.0).clamp(2.0, 20.0);
            let tgt_idx: Vec<usize> =
                dp.target.iter().filter_map(|r| names.iter().position(|n| n == role_name(*r))).collect();
            let sc_label = if tgt_idx.len() > 1 {
                ml.push(format!(
                    "{sc}asplit={}{}",
                    tgt_idx.len(),
                    (0..tgt_idx.len()).map(|k| format!("[sc{k}]")).collect::<String>()
                ));
                None
            } else {
                Some(sc.clone())
            };
            for (k, i) in names.iter().enumerate() {
                if let Some(u) = under_idx.iter().position(|x| *x == k) {
                    mixed.push(format!("[u{u}a]"));
                } else if let Some(t) = tgt_idx.iter().position(|x| *x == k) {
                    let scl = sc_label.clone().unwrap_or_else(|| format!("[sc{t}]"));
                    ml.push(format!(
                        "[{k}:a]{scl}sidechaincompress=threshold=0.015:ratio={}:attack={}:release={}:makeup=1[d{k}]",
                        f(ratio),
                        f(dp.params.attack * 1000.0),
                        f(dp.params.release * 1000.0)
                    ));
                    mixed.push(format!("[d{k}]"));
                    let _ = i;
                } else {
                    mixed.push(format!("[{k}:a]"));
                }
            }
        } else {
            mj.args(["-i".to_string(), env_wav.to_string_lossy().to_string()]);
            let tgt: Vec<usize> =
                dp.target.iter().filter_map(|r| names.iter().position(|n| n == role_name(*r))).collect();
            ml.push(format!(
                "[{env_idx}:a]aresample={SAMPLE_RATE}:filter_type=cubic,aformat=sample_fmts=flt:channel_layouts=mono,pan=stereo|c0=c0|c1=c0,apad=whole_len={total},atrim=end_sample={total}{}",
                if tgt.len() > 1 { format!(",asplit={}{}", tgt.len(), (0..tgt.len()).map(|k| format!("[env{k}]")).collect::<String>()) } else { "[env0]".to_string() }
            ));
            for (k, _) in names.iter().enumerate() {
                if let Some(t) = tgt.iter().position(|x| *x == k) {
                    ml.push(format!("[{k}:a][env{t}]amultiply[d{k}]"));
                    mixed.push(format!("[d{k}]"));
                } else {
                    mixed.push(format!("[{k}:a]"));
                }
            }
        }
    } else {
        for (k, _) in names.iter().enumerate() {
            mixed.push(format!("[{k}:a]"));
        }
    }
    if mixed.len() == 1 {
        ml.push(format!("{}anull[mix]", mixed[0]));
    } else {
        ml.push(format!("{}amix=inputs={}:normalize=0:duration=longest[mix]", mixed.join(""), mixed.len()));
    }
    let mscript = work.join("audio_mix.filter");
    mj.args(tc.filter_complex_file_args(&mscript));
    mj.args(["-map", "[mix]", "-c:a", "pcm_f32le", "-ar", "48000", "-ac", "2"]);
    mj.arg(mix_wav.to_string_lossy().to_string());
    mj.scripts.push((mscript, ml.join(";\n") + "\n"));
    mj.outputs.push(mix_wav.clone());

    let lt = a.loudness.clone().unwrap_or_default();
    let lra = lt.lra.unwrap_or(tl.target.lra_max);
    let loud = (!opts.skip_loudness).then_some((lt.integrated, lt.true_peak, lra));
    let final_wav = if loud.is_some() { work.join("final_audio.wav") } else { mix_wav.clone() };
    Ok(AudioPlan {
        stems_job: j,
        stems,
        transcript_speech: extras.speech.clone(),
        duck,
        env_wav,
        mix_job: mj,
        mix_wav,
        final_wav,
        loudness: loud,
        duration: dur,
    })
}

/// Speech spans used for ducking: transcripts when available, else energy VAD on `under` stems.
pub fn speech_spans(tc: &Toolchain, a: &AudioPlan) -> Result<(Vec<(f64, f64)>, String)> {
    if !a.transcript_speech.is_empty() {
        return Ok((a.transcript_speech.clone(), "transcript".into()));
    }
    let Some(d) = &a.duck else { return Ok((vec![], "none".into())) };
    let mut sum: Vec<f32> = vec![];
    for r in &d.under {
        if let Some(p) = a.stems.get(role_name(*r)) {
            let s = wav::decode_mono(tc, p, 16000)?;
            if sum.len() < s.len() {
                sum.resize(s.len(), 0.0);
            }
            for (i, v) in s.iter().enumerate() {
                sum[i] += v;
            }
        }
    }
    Ok((vad::detect(&sum, 16000), "vad".into()))
}

pub fn execute(tc: &Toolchain, a: &AudioPlan) -> Result<Option<LoudnessResult>> {
    a.stems_job.run(tc)?;
    let (spans, src) = speech_spans(tc, a)?;
    if let Some(d) = &a.duck {
        if !d.sidechain {
            let env = envelope::envelope(
                &spans,
                a.duration + 0.01,
                ENV_RATE,
                d.params.amount_db,
                d.params.attack,
                d.params.release,
            );
            wav::write_f32(&a.env_wav, &env, ENV_RATE, 1)?;
        }
    }
    a.mix_job.run(tc)?;
    let Some((i, tp, lra)) = a.loudness else { return Ok(None) };
    // pass 1
    let err = run_af(tc, &a.mix_wav, &loudness::pass1(i, tp, lra), None)?;
    let m = loudness::parse_loudnorm_json(&err);
    let Some(m) = m else {
        // silent mix: nothing to normalize
        std::fs::copy(&a.mix_wav, &a.final_wav)?;
        return Ok(Some(LoudnessResult {
            input_i: f64::NEG_INFINITY,
            speech_spans: spans.len(),
            speech_source: src,
            ..Default::default()
        }));
    };
    let tmp = a.final_wav.with_extension("pass2.wav");
    run_af(tc, &a.mix_wav, &loudness::pass2(i, tp, lra, &m), Some(&tmp))?;
    let meas = cutline_media::probe::measure_loudness(tc, &tmp).context("measuring pass 2")?;
    // Correct residual error (loudnorm falls back to dynamic mode in some cases) with a gain
    // + true-peak limiter pass.
    let delta = i - meas.integrated_lufs;
    let mut correction = 0.0;
    let mut out_meas = meas.clone();
    if delta.abs() > 0.2 || meas.true_peak_dbtp > tp {
        correction = delta;
        let lim = 10f64.powf((tp - 0.3) / 20.0);
        let af = format!("volume={}dB,aresample=192000,alimiter=limit={:.5}:attack=1:release=50:level=disabled:latency=1,aresample=48000", f(delta), lim);
        run_af(tc, &tmp, &af, Some(&a.final_wav))?;
        out_meas = cutline_media::probe::measure_loudness(tc, &a.final_wav)?;
        let _ = std::fs::remove_file(&tmp);
    } else {
        std::fs::rename(&tmp, &a.final_wav)?;
    }
    Ok(Some(LoudnessResult {
        input_i: m.input_i,
        input_tp: m.input_tp,
        output_i: out_meas.integrated_lufs,
        output_tp: out_meas.true_peak_dbtp,
        output_lra: out_meas.lra,
        correction_db: correction,
        speech_spans: spans.len(),
        speech_source: src,
    }))
}

fn run_af(tc: &Toolchain, input: &Path, af: &str, out: Option<&Path>) -> Result<String> {
    let mut c = std::process::Command::new(&tc.ffmpeg);
    c.args(["-hide_banner", "-nostdin", "-y", "-nostats", "-i"]).arg(input).args(["-af", af]);
    match out {
        Some(o) => {
            c.args(["-c:a", "pcm_f32le", "-ar", "48000", "-ac", "2"]).arg(o);
        }
        None => {
            c.args(["-f", "null", "-"]);
        }
    }
    tracing::debug!(?c, "ffmpeg");
    let o = c.output()?;
    let err = String::from_utf8_lossy(&o.stderr).to_string();
    anyhow::ensure!(
        o.status.success(),
        "ffmpeg audio pass failed: {}",
        err.lines().rev().take(10).collect::<Vec<_>>().join("\n")
    );
    Ok(err)
}

pub fn jobs_for_display(a: &AudioPlan) -> Vec<Job> {
    let mut v = vec![a.stems_job.clone(), a.mix_job.clone()];
    if let Some((i, tp, lra)) = a.loudness {
        let mut p1 = Job::new("loudness pass 1 (measure)");
        p1.args(["-hide_banner", "-nostdin", "-nostats", "-i"]).arg(a.mix_wav.to_string_lossy().to_string()).args([
            "-af",
            &loudness::pass1(i, tp, lra),
            "-f",
            "null",
            "-",
        ]);
        let mut p2 = Job::new("loudness pass 2 (apply; values from pass 1)");
        p2.args(["-hide_banner", "-nostdin", "-y", "-i"])
            .arg(a.mix_wav.to_string_lossy().to_string())
            .args(["-af", &loudness::pass2_template(i, tp, lra), "-c:a", "pcm_f32le"])
            .arg(a.final_wav.to_string_lossy().to_string());
        v.push(p1);
        v.push(p2);
    }
    v
}

/// Momentary loudness curve (t, LUFS) of a file via `ebur128` frame log.
pub fn momentary(tc: &Toolchain, p: &Path) -> Result<Vec<(f64, f64)>> {
    let o = std::process::Command::new(&tc.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "verbose", "-i"])
        .arg(p)
        .args(["-vn", "-af", "ebur128=framelog=verbose", "-f", "null", "-"])
        .output()?;
    let s = String::from_utf8_lossy(&o.stderr);
    let mut v = vec![];
    for line in s.lines() {
        let (Some(tp), Some(mp)) = (line.find(" t: "), line.find(" M: ")) else { continue };
        let t = line[tp + 4..].split_whitespace().next().and_then(|x| x.parse::<f64>().ok());
        let m = line[mp + 4..].split_whitespace().next().and_then(|x| x.parse::<f64>().ok());
        if let (Some(t), Some(m)) = (t, m) {
            v.push((t, m.max(-70.0)));
        }
    }
    Ok(v)
}

/// Render `audio.png` from the stems of an executed plan plus the final output.
pub fn audio_png(tc: &Toolchain, a: &AudioPlan, final_media: &Path, out: &Path) -> Result<()> {
    let rate = 8000;
    let mut tracks_data: Vec<(String, Vec<f32>, [u8; 3])> = vec![];
    for (name, color) in
        [("dialogue", [110, 200, 120]), ("voice", [90, 170, 255]), ("music", [230, 120, 220]), ("sfx", [255, 210, 90])]
    {
        if let Some(p) = a.stems.get(name) {
            if p.exists() {
                tracks_data.push((name.to_string(), wav::decode_mono(tc, p, rate)?, color));
            }
        }
    }
    tracks_data.push(("final mix".into(), wav::decode_mono(tc, final_media, rate)?, [235, 235, 235]));
    let env = if a.env_wav.exists() && a.duck.as_ref().map(|d| !d.sidechain).unwrap_or(false) {
        Some(wav::decode_mono(tc, &a.env_wav, ENV_RATE)?)
    } else {
        None
    };
    let mom = momentary(tc, final_media)?;
    let tracks: Vec<cutline_audio::viz::Track> = tracks_data
        .iter()
        .map(|(n, s, c)| cutline_audio::viz::Track { name: n, samples: s, rate, color: *c })
        .collect();
    let target = a.loudness.map(|l| l.0).unwrap_or(-14.0);
    let img =
        cutline_audio::viz::render(&tracks, env.as_deref().map(|e| (e, ENV_RATE)), &mom, target, a.duration, 1568);
    img.save(out)?;
    Ok(())
}
