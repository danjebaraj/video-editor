//! Variants and locales: several videos from one project.
//!
//! * A **variant** is a cut of the edit: it keeps some clips and layers (`include`/`exclude` on
//!   the variant, `only`/`except` on clips and layers), may swap assets (a shorter voice-over)
//!   and limits the exports.
//! * A **locale** is a language: it swaps assets (voice-over, script), cue phrases and the
//!   strings that text layers refer to as `"@key"`.
//!
//! Both are applied to the raw project before anything else reads it, so cues, steps, timing
//! and rendering see an ordinary project. Because cuts are timed by cues and `end_at`, a
//! shorter script or another language re-times the edit by itself.

use cutline_core::diag::{ptr_token, Diagnostic};
use serde_json::{json, Value};
use std::path::Path;

/// Which variant and locale of a project to work on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub variant: Option<String>,
    pub locale: Option<String>,
}

static CURRENT: std::sync::RwLock<Option<Selection>> = std::sync::RwLock::new(None);

/// Selection used by `pipeline::load` (set once by the CLI from `--variant` / `--locale`).
pub fn current() -> Selection {
    CURRENT.read().ok().and_then(|g| g.clone()).unwrap_or_default()
}

pub fn set_current(s: Selection) {
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(s);
    }
}

pub fn names(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(|x| x.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Whether an item with `only` / `except` lists belongs to `variant`.
fn belongs(item: &Value, variant: Option<&str>) -> bool {
    let (only, except) = (str_list(item.get("only")), str_list(item.get("except")));
    match variant {
        Some(v) => (only.is_empty() || only.iter().any(|x| x == v)) && !except.iter().any(|x| x == v),
        // without variants in play, items reserved for one are left out
        None => only.is_empty(),
    }
}

fn merge_assets(v: &mut Value, over: Option<&Value>, at: &str, d: &mut Vec<Diagnostic>) {
    let Some(over) = over.and_then(|o| o.as_object()) else { return };
    for (id, fields) in over {
        match v.get_mut("assets").and_then(|a| a.get_mut(id)).and_then(|a| a.as_object_mut()) {
            Some(asset) => {
                for (k, val) in fields.as_object().into_iter().flatten() {
                    asset.insert(k.clone(), val.clone());
                }
            }
            None => d.push(
                Diagnostic::error(
                    "E_UNKNOWN_ASSET",
                    format!("{at}/assets/{}", ptr_token(id)),
                    format!("unknown asset `{id}`"),
                )
                .hint("an override changes fields of an asset defined in `assets`"),
            ),
        }
    }
}

/// Replace `"@key"` strings (in `text`, `subtitle`, `label`, `title`, `url` and inside
/// `params`) with the string table's entries. `"@@…"` is a literal `@…`.
fn localize(v: &mut Value, table: &serde_json::Map<String, Value>, ptr: &str, d: &mut Vec<Diagnostic>) {
    match v {
        Value::String(s) => {
            if let Some(rest) = s.strip_prefix("@@") {
                *s = format!("@{rest}");
            } else if let Some(key) = s.strip_prefix('@').filter(|k| !k.is_empty() && !k.contains(char::is_whitespace))
            {
                match table.get(key).and_then(|x| x.as_str()) {
                    Some(t) => *s = t.to_string(),
                    None => d.push(
                        Diagnostic::error("E_UNKNOWN_STRING", ptr, format!("no string `{key}` for this locale")).hint(
                            "add it to `strings` (and to each locale's strings file); write \"@@…\" for a literal @",
                        ),
                    ),
                }
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter_mut().enumerate() {
                localize(x, table, &format!("{ptr}/{i}"), d);
            }
        }
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                localize(x, table, &format!("{ptr}/{}", ptr_token(k)), d);
            }
        }
        _ => {}
    }
}

/// Apply the selection to the raw project. With several variants and none selected, the first
/// by name is used. Returns diagnostics and the variant actually applied.
pub fn apply(v: &mut Value, dir: &Path, stem: &str, sel: &Selection) -> (Vec<Diagnostic>, Selection) {
    let mut d = vec![];
    let mut used = Selection::default();
    let base_name = v.get("name").and_then(|n| n.as_str()).unwrap_or(stem).to_string();
    let mut suffix = String::new();

    // ---- variant
    let all = names(v, "variants");
    let variant = match &sel.variant {
        Some(n) if all.contains(n) => Some(n.clone()),
        Some(n) => {
            d.push(
                Diagnostic::error("E_NO_SUCH_VARIANT", "/variants", format!("the project has no variant `{n}`")).hint(
                    if all.is_empty() {
                        "define `variants`".to_string()
                    } else {
                        format!("variants: {}", all.join(", "))
                    },
                ),
            );
            None
        }
        None => all.first().cloned(),
    };
    let spec = variant.as_ref().and_then(|n| v["variants"].get(n)).cloned().unwrap_or(json!({}));
    let (include, exclude) = (str_list(spec.get("include")), str_list(spec.get("exclude")));
    let clip_ids: Vec<String> = v
        .get("main")
        .and_then(|m| m.as_array())
        .map(|m| m.iter().filter_map(|i| i.get("id").and_then(|x| x.as_str()).map(str::to_string)).collect())
        .unwrap_or_default();
    for (list, key) in [(&include, "include"), (&exclude, "exclude")] {
        for (i, id) in list.iter().enumerate() {
            if !clip_ids.contains(id) {
                d.push(Diagnostic::error(
                    "E_UNKNOWN_CLIP",
                    format!("/variants/{}/{key}/{i}", ptr_token(variant.as_deref().unwrap_or(""))),
                    format!("no clip with id `{id}`"),
                ));
            }
        }
    }
    let vname = variant.as_deref();
    let mut dropped: Vec<String> = vec![];
    if let Some(main) = v.get_mut("main").and_then(|m| m.as_array_mut()) {
        // keep clips that belong; a transition survives only between two kept neighbours
        let keep: Vec<bool> = main
            .iter()
            .map(|it| match it.get("id").and_then(|x| x.as_str()) {
                Some(id) => {
                    let k = belongs(it, vname)
                        && (include.is_empty() || include.iter().any(|x| x == id))
                        && !exclude.iter().any(|x| x == id);
                    if !k {
                        dropped.push(id.to_string());
                    }
                    k
                }
                None => belongs(it, vname),
            })
            .collect();
        let is_clip: Vec<bool> = main.iter().map(|it| it.get("transition").is_none()).collect();
        let mut out = vec![];
        for i in 0..main.len() {
            if !keep[i] {
                continue;
            }
            if !is_clip[i] {
                let prev_ok = i > 0 && is_clip[i - 1] && keep[i - 1];
                let next_ok = i + 1 < main.len() && is_clip[i + 1] && keep[i + 1];
                if !(prev_ok && next_ok) {
                    continue;
                }
            }
            out.push(main[i].clone());
        }
        *main = out;
    }
    if let Some(layers) = v.get_mut("layers").and_then(|l| l.as_array_mut()) {
        layers.retain(|l| {
            belongs(l, vname)
                && !l.get("anchor").and_then(|a| a.as_str()).is_some_and(|a| dropped.iter().any(|x| x == a))
        });
    }
    if let Some(n) = &variant {
        merge_assets(v, spec.get("assets"), &format!("/variants/{}", ptr_token(n)), &mut d);
        let want = str_list(spec.get("exports"));
        if !want.is_empty() {
            if let Some(ex) = v.get_mut("exports").and_then(|e| e.as_array_mut()) {
                let known: Vec<String> =
                    ex.iter().filter_map(|e| e.get("name").and_then(|x| x.as_str()).map(str::to_string)).collect();
                for (i, w) in want.iter().enumerate() {
                    if !known.contains(w) {
                        d.push(Diagnostic::error(
                            "E_BAD_VALUE",
                            format!("/variants/{}/exports/{i}", ptr_token(n)),
                            format!("no export named `{w}`"),
                        ));
                    }
                }
                ex.retain(|e| e.get("name").and_then(|x| x.as_str()).is_some_and(|x| want.iter().any(|w| w == x)));
            }
        }
        suffix.push_str(&format!("-{n}"));
        used.variant = Some(n.clone());
    }

    // ---- locale
    let locales = names(v, "locales");
    let mut table = v.get("strings").and_then(|s| s.as_object()).cloned().unwrap_or_default();
    if let Some(n) = &sel.locale {
        match v.get("locales").and_then(|l| l.get(n)).cloned() {
            Some(spec) => {
                let at = format!("/locales/{}", ptr_token(n));
                merge_assets(v, spec.get("assets"), &at, &mut d);
                if let Some(cues) = spec.get("cues").and_then(|c| c.as_object()) {
                    for (k, phrase) in cues {
                        match v.get_mut("cues").and_then(|c| c.as_object_mut()) {
                            Some(c) if c.contains_key(k) => {
                                c.insert(k.clone(), phrase.clone());
                            }
                            _ => d.push(Diagnostic::error(
                                "E_UNKNOWN_CUE",
                                format!("{at}/cues/{}", ptr_token(k)),
                                format!("the project defines no cue `{k}`"),
                            )),
                        }
                    }
                }
                match spec.get("strings") {
                    Some(Value::String(file)) => {
                        let p = dir.join(file);
                        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| {
                            cutline_core::parse::parse_json5(&t)
                                .map_err(|e| e.iter().map(|x| x.message.clone()).collect::<Vec<_>>().join("; "))
                        }) {
                            Ok(Value::Object(o)) => table.extend(o),
                            Ok(_) => d.push(Diagnostic::error(
                                "E_BAD_VALUE",
                                format!("{at}/strings"),
                                "the strings file must be an object of key: text",
                            )),
                            Err(e) => d.push(Diagnostic::error(
                                "E_ASSET_NOT_FOUND",
                                format!("{at}/strings"),
                                format!("strings file {file}: {e}"),
                            )),
                        }
                    }
                    Some(Value::Object(o)) => table.extend(o.clone()),
                    _ => {}
                }
                if let Some(lang) = spec.get("lang").and_then(|x| x.as_str()) {
                    if let Some(c) = v.get_mut("captions").and_then(|c| c.as_object_mut()) {
                        c.insert("lang".into(), json!(lang));
                    }
                }
                suffix.push_str(&format!("-{n}"));
                used.locale = Some(n.clone());
            }
            None => d.push(
                Diagnostic::error("E_NO_SUCH_LOCALE", "/locales", format!("the project has no locale `{n}`")).hint(
                    if locales.is_empty() {
                        "define `locales`".to_string()
                    } else {
                        format!("locales: {}", locales.join(", "))
                    },
                ),
            ),
        }
    }
    // text that refers to the string table
    if let Some(layers) = v.get_mut("layers").and_then(|l| l.as_array_mut()) {
        for (i, l) in layers.iter_mut().enumerate() {
            for field in ["text", "subtitle", "label", "params", "keys"] {
                if let Some(x) = l.get_mut(field) {
                    localize(x, &table, &format!("/layers/{i}/{field}"), &mut d);
                }
            }
        }
    }
    if let Some(main) = v.get_mut("main").and_then(|m| m.as_array_mut()) {
        for (i, c) in main.iter_mut().enumerate() {
            for field in ["chapter", "frame"] {
                if let Some(x) = c.get_mut(field) {
                    localize(x, &table, &format!("/main/{i}/{field}"), &mut d);
                }
            }
        }
    }
    if let Some(m) = v.get_mut("markers") {
        localize(m, &table, "/markers", &mut d);
    }
    if !suffix.is_empty() {
        v["name"] = json!(format!("{base_name}{suffix}"));
    }
    (d, used)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Value {
        json!({
            "name": "demo",
            "assets": {"vo": {"path": "vo.mp3", "script": "vo.txt"}, "s": {"path": "s.mp4"}},
            "cues": {"export": "the export button"},
            "strings": {"title": "World clocks", "cta": "Try it free"},
            "main": [
                {"id": "title", "clip": "s"}, {"transition": {"type": "fade", "duration": 0.4}},
                {"id": "tour", "clip": "s", "except": ["teaser"]}, {"transition": {"type": "fade", "duration": 0.4}},
                {"id": "export", "clip": "s"},
                {"id": "deep", "clip": "s", "only": ["full"]},
                {"id": "cta", "clip": "s"},
            ],
            "layers": [
                {"type": "text", "template": "title_card", "text": "@title", "anchor": "title"},
                {"type": "text", "template": "callout", "text": "@@handle", "anchor": "tour"},
                {"type": "text", "template": "cta_link", "text": "@cta", "params": {"url": "example.com", "items": ["@title"]}, "only": ["teaser"]},
            ],
            "exports": [{"name": "yt"}, {"name": "linkedin"}],
            "variants": {"full": {}, "teaser": {"exclude": [], "exports": ["linkedin"], "assets": {"vo": {"path": "vo.teaser.mp3"}}}},
            "locales": {"de": {"assets": {"vo": {"path": "vo.de.mp3", "script": "vo.de.txt"}}, "cues": {"export": "die Export-Schaltfläche"},
                               "strings": {"title": "Weltuhren"}, "lang": "de"}},
            "captions": {"source": "auto"},
        })
    }

    fn ids(v: &Value) -> Vec<String> {
        v["main"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.get("id").and_then(|x| x.as_str()).unwrap_or("~").to_string())
            .collect()
    }

    #[test]
    fn a_variant_keeps_its_clips_layers_assets_and_exports() {
        let mut v = project();
        let sel = Selection { variant: Some("teaser".into()), locale: None };
        let (d, used) = apply(&mut v, Path::new("."), "p", &sel);
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(used.variant.as_deref(), Some("teaser"));
        // `tour` and `deep` are gone, and with `tour` both transitions around it
        assert_eq!(ids(&v), vec!["title", "export", "cta"]);
        // the layer anchored to `tour` went with it; the teaser-only layer stays
        let texts: Vec<&str> = v["layers"].as_array().unwrap().iter().map(|l| l["text"].as_str().unwrap()).collect();
        assert_eq!(texts, vec!["World clocks", "Try it free"]);
        assert_eq!(v["layers"][1]["params"]["items"][0], json!("World clocks"));
        assert_eq!(v["assets"]["vo"], json!({"path": "vo.teaser.mp3", "script": "vo.txt"}));
        assert_eq!(v["exports"], json!([{"name": "linkedin"}]));
        assert_eq!(v["name"], json!("demo-teaser"));
    }

    #[test]
    fn the_first_variant_is_the_default_and_a_locale_swaps_voice_cues_and_strings() {
        let mut v = project();
        let sel = Selection { variant: None, locale: Some("de".into()) };
        let (d, used) = apply(&mut v, Path::new("."), "p", &sel);
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(used, Selection { variant: Some("full".into()), locale: Some("de".into()) });
        assert_eq!(ids(&v), vec!["title", "~", "tour", "~", "export", "deep", "cta"]);
        assert_eq!(v["assets"]["vo"], json!({"path": "vo.de.mp3", "script": "vo.de.txt"}));
        assert_eq!(v["cues"]["export"], json!("die Export-Schaltfläche"));
        // the locale's strings win, the base table fills the rest; "@@" is a literal "@"
        assert_eq!(v["layers"][0]["text"], json!("Weltuhren"));
        assert_eq!(v["layers"][1]["text"], json!("@handle"));
        assert_eq!(v["captions"]["lang"], json!("de"));
        assert_eq!(v["name"], json!("demo-full-de"));
    }

    #[test]
    fn unknown_names_and_strings_are_errors() {
        let mut v = project();
        v["layers"][0]["text"] = json!("@missing");
        v["variants"]["teaser"]["include"] = json!(["title", "nope"]);
        v["variants"]["teaser"]["exports"] = json!(["tiktok"]);
        let sel = Selection { variant: Some("teaser".into()), locale: Some("fr".into()) };
        let (d, _) = apply(&mut v, Path::new("."), "p", &sel);
        let has = |code: &str, path: &str| d.iter().any(|x| x.code == code && x.path == path);
        assert!(has("E_UNKNOWN_CLIP", "/variants/teaser/include/1"), "{d:?}");
        assert!(has("E_BAD_VALUE", "/variants/teaser/exports/0"), "{d:?}");
        assert!(has("E_NO_SUCH_LOCALE", "/locales"), "{d:?}");
        assert!(has("E_UNKNOWN_STRING", "/layers/0/text"), "{d:?}");
        let (d, _) = apply(&mut project(), Path::new("."), "p", &Selection { variant: Some("x".into()), locale: None });
        assert!(d.iter().any(|x| x.code == "E_NO_SUCH_VARIANT"), "{d:?}");
    }
}
