//! Final assembly: join segment mezzanines (concat/xfade), then overlays, then encode.

use crate::escape::f;
use crate::transitions;
use crate::Geometry;
use cutline_core::model::Ease;
use cutline_core::timeline::Timeline;

/// Video graph joining `[i:v]` segment inputs. Returns (graph lines, output label).
pub fn join_video(tl: &Timeline, g: &Geometry) -> (Vec<String>, String) {
    let mut lines = vec![];
    let mut run: Vec<String> = vec![];
    let mut cur: Option<String> = None;
    let mut len_frames: i64 = 0;
    let mut n = 0usize;
    let mut fresh = |p: &str| {
        n += 1;
        format!("{p}{n}")
    };
    for (i, c) in tl.clips.iter().enumerate() {
        // normalize timebases so concat/xfade outputs can be chained
        lines.push(format!("[{i}:v]settb=AVTB,setpts=PTS-STARTPTS[s{i}]"));
        let inp = format!("[s{i}]");
        let t = tl.transitions.iter().find(|t| t.after + 1 == i);
        match t {
            Some(t) if t.frames > 0 && t.transition.kind != "cut" => {
                // flush pending concat run
                let base = flush(&mut run, &mut lines, &mut fresh, cur.take());
                let out = fresh("x");
                let offset = g.fps.frames_to_secs(len_frames - t.frames);
                let ease =
                    t.transition.ease.unwrap_or_else(|| cutline_core::catalog::default_transition_ease(&t.transition.kind));
                if ease == Ease::Linear {
                    lines.push(transitions::pair(&base, &inp, &t.transition, t.frames, offset, g, &out));
                } else {
                    // xfade pairs frames by count and takes its progress from the first
                    // input's timestamps. Bending those timestamps inside the overlap eases
                    // every transition kind at no cost; the frames themselves are untouched,
                    // and the timestamps are made regular again right after.
                    let d = g.fps.frames_to_secs(t.frames);
                    let u = format!("clip((T-{})/{},0,1)", f(offset), f(d));
                    let warped = fresh("w");
                    lines.push(format!(
                        "{base}setpts='if(between(T,{o},{e}),({o}+{d}*{ez})/TB,PTS)'[{warped}]",
                        o = f(offset),
                        e = f(offset + d - 0.5 / g.fps.as_f64()),
                        d = f(d),
                        ez = crate::effects::ease_expr(ease, &u),
                    ));
                    let raw = fresh("x");
                    lines.push(transitions::pair(
                        &format!("[{warped}]"),
                        &inp,
                        &t.transition,
                        t.frames,
                        offset,
                        g,
                        &raw,
                    ));
                    lines.push(format!("[{raw}]setpts=N/({})/TB[{out}]", g.fps.ff()));
                }
                cur = Some(format!("[{out}]"));
                len_frames += c.frames - t.frames;
            }
            _ => {
                if let Some(cl) = cur.take() {
                    run.push(cl);
                }
                run.push(inp);
                len_frames += c.frames;
            }
        }
    }
    let out = flush(&mut run, &mut lines, &mut fresh, cur.take());
    debug_assert_eq!(len_frames, tl.total_frames);
    (lines, out.trim_matches(|c| c == '[' || c == ']').to_string())
}

fn flush(
    run: &mut Vec<String>,
    lines: &mut Vec<String>,
    fresh: &mut impl FnMut(&str) -> String,
    cur: Option<String>,
) -> String {
    let mut items: Vec<String> = cur.into_iter().collect();
    items.append(run);
    match items.len() {
        0 => panic!("empty join"),
        1 => items.pop().unwrap(),
        k => {
            let out = fresh("c");
            lines.push(format!("{}concat=n={k}:v=1:a=0[{out}]", items.join("")));
            format!("[{out}]")
        }
    }
}

/// Dialogue join: `[i:a]` → concat / acrossfade with exact sample overlaps. Transitions with
/// `audio: cut` hard-cut at the overlap midpoint (with micro-fades).
pub fn join_dialogue(tl: &Timeline, g: &Geometry, input_offset: usize) -> (Vec<String>, String) {
    use crate::segment::samples_at;
    use cutline_core::model::TransitionAudio;
    let mut lines = vec![];
    let n = tl.clips.len();
    let mut head = vec![0i64; n];
    let mut tail = vec![0i64; n];
    let mut cross: Vec<Option<(i64, String)>> = vec![None; n];
    for t in &tl.transitions {
        if t.frames == 0 {
            continue;
        }
        let ns = samples_at(t.start + t.frames, g) - samples_at(t.start, g);
        let hard = t.transition.kind == "cut" || t.transition.audio == Some(TransitionAudio::Cut);
        if hard {
            tail[t.after] = ns / 2;
            head[t.after + 1] = ns - ns / 2;
        } else {
            cross[t.after + 1] = Some((ns, t.transition.curve.clone().unwrap_or_else(|| "tri".into())));
        }
    }
    let mut labels = vec![];
    for (i, c) in tl.clips.iter().enumerate() {
        let inp = format!("[{}:a]", i + input_offset);
        if head[i] > 0 || tail[i] > 0 {
            let total = samples_at(c.end(), g) - samples_at(c.start, g);
            let keep = total - head[i] - tail[i];
            let secs = keep as f64 / 48000.0;
            lines.push(format!(
                "{inp}atrim=start_sample={}:end_sample={},asetpts=N/SR/TB,afade=t=in:d=0.01,afade=t=out:st={}:d=0.01[at{i}]",
                head[i],
                total - tail[i],
                f((secs - 0.01).max(0.0))
            ));
            labels.push(format!("[at{i}]"));
        } else {
            labels.push(inp);
        }
    }
    let mut k = 0usize;
    let mut run: Vec<String> = vec![];
    for (i, l) in labels.into_iter().enumerate() {
        if let Some((ns, curve)) = &cross[i] {
            let base = if run.len() == 1 {
                run.pop().unwrap()
            } else {
                k += 1;
                lines.push(format!("{}concat=n={}:v=0:a=1[ac{k}]", run.join(""), run.len()));
                run.clear();
                format!("[ac{k}]")
            };
            k += 1;
            lines.push(format!("{base}{l}acrossfade=ns={ns}:c1={curve}:c2={curve}[ax{k}]"));
            run = vec![format!("[ax{k}]")];
        } else {
            run.push(l);
        }
    }
    let out = if run.len() == 1 {
        run.pop().unwrap()
    } else {
        k += 1;
        lines.push(format!("{}concat=n={}:v=0:a=1[ac{k}]", run.join(""), run.len()));
        format!("[ac{k}]")
    };
    let total = samples_at(tl.total_frames, g);
    lines.push(format!("{out}apad=whole_len={total},atrim=end_sample={total}[dialogue]"));
    (lines, "dialogue".into())
}

pub fn secs(v: f64) -> String {
    f(v)
}
