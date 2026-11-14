//! Browser recordings as assets: an asset whose path is a `.demo.json5` script is recorded in
//! headless Chrome when the project loads (cached by the script) and replaced by the video.
//! The recording's named steps can then be used in the edit:
//!
//! * clip `in` / `out`: `"step:export"`, `"step:export-0.5"` (seconds in the recording);
//! * `zoom_to` / `punch_in` effects: `step: "export"` instead of a `rect` / `focus`;
//! * `highlight`, `spotlight` and `mask` layers: `step: "export"` instead of a `rect`.
//!
//! References are replaced by numbers and rects before the project is typed, so validation,
//! `expand` and rendering see an ordinary project.

use cutline_core::diag::{ptr_token, Diagnostic};
use cutline_html::demo::{self, DemoEvents, StepEvent};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// Events of every recorded demo asset, by asset id.
pub type Demos = BTreeMap<String, DemoEvents>;

pub fn cache_dir(dir: &Path) -> std::path::PathBuf {
    crate::cutline_dir(dir).join("cache").join("browser")
}

/// Record (or, with `record: false`, only look up) every `.demo.json5` asset in the raw
/// project and point the asset at its video.
pub fn prepare(v: &mut Value, dir: &Path, record: bool, locale: Option<&str>) -> (Vec<Diagnostic>, Demos) {
    let mut d = vec![];
    let mut demos = Demos::new();
    let Some(assets) = v.get_mut("assets").and_then(|a| a.as_object_mut()) else { return (d, demos) };
    for (id, a) in assets.iter_mut() {
        let Some(path) = a.get("path").and_then(|p| p.as_str()).filter(|p| demo::is_demo(p)).map(str::to_string) else {
            continue;
        };
        let script = dir.join(&path);
        let at = format!("/assets/{}/path", ptr_token(id));
        if !script.is_file() {
            d.push(Diagnostic::error("E_ASSET_NOT_FOUND", at, format!("demo script not found: {path}")));
            continue;
        }
        let cache = cache_dir(dir);
        let rec = if record {
            cutline_deps::toolchain().and_then(|tc| demo::record_cached(tc, &script, &cache, false, locale)).map(Some)
        } else {
            demo::cached(&script, &cache, locale)
        };
        match rec {
            Ok(Some(r)) => {
                a["path"] = json!(r.video.to_string_lossy());
                if !r.events.blocked.is_empty() {
                    d.push(
                        Diagnostic::warning(
                            "W_DEMO_BLOCKED",
                            at,
                            format!(
                                "{} request(s) were blocked while recording, e.g. {}",
                                r.events.blocked.len(),
                                r.events.blocked[0]
                            ),
                        )
                        .hint("add the origin to the script's `allow` list if the page needs it"),
                    );
                }
                demos.insert(id.clone(), r.events);
            }
            Ok(None) => {}
            Err(e) => {
                let msg = format!("{e:#}");
                let code = if msg.contains("step ") { "E_DEMO_STEP_FAILED" } else { "E_DEMO_FAILED" };
                d.push(Diagnostic::error(code, at, format!("recording {path} failed: {msg}")).hint(
                    "check the selector against the page; `{wait: \"selector\"}` before a step gives slow pages time",
                ));
            }
        }
    }
    (d, demos)
}

/// `step:name`, `step:name+0.4`, `step:name-0.25` → (name, offset).
fn split_ref(r: &str, ev: &DemoEvents) -> Option<(String, f64)> {
    let body = r.trim().strip_prefix("step:")?;
    if ev.step(body).is_some() {
        return Some((body.to_string(), 0.0));
    }
    let pos = body.rfind(['+', '-'])?;
    let off: f64 = body[pos..].parse().ok()?;
    Some((body[..pos].to_string(), off))
}

fn unknown(name: &str, ev: &DemoEvents, at: String) -> Diagnostic {
    Diagnostic::error("E_UNKNOWN_STEP", at, format!("the recording has no step named `{name}`"))
        .hint(format!("named steps: {}", ev.names().join(", ")))
}

fn rect_json(st: &StepEvent) -> Option<Value> {
    st.rect.map(|r| json!({"x": r[0], "y": r[1], "w": r[2].max(1e-4), "h": r[3].max(1e-4)}))
}

fn secs(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => cutline_core::time::parse_time(s)
            .ok()
            .filter(|t| !t.is_end())
            .map(|t| t.seconds(cutline_core::time::Fps::new(30, 1), 0.0)),
        _ => None,
    }
}

/// A clip that plays a recording: where it starts in the recording, and its speed.
struct RecClip {
    id: String,
    asset: String,
    src_in: f64,
    speed: f64,
}

/// Replace every step reference in the raw project. Returns diagnostics (errors stop the load).
pub fn resolve_steps(v: &mut Value, demos: &Demos) -> Vec<Diagnostic> {
    let mut d = vec![];
    let r6 = cutline_core::time::round6;
    let mut clips: Vec<RecClip> = vec![];
    if let Some(main) = v.get_mut("main").and_then(|m| m.as_array_mut()) {
        for (mi, item) in main.iter_mut().enumerate() {
            let Some(asset) = item.get("clip").and_then(|c| c.as_str()).map(str::to_string) else { continue };
            let ev = demos.get(&asset);
            // source times
            for field in ["in", "out"] {
                let Some(r) = item.get(field).and_then(|x| x.as_str()).filter(|s| s.trim().starts_with("step:")) else {
                    continue;
                };
                let at = format!("/main/{mi}/{field}");
                let Some(ev) = ev else {
                    d.push(Diagnostic::error(
                        "E_STEP_NOT_A_RECORDING",
                        at,
                        format!("`{r}`: asset `{asset}` is not a browser recording (a `.demo.json5` script)"),
                    ));
                    continue;
                };
                match split_ref(r, ev).and_then(|(n, off)| ev.step(&n).map(|s| (s.t + off).clamp(0.0, ev.duration))) {
                    Some(t) => item[field] = json!(r6(t)),
                    None => d.push(unknown(r.trim().trim_start_matches("step:"), ev, at)),
                }
            }
            let src_in = secs(item.get("in")).unwrap_or(0.0);
            let speed = item.get("speed").and_then(|s| s.as_f64()).filter(|s| *s > 0.0).unwrap_or(1.0);
            let id = item.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            // effects that name a step
            if let Some(effects) = item.get_mut("effects").and_then(|e| e.as_array_mut()) {
                for (ei, e) in effects.iter_mut().enumerate() {
                    let Some(name) = e.get("step").and_then(|s| s.as_str()).map(str::to_string) else { continue };
                    let at = format!("/main/{mi}/effects/{ei}/step");
                    let Some(ev) = ev else {
                        d.push(Diagnostic::error(
                            "E_STEP_NOT_A_RECORDING",
                            at,
                            format!("asset `{asset}` is not a browser recording, so it has no steps"),
                        ));
                        continue;
                    };
                    let Some(st) = ev.step(&name) else {
                        d.push(unknown(&name, ev, at));
                        continue;
                    };
                    let Some(rect) = rect_json(st) else {
                        d.push(Diagnostic::error(
                            "E_STEP_NO_RECT",
                            at,
                            format!("step `{name}` ({}) touched no element", st.kind),
                        ));
                        continue;
                    };
                    let o = e.as_object_mut().unwrap();
                    o.remove("step");
                    let kind = o.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
                    // clip-relative times of the step: when the pointer sets off, when it acts
                    let (set_off, acts) =
                        (r6(((st.t0 - src_in) / speed).max(0.0)), r6(((st.t - src_in) / speed).max(0.0)));
                    match kind.as_str() {
                        "zoom_to" => {
                            o.insert("rect".into(), rect);
                            o.entry("start").or_insert(json!(set_off));
                        }
                        "punch_in" => {
                            let r = st.rect.unwrap();
                            o.insert("focus".into(), json!({"x": r[0] + r[2] / 2.0, "y": r[1] + r[3] / 2.0}));
                            o.entry("at").or_insert(json!(acts));
                        }
                        other => d.push(Diagnostic::error(
                            "E_STEP_NOT_ALLOWED",
                            at,
                            format!("effect `{other}` cannot take `step` (zoom_to and punch_in can)"),
                        )),
                    }
                }
            }
            if ev.is_some() {
                clips.push(RecClip { id, asset, src_in, speed });
            }
        }
    }
    if let Some(layers) = v.get_mut("layers").and_then(|l| l.as_array_mut()) {
        for (li, l) in layers.iter_mut().enumerate() {
            let Some(name) = l.get("step").and_then(|s| s.as_str()).map(str::to_string) else { continue };
            let at = format!("/layers/{li}/step");
            let kind = l.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
            if !["highlight", "spotlight", "mask"].contains(&kind.as_str()) {
                d.push(Diagnostic::error(
                    "E_STEP_NOT_ALLOWED",
                    at,
                    format!("layer `{kind}` cannot take `step` (highlight, spotlight and mask can)"),
                ));
                continue;
            }
            // the clip: the anchor, else the only recording clip that has this step
            let anchor = l.get("anchor").and_then(|a| a.as_str()).map(str::to_string);
            let cands: Vec<&RecClip> = clips
                .iter()
                .filter(|c| anchor.as_ref().map_or(true, |a| &c.id == a))
                .filter(|c| demos.get(&c.asset).is_some_and(|ev| ev.step(&name).is_some()))
                .collect();
            let Some(c) = (cands.len() == 1 || (anchor.is_some() && !cands.is_empty())).then(|| cands[0]) else {
                let all: Vec<&str> = demos.values().flat_map(|e| e.names()).collect();
                d.push(
                    Diagnostic::error(
                        "E_UNKNOWN_STEP",
                        at,
                        if cands.is_empty() {
                            format!("no clip of a browser recording has a step named `{name}`")
                        } else {
                            format!("several clips play the recording with step `{name}`; add `anchor` to choose one")
                        },
                    )
                    .hint(format!("named steps: {}", all.join(", "))),
                );
                continue;
            };
            let st = demos[&c.asset].step(&name).unwrap();
            let Some(rect) = rect_json(st) else {
                d.push(Diagnostic::error(
                    "E_STEP_NO_RECT",
                    at,
                    format!("step `{name}` ({}) touched no element", st.kind),
                ));
                continue;
            };
            let o = l.as_object_mut().unwrap();
            o.remove("step");
            o.insert("rect".into(), rect);
            o.insert("anchor".into(), json!(c.id));
            if kind == "mask" {
                // the element's rect is in the recording's own fractions: the mask must move
                // with the clip's frame and zoom, which a filled mask can
                o.entry("mode").or_insert(json!("fill"));
                o.entry("follow").or_insert(json!(true));
            }
            let timed = ["start", "end", "offset", "duration"].iter().any(|k| o.contains_key(*k));
            if !timed && kind != "mask" {
                // from when the pointer sets off until a second after the step is done
                let from = ((st.t0 - c.src_in) / c.speed).max(0.0);
                let to = ((st.t1 - c.src_in) / c.speed).max(from) + 1.0;
                o.insert("offset".into(), json!(r6(from)));
                o.insert("duration".into(), json!(r6(to - from)));
            }
        }
    }
    d
}

/// Re-record the `.demo.json5` assets of a project (all, or the one named `only`).
pub fn rerecord(
    project: &Path,
    only: Option<&str>,
    locale: Option<&str>,
) -> anyhow::Result<Vec<(String, std::path::PathBuf)>> {
    use anyhow::Context;
    let project = std::fs::canonicalize(project).with_context(|| format!("cannot open {}", project.display()))?;
    let dir = project.parent().unwrap_or(Path::new("."));
    let text = std::fs::read_to_string(&project)?;
    let v = cutline_core::parse::parse_json5(&text).map_err(|d| anyhow::anyhow!("{d:?}"))?;
    let tc = cutline_deps::toolchain()?;
    let mut out = vec![];
    for (id, a) in v.get("assets").and_then(|a| a.as_object()).into_iter().flatten() {
        let Some(path) = a.get("path").and_then(|p| p.as_str()).filter(|p| demo::is_demo(p)) else { continue };
        if only.is_some_and(|o| o != id) {
            continue;
        }
        let r = demo::record_cached(tc, &dir.join(path), &cache_dir(dir), true, locale)?;
        out.push((id.clone(), r.video));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutline_html::demo::CursorPoint;

    fn demos() -> Demos {
        let ev = DemoEvents {
            width: 800,
            height: 500,
            scale: 2.0,
            duration: 10.0,
            cursor: vec![CursorPoint { t: 0.0, x: 0.5, y: 0.5, click: false }],
            steps: vec![
                StepEvent {
                    index: 0,
                    name: Some("export".into()),
                    kind: "click".into(),
                    t0: 3.0,
                    t: 3.8,
                    t1: 3.8,
                    rect: Some([0.1, 0.2, 0.25, 0.1]),
                },
                StepEvent {
                    index: 1,
                    name: Some("send".into()),
                    kind: "press".into(),
                    t0: 6.0,
                    t: 6.0,
                    t1: 6.0,
                    rect: None,
                },
            ],
            keys: vec![],
            blocked: vec![],
        };
        [("rec".to_string(), ev)].into_iter().collect()
    }

    #[test]
    fn step_references_become_times_and_rects() {
        let mut v = json!({
            "main": [{"id": "a", "clip": "rec", "in": "step:export-1.8", "out": "step:send+1",
                      "effects": [{"type": "zoom_to", "step": "export"}, {"type": "punch_in", "step": "export", "scale": 1.2}]}],
            "layers": [{"type": "highlight", "step": "export"}, {"type": "mask", "step": "export", "mode": "fill"}],
        });
        let d = resolve_steps(&mut v, &demos());
        assert!(d.is_empty(), "{d:?}");
        let c = &v["main"][0];
        assert_eq!(c["in"], json!(2.0));
        assert_eq!(c["out"], json!(7.0));
        // the zoom starts as the pointer sets off: 3.0 s in the recording = 1.0 s into the clip
        assert_eq!(
            c["effects"][0],
            json!({"type": "zoom_to", "rect": {"x": 0.1, "y": 0.2, "w": 0.25, "h": 0.1}, "start": 1.0})
        );
        assert_eq!(c["effects"][1]["focus"], json!({"x": 0.225, "y": 0.25}));
        assert_eq!(c["effects"][1]["at"], json!(1.8));
        let h = &v["layers"][0];
        assert_eq!(
            (h["anchor"].clone(), h["offset"].clone(), h["duration"].clone()),
            (json!("a"), json!(1.0), json!(1.8))
        );
        assert!(h.get("step").is_none() && h["rect"]["w"] == json!(0.25));
        // a mask keeps its own timing (the whole video unless told otherwise) and follows the clip
        assert!(v["layers"][1].get("offset").is_none() && v["layers"][1]["anchor"] == json!("a"));
        assert_eq!(v["layers"][1]["follow"], json!(true));
    }

    #[test]
    fn bad_step_references_are_reported() {
        let mut v = json!({
            "main": [{"id": "a", "clip": "rec", "in": "step:nope", "effects": [{"type": "zoom_to", "step": "send"}, {"type": "shake", "step": "export", "at": 1}]},
                     {"id": "b", "clip": "other", "out": "step:export"}],
            "layers": [{"type": "arrow", "step": "export"}, {"type": "highlight", "step": "missing"}],
        });
        let d = resolve_steps(&mut v, &demos());
        let has = |code: &str, path: &str| d.iter().any(|x| x.code == code && x.path == path);
        assert!(has("E_UNKNOWN_STEP", "/main/0/in"), "{d:?}");
        assert!(has("E_STEP_NO_RECT", "/main/0/effects/0/step"), "{d:?}");
        assert!(has("E_STEP_NOT_ALLOWED", "/main/0/effects/1/step"), "{d:?}");
        assert!(has("E_STEP_NOT_A_RECORDING", "/main/1/out"), "{d:?}");
        assert!(has("E_STEP_NOT_ALLOWED", "/layers/0/step"), "{d:?}");
        assert!(has("E_UNKNOWN_STEP", "/layers/1/step"), "{d:?}");
        assert!(d.iter().find(|x| x.path == "/main/0/in").unwrap().hint.as_deref().unwrap().contains("export, send"));
    }
}
