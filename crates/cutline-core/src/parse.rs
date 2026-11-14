//! JSON5 parsing with JSON-pointer error paths.

use crate::diag::{ptr_token, Diagnostic};
use crate::model::{Clip, Layer, Project, TransitionItem};
use serde::de::DeserializeOwned;
use serde_json::Value;

fn path_to_pointer(p: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;
    let mut s = String::new();
    for seg in p.iter() {
        match seg {
            Segment::Seq { index } => s.push_str(&format!("/{index}")),
            Segment::Map { key } => s.push_str(&format!("/{}", ptr_token(key))),
            Segment::Enum { variant } => s.push_str(&format!("/{}", ptr_token(variant))),
            Segment::Unknown => {}
        }
    }
    s
}

fn classify(msg: &str) -> &'static str {
    if msg.contains("unknown field") {
        "E_UNKNOWN_FIELD"
    } else if msg.contains("unknown variant") {
        "E_UNKNOWN_VARIANT"
    } else if msg.contains("missing field") {
        "E_MISSING_FIELD"
    } else if msg.contains("time") || msg.contains("timecode") || msg.contains("frame count") {
        "E_BAD_TIME"
    } else if msg.contains("fps") {
        "E_BAD_FPS"
    } else {
        "E_SCHEMA"
    }
}

fn de_at<T: DeserializeOwned>(v: &Value, base: &str, out: &mut Vec<Diagnostic>) -> Option<T> {
    match serde_path_to_error::deserialize::<_, T>(v.clone()) {
        Ok(t) => Some(t),
        Err(e) => {
            let inner = path_to_pointer(e.path());
            let msg = e.inner().to_string();
            // the custom field-unknown error ends with "field `x`" — append it to the path
            let mut path = format!("{base}{inner}");
            if let Some(f) = msg.strip_prefix("unknown field `").and_then(|r| r.split('`').next()) {
                if !path.ends_with(&format!("/{f}")) {
                    path = format!("{path}/{}", ptr_token(f));
                }
            }
            out.push(Diagnostic::error(classify(&msg), path, msg));
            None
        }
    }
}

/// Property names allowed for a layer `type`, derived from the JSON schema.
fn layer_allowed_keys(kind: &str) -> Option<Vec<String>> {
    let schema = serde_json::to_value(schemars::schema_for!(Layer)).ok()?;
    let variants = schema.get("oneOf")?.as_array()?;
    for v in variants {
        let props = v.get("properties")?.as_object()?;
        let ty = props.get("type")?;
        let matches = ty.get("const").and_then(|c| c.as_str()) == Some(kind)
            || ty.get("enum").and_then(|e| e.as_array()).map(|a| a.iter().any(|x| x == kind)).unwrap_or(false);
        if matches {
            return Some(props.keys().cloned().collect());
        }
    }
    None
}

/// Clip-relative effect time fields and their source-relative twins.
const EFFECT_TIME_TWINS: [(&str, &str); 3] = [("at", "src_at"), ("start", "src_start"), ("end", "src_end")];

/// In a serialized project (`cutline expand`), drop the clip-relative defaults (`start: 0`,
/// `at: 0`) that serialization writes beside an effect's source-relative time.
pub fn strip_shadowed_effect_times(project: &mut Value) {
    let Some(main) = project.get_mut("main").and_then(|m| m.as_array_mut()) else { return };
    for effect in main.iter_mut().filter_map(|c| c.get_mut("effects")).filter_map(|e| e.as_array_mut()).flatten() {
        let Some(o) = effect.as_object_mut() else { continue };
        for (plain, src) in EFFECT_TIME_TWINS {
            if o.get(src).is_some_and(|v| !v.is_null()) && o.get(plain).is_some_and(|v| v.as_f64() == Some(0.0)) {
                o.remove(plain);
            }
        }
    }
}

/// An effect's time is written clip-relative (`at`, `start`, `end`) or source-relative
/// (`src_at`, `src_start`, `src_end`), not both; `punch_in` and the other `at` effects need one.
fn check_effect_times(clip: &Value, base: &str, errs: &mut Vec<Diagnostic>) {
    let Some(effects) = clip.get("effects").and_then(|e| e.as_array()) else { return };
    for (i, e) in effects.iter().enumerate() {
        let Some(o) = e.as_object() else { continue };
        let kind = o.get("type").and_then(|t| t.as_str()).unwrap_or("");
        for (plain, src) in EFFECT_TIME_TWINS {
            // a clip-relative 0 beside a source time is the default written out (a serialized
            // project), not a second time
            let set = o.get(plain).is_some_and(|v| v.as_f64() != Some(0.0) && !v.is_null());
            if set && o.get(src).is_some_and(|v| !v.is_null()) {
                errs.push(
                    Diagnostic::error(
                        "E_FIELD_CONFLICT",
                        format!("{base}/effects/{i}/{src}"),
                        format!("`{src}` (seconds in the asset) and `{plain}` (seconds from the clip's start) both set the same time"),
                    )
                    .hint(format!("keep one: `{src}` follows the footage when the clip is retimed, `{plain}` stays put on the timeline")),
                );
            }
        }
        if matches!(kind, "punch_in" | "shake" | "flash" | "glitch" | "motion_blur")
            && !o.contains_key("at")
            && !o.contains_key("src_at")
            && !o.contains_key("step")
        {
            errs.push(
                Diagnostic::error(
                    "E_MISSING_FIELD",
                    format!("{base}/effects/{i}/at"),
                    format!("`{kind}` needs `at` or `src_at`"),
                )
                .hint("`at`: seconds from the clip's start; `src_at`: seconds in the asset"),
            );
        }
    }
}

/// Parse project text (JSON5 or JSON). Returns all structural errors found.
pub fn parse_project(text: &str) -> Result<Project, Vec<Diagnostic>> {
    parse_value(&parse_json5(text)?)
}

/// JSON5 text → JSON value (syntax errors as diagnostics).
pub fn parse_json5(text: &str) -> Result<Value, Vec<Diagnostic>> {
    json5::from_str(text).map_err(|e| {
        vec![Diagnostic::error("E_PARSE", "", format!("JSON5 syntax error: {e}"))
            .hint("check for missing commas, unbalanced braces or unquoted special characters")]
    })
}

pub fn parse_value(v: &Value) -> Result<Project, Vec<Diagnostic>> {
    let mut errs = vec![];
    let Some(obj) = v.as_object() else {
        return Err(vec![Diagnostic::error("E_SCHEMA", "", "project must be an object")]);
    };
    match obj.get("version") {
        Some(Value::Number(n)) if n.as_u64() == Some(1) => {}
        Some(other) => errs.push(
            Diagnostic::error("E_UNSUPPORTED_VERSION", "/version", format!("unsupported schema version {other}"))
                .hint("this cutline understands version 1"),
        ),
        None => errs
            .push(Diagnostic::error("E_MISSING_FIELD", "/version", "missing field `version`").hint("add `version: 1`")),
    }
    if let Some(Value::Array(main)) = obj.get("main") {
        for (i, item) in main.iter().enumerate() {
            let base = format!("/main/{i}");
            if item.get("transition").is_some() {
                de_at::<TransitionItem>(item, &base, &mut errs);
            } else {
                check_effect_times(item, &base, &mut errs);
                de_at::<Clip>(item, &base, &mut errs);
            }
        }
    }
    if let Some(Value::Array(layers)) = obj.get("layers") {
        for (i, item) in layers.iter().enumerate() {
            let base = format!("/layers/{i}");
            if let Some(kind) = item.get("type").and_then(|t| t.as_str()) {
                if let (Some(allowed), Some(o)) = (layer_allowed_keys(kind), item.as_object()) {
                    for k in o.keys() {
                        if !allowed.contains(k) {
                            errs.push(
                                Diagnostic::error(
                                    "E_UNKNOWN_FIELD",
                                    format!("{base}/{}", ptr_token(k)),
                                    format!("unknown field `{k}` for layer type `{kind}`"),
                                )
                                .hint(format!("allowed: {}", allowed.join(", "))),
                            );
                        }
                    }
                }
            }
            de_at::<Layer>(item, &base, &mut errs);
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    match de_at::<Project>(v, "", &mut errs) {
        Some(p) => Ok(p),
        None => Err(errs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal() {
        let p = parse_project(
            "{version: 1, assets: {a: {path: 'a.mp4'}}, main: [{id: 'c', clip: 'a', in: 1, out: '00:02.5'},],}",
        )
        .unwrap();
        assert_eq!(p.clips().count(), 1);
    }

    #[test]
    fn pointer_paths() {
        let e = parse_project("{version: 1, main: [{id: 'c', clip: 'a', inn: 1}]}").unwrap_err();
        assert_eq!(e[0].code, "E_UNKNOWN_FIELD");
        assert_eq!(e[0].path, "/main/0/inn");
        let e = parse_project("{version: 1, layers: [{type: 'image', asset: 'x', colour: 'red'}]}").unwrap_err();
        assert_eq!(e[0].path, "/layers/0/colour");
        let e = parse_project("{version: 1, main: [{id: 'c', clip: 'a', in: 'xx'}]}").unwrap_err();
        assert_eq!(e[0].code, "E_BAD_TIME");
        assert_eq!(e[0].path, "/main/0/in");
    }
}
