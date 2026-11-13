//! Cue references (`"cue:name"`, `"cue:name+0.4"`): named phrases in a voice track, found in
//! its transcript and replaced by timeline seconds before the project is typed. After this
//! pass a project holds only explicit times, so validation, `expand` and rendering need no
//! knowledge of cues.

use anyhow::Result;
use cutline_core::diag::{ptr_token, Diagnostic};
use cutline_core::model::{Asset, Cue, CueEdge, Role};
use cutline_core::time::{parse_time, round6, TimeValue};
use cutline_transcribe::Transcript;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Fields that hold timeline times and may use cue references.
const TIMELINE_FIELDS: &[(&str, &str)] =
    &[("main", "end_at"), ("layers", "start"), ("layers", "end"), ("layers", "at"), ("markers", "at")];
/// Every time-valued field name (cue references anywhere else are an error).
const TIME_FIELDS: &[&str] = &["in", "out", "duration", "start", "end", "at", "end_at", "src_in", "src_out"];

/// A resolved cue (reported by `cutline cues`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolvedCue {
    pub name: String,
    pub phrase: String,
    pub asset: String,
    /// Timeline seconds.
    pub at: f64,
    /// Matched transcript text and its source-time span.
    pub matched: String,
    pub source_start: f64,
    pub source_end: f64,
    pub exact: bool,
}

/// A voice layer pinned to a clip (`anchor` + `offset`) that a cue was measured from.
#[derive(Debug, Clone, PartialEq)]
pub struct AnchoredVoice {
    pub layer: usize,
    pub anchor: String,
    pub offset: f64,
}

/// Resolve every cue reference in `v` in place. Returns diagnostics (errors stop the load)
/// and the resolved cues. Voice layers anchored to a clip are taken to start at 0; use
/// `resolve_with` to supply their timeline starts.
pub fn resolve(v: &mut Value, dir: &Path) -> (Vec<Diagnostic>, Vec<ResolvedCue>) {
    let (d, c, _) = resolve_with(v, dir, &BTreeMap::new());
    (d, c)
}

/// `resolve`, with the timeline start (seconds) of anchored voice layers by layer index. Also
/// returns the anchored layers the cues were measured from, so the caller can find their real
/// starts and resolve again.
pub fn resolve_with(
    v: &mut Value,
    dir: &Path,
    anchored_starts: &BTreeMap<usize, f64>,
) -> (Vec<Diagnostic>, Vec<ResolvedCue>, Vec<AnchoredVoice>) {
    let mut d = vec![];
    let mut refs: Vec<(String, String)> = vec![]; // (pointer, reference)
    collect(v, "", None, &mut refs);
    let mut allowed = vec![];
    for (ptr, r) in refs {
        if is_timeline_field(&ptr) {
            allowed.push((ptr, r));
        } else {
            d.push(
                Diagnostic::error("E_CUE_NOT_ALLOWED", ptr, format!("`{r}`: cue references are timeline times")).hint(
                    "use cues in layer start/end, sfx at, clip end_at or marker at; source times (in/out) need numbers",
                ),
            );
        }
    }
    let cues: BTreeMap<String, Cue> =
        v.get("cues").and_then(|c| serde_json::from_value(c.clone()).ok()).unwrap_or_default();
    if allowed.is_empty() || !d.is_empty() {
        return (d, vec![], vec![]);
    }
    let mut ctx = match Ctx::new(v, dir, anchored_starts) {
        Ok(c) => c,
        Err(e) => {
            d.push(Diagnostic::error("E_CUE_TRANSCRIPT", "/cues", format!("{e:#}")));
            return (d, vec![], vec![]);
        }
    };
    let mut resolved: BTreeMap<String, Option<ResolvedCue>> = BTreeMap::new();
    for (ptr, r) in &allowed {
        let Some((name, offset)) = split_ref(r, &cues) else {
            d.push(Diagnostic::error("E_UNKNOWN_CUE", ptr.clone(), format!("`{r}` names no cue")).hint(
                if cues.is_empty() {
                    "define it under `cues`, e.g. `cues: {clocks: \"World clocks\"}`".to_string()
                } else {
                    format!("defined cues: {}", cues.keys().cloned().collect::<Vec<_>>().join(", "))
                },
            ));
            continue;
        };
        if !resolved.contains_key(&name) {
            let rc = ctx.cue(&name, &cues[&name], &mut d);
            resolved.insert(name.clone(), rc);
        }
        let Some(rc) = resolved[&name].as_ref() else { continue };
        let t = rc.at + offset;
        if t < 0.0 {
            d.push(Diagnostic::error("E_BAD_TIME", ptr.clone(), format!("`{r}` resolves to {t:.3}s, before 0")));
            continue;
        }
        if let Some(slot) = v.pointer_mut(ptr) {
            *slot = serde_json::json!(round6(t));
        }
    }
    (d, resolved.into_values().flatten().collect(), ctx.used_anchors)
}

fn is_timeline_field(ptr: &str) -> bool {
    let parts: Vec<&str> = ptr.trim_start_matches('/').split('/').collect();
    parts.len() == 3
        && parts[1].parse::<usize>().is_ok()
        && TIMELINE_FIELDS.iter().any(|(sec, field)| parts[0] == *sec && parts[2] == *field)
}

fn collect(v: &Value, ptr: &str, key: Option<&str>, out: &mut Vec<(String, String)>) {
    match v {
        Value::String(s) if s.trim_start().starts_with("cue:") && key.is_some_and(|k| TIME_FIELDS.contains(&k)) => {
            out.push((ptr.to_string(), s.trim().to_string()))
        }
        Value::Object(o) => {
            for (k, x) in o {
                // the cue definitions themselves are phrases, not references
                if ptr.is_empty() && k == "cues" {
                    continue;
                }
                collect(x, &format!("{ptr}/{}", ptr_token(k)), Some(k), out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                collect(x, &format!("{ptr}/{i}"), key, out);
            }
        }
        _ => {}
    }
}

/// `cue:name`, `cue:name+0.4`, `cue:name-0.25` → (name, offset). Names may contain `-`, so a
/// numeric suffix only counts as an offset when the rest is a defined cue.
fn split_ref(r: &str, cues: &BTreeMap<String, Cue>) -> Option<(String, f64)> {
    let body = r.trim().strip_prefix("cue:")?;
    if let Some(pos) = body.rfind(['+', '-']) {
        let (name, off) = body.split_at(pos);
        if let Ok(o) = off.parse::<f64>() {
            if cues.contains_key(name) {
                return Some((name.to_string(), o));
            }
        }
    }
    cues.contains_key(body).then(|| (body.to_string(), 0.0))
}

/// Seconds of a plain time value in the raw project (no fps needed for seconds/timecodes).
fn plain_secs(v: Option<&Value>) -> Result<f64, String> {
    match v {
        None => Ok(0.0),
        Some(Value::Number(n)) => n.as_f64().ok_or_else(|| "invalid number".into()),
        Some(Value::String(s)) => match parse_time(s)? {
            TimeValue::Seconds(x) => Ok(x),
            _ => Err(format!("`{s}` must be seconds or a timecode here")),
        },
        Some(other) => Err(format!("invalid time {other}")),
    }
}

/// An audio layer that plays a voice asset.
struct VoiceLayer {
    index: usize,
    start: Result<f64, String>,
    src_in: Result<f64, String>,
    /// `(anchor clip id, offset)` when the layer is pinned to a clip
    anchor: Option<(String, f64)>,
}

struct Ctx<'a> {
    dir: &'a Path,
    assets: BTreeMap<String, Asset>,
    /// asset id → audio layers playing it
    layers: BTreeMap<String, Vec<VoiceLayer>>,
    voice_assets: Vec<String>,
    lang: Option<String>,
    transcripts: BTreeMap<String, Option<Transcript>>,
    used_anchors: Vec<AnchoredVoice>,
}

impl<'a> Ctx<'a> {
    fn new(v: &Value, dir: &'a Path, anchored_starts: &BTreeMap<usize, f64>) -> Result<Self> {
        let assets: BTreeMap<String, Asset> =
            v.get("assets").map(|a| serde_json::from_value(a.clone())).transpose()?.unwrap_or_default();
        let mut layers: BTreeMap<String, Vec<VoiceLayer>> = BTreeMap::new();
        let mut voice_assets = vec![];
        for (i, l) in v.get("layers").and_then(|l| l.as_array()).into_iter().flatten().enumerate() {
            if l.get("type").and_then(|t| t.as_str()) != Some("audio") {
                continue;
            }
            let Some(asset) = l.get("asset").and_then(|a| a.as_str()) else { continue };
            let anchor = match l.get("anchor") {
                None => None,
                Some(a) => match a.as_str() {
                    Some(id) => {
                        let off = l.get("offset").and_then(|o| o.as_f64()).unwrap_or(0.0);
                        Some((id.to_string(), off))
                    }
                    None => None,
                },
            };
            let start = match (&anchor, l.get("anchor")) {
                // provisional until the caller supplies the anchor clip's timeline start
                (Some(_), _) => Ok(anchored_starts.get(&i).copied().unwrap_or(0.0)),
                (None, Some(_)) => Err("has an invalid anchor".to_string()),
                (None, None) => plain_secs(l.get("start")),
            };
            layers.entry(asset.to_string()).or_default().push(VoiceLayer {
                index: i,
                start,
                src_in: plain_secs(l.get("src_in")),
                anchor,
            });
            let role = l
                .get("role")
                .and_then(|r| serde_json::from_value::<Role>(r.clone()).ok())
                .or(assets.get(asset).and_then(|a| a.role));
            if role == Some(Role::Voice) && !voice_assets.iter().any(|a| a == asset) {
                voice_assets.push(asset.to_string());
            }
        }
        let lang = v.pointer("/captions/lang").and_then(|l| l.as_str()).map(String::from);
        Ok(Self { dir, assets, layers, voice_assets, lang, transcripts: BTreeMap::new(), used_anchors: vec![] })
    }

    /// The asset's transcript (script-aligned when it has a `script`), or `None` after
    /// reporting why there is none.
    fn load(&self, asset: &str, at: &str, d: &mut Vec<Diagnostic>) -> Option<Transcript> {
        let a = self.assets.get(asset);
        let mut missing = false;
        for (field, file) in a.map(|a| [("transcript", &a.transcript), ("script", &a.script)]).into_iter().flatten() {
            if let Some(f) = file {
                if !self.dir.join(f).exists() {
                    d.push(Diagnostic::error(
                        "E_ASSET_NOT_FOUND",
                        format!("/assets/{}/{field}", ptr_token(asset)),
                        format!("{field} file not found: {f}"),
                    ));
                    missing = true;
                }
            }
        }
        if missing {
            return None;
        }
        match crate::auto::load_transcript(self.dir, asset, a, self.lang.as_deref(), true) {
            Ok(Some(t)) => Some(t),
            Ok(None) => {
                d.push(
                    Diagnostic::error("E_CUE_TRANSCRIPT", at, format!("no transcript for `{asset}`"))
                        .hint("set the asset's `transcript` (Scribe or cutline JSON), run `cutline transcribe`, or install a whisper model"),
                );
                None
            }
            Err(e) => {
                d.push(Diagnostic::error("E_CUE_TRANSCRIPT", at, format!("{e:#}")));
                None
            }
        }
    }

    fn cue(&mut self, name: &str, cue: &Cue, d: &mut Vec<Diagnostic>) -> Option<ResolvedCue> {
        let at = format!("/cues/{}", ptr_token(name));
        let asset = match cue.asset() {
            Some(a) => a.to_string(),
            None if self.voice_assets.len() == 1 => self.voice_assets[0].clone(),
            None => {
                d.push(
                    Diagnostic::error(
                        "E_CUE_ASSET",
                        at,
                        if self.voice_assets.is_empty() {
                            "no voice-role audio layer to search; set the cue's `asset`".to_string()
                        } else {
                            format!("several voice tracks ({}); set the cue's `asset`", self.voice_assets.join(", "))
                        },
                    )
                    .hint("cues: {name: {phrase: \"…\", asset: \"vo\"}}"),
                );
                return None;
            }
        };
        let layer = match self.layers.get(&asset).map(|v| v.as_slice()) {
            Some([one]) => one,
            Some([]) | None => {
                d.push(Diagnostic::error(
                    "E_CUE_ASSET",
                    at,
                    format!("asset `{asset}` is not played by an audio layer, so its words have no timeline time"),
                ));
                return None;
            }
            Some(_) => {
                d.push(Diagnostic::error(
                    "E_CUE_ASSET",
                    at,
                    format!("asset `{asset}` is played by several audio layers; cues need exactly one"),
                ));
                return None;
            }
        };
        if let Some((anchor, offset)) = &layer.anchor {
            let av = AnchoredVoice { layer: layer.index, anchor: anchor.clone(), offset: *offset };
            if !self.used_anchors.contains(&av) {
                self.used_anchors.push(av);
            }
        }
        let (start, src_in) = match (&layer.start, &layer.src_in) {
            (Ok(s), Ok(i)) => (*s, *i),
            (Err(e), _) | (_, Err(e)) => {
                d.push(
                    Diagnostic::error(
                        "E_CUE_LAYER_TIME",
                        format!("/layers/{}", layer.index),
                        format!("the `{asset}` layer's start/src_in {e}; cues need plain seconds there"),
                    )
                    .hint(
                        "give the voice layer a numeric `start` or an `anchor` clip (cue times are measured from it)",
                    ),
                );
                return None;
            }
        };
        if !self.transcripts.contains_key(&asset) {
            // each asset's transcript problem is reported once, however many cues use it
            let t = self.load(&asset, &at, d);
            self.transcripts.insert(asset.clone(), t);
        }
        let t = self.transcripts[&asset].as_ref()?;
        let m = match cutline_transcribe::text::find_phrase(&t.words, cue.phrase(), cue.occurrence()) {
            Ok(m) => m,
            Err(nf) => {
                let msg = if nf.found > 0 {
                    format!(
                        "\"{}\" occurs {} time(s) in `{asset}`, so occurrence {} does not exist",
                        cue.phrase(),
                        nf.found,
                        cue.occurrence()
                    )
                } else {
                    format!("\"{}\" was not found in the `{asset}` transcript", cue.phrase())
                };
                let mut diag = Diagnostic::error("E_CUE_NOT_FOUND", at, msg);
                if !nf.nearest.is_empty() {
                    diag = diag.hint(format!("closest: {}", nf.nearest.join("; ")));
                }
                d.push(diag);
                return None;
            }
        };
        let matched = t.words[m.first..=m.last].iter().map(|w| w.w.as_str()).collect::<Vec<_>>().join(" ");
        if !m.exact() {
            d.push(Diagnostic::warning(
                "W_CUE_FUZZY",
                at.clone(),
                format!("cue `{name}` matched \"{matched}\" for \"{}\" (not word for word)", cue.phrase()),
            ));
        }
        let src = if cue.edge() == CueEdge::End { m.end } else { m.start };
        if src < src_in {
            d.push(Diagnostic::error(
                "E_CUE_NOT_FOUND",
                at,
                format!("\"{}\" is at {src:.2}s in `{asset}`, before the layer's src_in {src_in:.2}s", cue.phrase()),
            ));
            return None;
        }
        Some(ResolvedCue {
            name: name.to_string(),
            phrase: cue.phrase().to_string(),
            asset,
            at: start + src - src_in,
            matched,
            source_start: m.start,
            source_end: m.end,
            exact: m.exact(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_ref_offsets_and_dashed_names() {
        let mut cues = BTreeMap::new();
        cues.insert("clocks".to_string(), Cue::Phrase("World clocks".into()));
        cues.insert("end-card".to_string(), Cue::Phrase("Install it".into()));
        assert_eq!(split_ref("cue:clocks", &cues), Some(("clocks".into(), 0.0)));
        assert_eq!(split_ref("cue:clocks+0.4", &cues), Some(("clocks".into(), 0.4)));
        assert_eq!(split_ref("cue:clocks-0.25", &cues), Some(("clocks".into(), -0.25)));
        assert_eq!(split_ref("cue:end-card", &cues), Some(("end-card".into(), 0.0)));
        assert_eq!(split_ref("cue:end-card-1", &cues), Some(("end-card".into(), -1.0)));
        assert_eq!(split_ref("cue:nope", &cues), None);
    }

    #[test]
    fn only_timeline_fields_take_cues() {
        assert!(is_timeline_field("/main/3/end_at"));
        assert!(is_timeline_field("/layers/0/start"));
        assert!(is_timeline_field("/markers/2/at"));
        assert!(!is_timeline_field("/main/3/in"));
        assert!(!is_timeline_field("/layers/0/src_in"));
    }
}
