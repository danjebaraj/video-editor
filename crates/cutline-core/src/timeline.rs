//! Resolve a project into absolute timeline positions (frames) and run semantic validation.

use crate::catalog;
use crate::diag::{ptr_token, Diagnostic};
use crate::model::*;
use crate::target::{self, TargetSpec};
use crate::time::{Fps, TimeValue};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// What validation/resolution needs to know about an asset's media.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MediaFacts {
    pub exists: bool,
    pub kind: AssetKind,
    pub duration: f64,
    pub has_video: bool,
    pub has_audio: bool,
    pub alpha: bool,
    pub width: u32,
    pub height: u32,
    pub fps: Option<Fps>,
    pub rotation: i32,
}

pub trait MediaLookup {
    fn facts(&self, asset_id: &str) -> Option<MediaFacts>;
}

/// Lookup backed by a precomputed map (used by tests and by the CLI after probing).
#[derive(Debug, Clone, Default)]
pub struct StaticLookup(pub BTreeMap<String, MediaFacts>);
impl MediaLookup for StaticLookup {
    fn facts(&self, id: &str) -> Option<MediaFacts> {
        self.0.get(id).cloned()
    }
}

/// Extra knowledge from other crates.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub project_dir: PathBuf,
    /// Bundled font family names (for E_FONT_MISSING).
    pub fonts: Vec<String>,
    /// Brand SFX aliases mapped to asset paths.
    pub sfx_aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TClip {
    /// Index into `project.main`.
    pub main_index: usize,
    /// Index among clips.
    pub index: usize,
    pub id: String,
    pub asset: String,
    pub kind: AssetKind,
    pub src_in: f64,
    pub src_out: f64,
    pub speed: f64,
    /// Timeline start frame.
    pub start: i64,
    /// Timeline duration in frames (including freezes).
    pub frames: i64,
    pub clip: Clip,
}

impl TClip {
    pub fn end(&self) -> i64 {
        self.start + self.frames
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TTransition {
    pub main_index: usize,
    /// Transition between clip `after` and `after + 1`.
    pub after: usize,
    pub frames: i64,
    /// Timeline frame where the overlap starts.
    pub start: i64,
    pub transition: Transition,
}

#[derive(Debug, Clone, Serialize)]
pub struct TLayer {
    pub index: usize,
    pub start: i64,
    pub end: i64,
    pub z: i32,
    pub layer: Layer,
}

#[derive(Debug, Clone, Serialize)]
pub struct Timeline {
    pub fps: Fps,
    pub target: TargetSpec,
    pub total_frames: i64,
    pub clips: Vec<TClip>,
    pub transitions: Vec<TTransition>,
    pub layers: Vec<TLayer>,
}

impl Timeline {
    pub fn duration(&self) -> f64 {
        self.fps.frames_to_secs(self.total_frames)
    }
    pub fn secs(&self, frames: i64) -> f64 {
        self.fps.frames_to_secs(frames)
    }
    pub fn clip_by_id(&self, id: &str) -> Option<&TClip> {
        self.clips.iter().find(|c| c.id == id)
    }
    /// Clip whose span contains timeline frame `f` (the later one during a transition).
    pub fn clip_at(&self, f: i64) -> Option<&TClip> {
        self.clips.iter().rev().find(|c| f >= c.start && f < c.end())
    }
}

pub fn default_kind_from_ext(path: &str) -> AssetKind {
    let p = path.to_ascii_lowercase();
    if p.contains('%') {
        return AssetKind::ImageSequence;
    }
    let ext = Path::new(&p).extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" | "gif" => AssetKind::Image,
        "wav" | "mp3" | "m4a" | "aac" | "flac" | "ogg" | "opus" | "aiff" | "aif" => AssetKind::Audio,
        "html" | "htm" => AssetKind::Html,
        "svg" => AssetKind::Svg,
        "json" | "lottie" => AssetKind::Lottie,
        _ => AssetKind::Video,
    }
}

fn is_visual_still(k: AssetKind) -> bool {
    matches!(k, AssetKind::Image | AssetKind::Html | AssetKind::Lottie | AssetKind::Svg)
}

/// Integrate timeline duration of a source span under a speed ramp (piecewise linear speed).
pub fn ramp_duration(src_len: f64, keys: &[SpeedKey]) -> f64 {
    if keys.is_empty() {
        return src_len;
    }
    let speed_at = |s: f64| -> f64 {
        if s <= keys[0].t {
            return keys[0].speed;
        }
        for w in keys.windows(2) {
            if s >= w[0].t && s <= w[1].t {
                let u = if w[1].t > w[0].t { (s - w[0].t) / (w[1].t - w[0].t) } else { 0.0 };
                return w[0].speed + (w[1].speed - w[0].speed) * u;
            }
        }
        keys.last().unwrap().speed
    };
    let n = 2000;
    let ds = src_len / n as f64;
    (0..n).map(|i| ds / speed_at((i as f64 + 0.5) * ds).max(0.01)).sum()
}

/// Resolve + validate. Returns the timeline (if resolvable) and all diagnostics.
pub fn resolve(p: &Project, look: &dyn MediaLookup, ctx: &Context) -> (Option<Timeline>, Vec<Diagnostic>) {
    let mut d: Vec<Diagnostic> = vec![];

    // ---- assets
    let mut facts: BTreeMap<String, MediaFacts> = BTreeMap::new();
    for (id, a) in &p.assets {
        let base = format!("/assets/{}", ptr_token(id));
        match look.facts(id) {
            Some(f) if !f.exists => d.push(
                Diagnostic::error(
                    "E_ASSET_NOT_FOUND",
                    format!("{base}/path"),
                    format!("asset file not found: {}", a.path),
                )
                .hint("paths are relative to the project file"),
            ),
            Some(f) => {
                if a.alpha == Some(true) && !f.alpha {
                    d.push(
                        Diagnostic::error(
                            "E_ALPHA_NOT_PRESENT",
                            format!("{base}/alpha"),
                            format!("asset `{id}` declares alpha but the media has no alpha channel"),
                        )
                        .hint("export ProRes 4444, WebM VP9 with alpha, or PNG; or remove `alpha: true`"),
                    );
                }
                if let Some(k) = a.kind {
                    let ok = match k {
                        AssetKind::Audio => f.has_audio,
                        AssetKind::Video | AssetKind::Image | AssetKind::ImageSequence => f.has_video,
                        _ => true,
                    };
                    if !ok {
                        d.push(Diagnostic::error(
                            "E_WRONG_ASSET_KIND",
                            format!("{base}/kind"),
                            format!("asset `{id}` is declared {k:?} but the media does not match"),
                        ));
                    }
                }
                facts.insert(id.clone(), f);
            }
            None => {}
        }
        for (field, file) in [("transcript", &a.transcript), ("script", &a.script)] {
            if let Some(t) = file {
                if !ctx.project_dir.join(t).exists() {
                    d.push(Diagnostic::error(
                        "E_ASSET_NOT_FOUND",
                        format!("{base}/{field}"),
                        format!("{field} file not found: {t}"),
                    ));
                }
            }
        }
    }

    // ---- target
    if p.target == TargetName::Custom && (p.width.is_none() || p.height.is_none()) {
        d.push(Diagnostic::error("E_CUSTOM_SIZE", "/target", "target `custom` needs `width` and `height`"));
    }
    for (k, v) in [("width", p.width), ("height", p.height)] {
        if let Some(v) = v {
            if !(16..=8192).contains(&v) || v % 2 != 0 {
                d.push(Diagnostic::error(
                    "E_BAD_VALUE",
                    format!("/{k}"),
                    format!("{k} must be an even number between 16 and 8192"),
                ));
            }
        }
    }
    let first_src_fps =
        p.clips().find_map(|c| facts.get(&c.clip).filter(|f| f.kind == AssetKind::Video).and_then(|f| f.fps));
    let mut spec = target::resolve(p.target, p.fps, first_src_fps, p.width, p.height);
    spec.screencast = p.profile == Some(Profile::Screencast);
    spec.chroma444 = p.chroma == Some(Chroma::C444);
    let fps = spec.fps;

    // ---- main
    let clip_count = p.clips().count();
    if clip_count == 0 {
        d.push(
            Diagnostic::error("E_EMPTY_MAIN", "/main", "the main storyline has no clips")
                .hint("add at least one clip to `main`"),
        );
    }
    let mut ids = BTreeSet::new();
    let mut clips: Vec<TClip> = vec![];
    let mut transitions: Vec<TTransition> = vec![];
    let mut pending_trans: Option<(usize, Transition)> = None;
    let mut ok = true;
    for (mi, item) in p.main.iter().enumerate() {
        let base = format!("/main/{mi}");
        match item {
            MainItem::Transition(t) => {
                let t = &t.transition;
                if !catalog::is_transition(&t.kind) {
                    d.push(
                        Diagnostic::error(
                            "E_UNKNOWN_TRANSITION",
                            format!("{base}/transition/type"),
                            format!("unknown transition `{}`", t.kind),
                        )
                        .hint("run `cutline templates list` for transition names"),
                    );
                }
                if t.duration.is_nan() || t.duration < 0.0 || t.duration > 5.0 {
                    d.push(Diagnostic::error(
                        "E_BAD_VALUE",
                        format!("{base}/transition/duration"),
                        "transition duration must be 0–5 s",
                    ));
                }
                if clips.is_empty() || pending_trans.is_some() {
                    d.push(Diagnostic::error(
                        "E_TRANSITION_POSITION",
                        base.clone(),
                        "a transition must sit between two clips",
                    ));
                    ok = false;
                } else {
                    pending_trans = Some((mi, t.clone()));
                }
                if let Some(s) = &t.sfx {
                    check_sfx(s, p, ctx, &format!("{base}/transition/sfx"), &mut d);
                }
            }
            MainItem::Clip(c) => {
                if !ids.insert(c.id.clone()) {
                    d.push(Diagnostic::error(
                        "E_DUPLICATE_ID",
                        format!("{base}/id"),
                        format!("duplicate clip id `{}`", c.id),
                    ));
                }
                let Some(asset) = p.assets.get(&c.clip) else {
                    d.push(
                        Diagnostic::error(
                            "E_UNKNOWN_ASSET",
                            format!("{base}/clip"),
                            format!("unknown asset `{}`", c.clip),
                        )
                        .hint(format!("defined assets: {}", p.assets.keys().cloned().collect::<Vec<_>>().join(", "))),
                    );
                    ok = false;
                    continue;
                };
                let f = facts.get(&c.clip);
                let kind = asset.kind.or(f.map(|f| f.kind)).unwrap_or_else(|| default_kind_from_ext(&asset.path));
                if kind == AssetKind::Audio {
                    d.push(
                        Diagnostic::error(
                            "E_WRONG_ASSET_KIND",
                            format!("{base}/clip"),
                            format!("asset `{}` is audio-only and cannot be a main clip", c.clip),
                        )
                        .hint("use an `audio` layer"),
                    );
                    ok = false;
                    continue;
                }
                let mut speed = c.speed.unwrap_or(1.0);
                if !(0.25..=4.0).contains(&speed) {
                    d.push(Diagnostic::error(
                        "E_SPEED_RANGE",
                        format!("{base}/speed"),
                        format!("speed {speed} outside 0.25–4"),
                    ));
                }
                for (k, sk) in c.speed_ramp.iter().enumerate() {
                    if !(0.1..=8.0).contains(&sk.speed) {
                        d.push(Diagnostic::error(
                            "E_SPEED_RANGE",
                            format!("{base}/speed_ramp/{k}/speed"),
                            "ramp speed outside 0.1–8",
                        ));
                    }
                }
                let src_fps = f.and_then(|f| f.fps).unwrap_or(fps);
                let media_dur = f.map(|f| f.duration).unwrap_or(0.0);
                let freeze_frames: i64 = c
                    .effects
                    .iter()
                    .map(|e| match e {
                        Effect::Freeze { duration, .. } => fps.secs_to_frames(*duration),
                        _ => 0,
                    })
                    .sum();
                // `end_at`: content frames that make the cut to the next clip land on it.
                let fit_frames = match c.end_at {
                    None => None,
                    Some(e) => {
                        let at = format!("{base}/end_at");
                        if e.is_end() {
                            d.push(Diagnostic::error("E_BAD_TIME", at, "end_at must be a time, not `end`"));
                            ok = false;
                            continue;
                        }
                        if c.duration.is_some() || !c.speed_ramp.is_empty() {
                            d.push(
                                Diagnostic::error(
                                    "E_FIELD_CONFLICT",
                                    at,
                                    "end_at sets the clip length, so it cannot be combined with `duration` or `speed_ramp`",
                                )
                                .hint("remove `duration`/`speed_ramp`, or remove `end_at`"),
                            );
                            ok = false;
                            continue;
                        }
                        let prev_end = clips.last().map(|l: &TClip| l.end()).unwrap_or(0);
                        let tf_in = pending_trans
                            .as_ref()
                            .map(|(_, t)| if t.kind == "cut" { 0 } else { fps.secs_to_frames(t.duration) })
                            .unwrap_or(0);
                        let tf_out = match p.main.get(mi + 1) {
                            Some(MainItem::Transition(t)) if t.transition.kind != "cut" => {
                                fps.secs_to_frames(t.transition.duration)
                            }
                            _ => 0,
                        };
                        let start = prev_end - tf_in;
                        let content = e.frames(fps, 0) + tf_out / 2 - start - freeze_frames;
                        if content < 1 {
                            d.push(
                                Diagnostic::error(
                                    "E_END_AT_BEFORE_START",
                                    at,
                                    format!(
                                        "end_at {:.3}s is not after this clip's start ({:.3}s)",
                                        e.seconds(fps, 0.0),
                                        fps.frames_to_secs(start)
                                    ),
                                )
                                .hint("cues and end_at times must increase along `main`"),
                            );
                            ok = false;
                            continue;
                        }
                        Some(content)
                    }
                };
                let (src_in, src_out, tl_secs);
                if is_visual_still(kind) {
                    let dur = match fit_frames {
                        Some(ff) => fps.frames_to_secs(ff),
                        None => c.duration.or(asset.duration).map(|t| t.seconds(fps, 0.0)).unwrap_or(3.0),
                    };
                    src_in = 0.0;
                    src_out = dur;
                    tl_secs = dur;
                } else {
                    src_in = c.in_.map(|t| t.seconds(src_fps, media_dur)).unwrap_or(0.0);
                    src_out = match (c.out, c.duration, fit_frames) {
                        (Some(o), _, _) => o.seconds(src_fps, media_dur),
                        (None, _, Some(ff)) => src_in + fps.frames_to_secs(ff) * speed,
                        (None, Some(dd), None) => src_in + dd.seconds(src_fps, media_dur) * speed,
                        (None, None, None) => media_dur,
                    };
                    if let (Some(ff), Some(_)) = (fit_frames, c.out) {
                        // in and out fixed: the speed absorbs the difference
                        speed = (src_out - src_in) / fps.frames_to_secs(ff);
                        if !(0.25..=4.0).contains(&speed) && src_out > src_in {
                            d.push(
                                Diagnostic::error(
                                    "E_FIT_SPEED",
                                    format!("{base}/end_at"),
                                    format!(
                                        "fitting {:.3}s of footage into {:.3}s needs speed {speed:.2}, outside 0.25–4",
                                        src_out - src_in,
                                        fps.frames_to_secs(ff)
                                    ),
                                )
                                .hint("widen or narrow `in`/`out`, or remove `out` to trim instead of changing speed"),
                            );
                        }
                    } else if fit_frames.is_some() && f.is_some() && src_out > media_dur + 0.5 / src_fps.as_f64() {
                        d.push(
                            Diagnostic::error(
                                "E_TIME_OUT_OF_RANGE",
                                format!("{base}/end_at"),
                                format!(
                                    "end_at needs footage up to {src_out:.3}s at speed {speed}, but the asset is {media_dur:.3}s long"
                                ),
                            )
                            .hint("set `out` so cutline speeds the clip up to fit, raise `speed`, or move `in` earlier"),
                        );
                        ok = false;
                        continue;
                    }
                    if f.is_some() && src_in >= media_dur - 1e-6 {
                        d.push(
                            Diagnostic::error(
                                "E_TIME_OUT_OF_RANGE",
                                format!("{base}/in"),
                                format!("in point {src_in:.3}s is beyond the asset duration {media_dur:.3}s"),
                            )
                            .hint("check `cutline probe` for the asset duration"),
                        );
                        ok = false;
                    }
                    if f.is_some() && src_out > media_dur + 0.5 / src_fps.as_f64() {
                        d.push(
                            Diagnostic::error(
                                "E_TIME_OUT_OF_RANGE",
                                format!("{base}/out"),
                                format!("out point {src_out:.3}s is beyond the asset duration {media_dur:.3}s"),
                            )
                            .hint(format!("use at most {media_dur:.3} or \"end\"")),
                        );
                        ok = false;
                    }
                    if src_out <= src_in {
                        d.push(Diagnostic::error(
                            "E_EMPTY_RANGE",
                            format!("{base}/out"),
                            format!("out ({src_out:.3}) must be after in ({src_in:.3})"),
                        ));
                        ok = false;
                        continue;
                    }
                    if c.reverse && src_out - src_in > 10.0 {
                        d.push(Diagnostic::error(
                            "E_REVERSE_TOO_LONG",
                            format!("{base}/reverse"),
                            "reverse is limited to clips of 10 s or less",
                        ));
                    }
                    tl_secs = if c.speed_ramp.is_empty() {
                        (src_out - src_in) / speed
                    } else {
                        ramp_duration(src_out - src_in, &c.speed_ramp)
                    };
                }
                // fitted clips take the exact frame count (no rounding drift against the cue)
                let mut frames = fit_frames.unwrap_or_else(|| fps.secs_to_frames(tl_secs));
                let clip_secs = tl_secs;
                for (ei, e) in c.effects.iter().enumerate() {
                    check_effect(e, clip_secs, &format!("{base}/effects/{ei}"), &mut d);
                }
                frames += freeze_frames;
                if let Some(cg) = &c.color {
                    check_color(cg, ctx, &format!("{base}/color"), &mut d);
                }
                if let Some(r) = &c.reframe {
                    check_reframe(r, &format!("{base}/reframe"), &mut d);
                }
                if let Some(a) = &c.audio {
                    if !["keep", "mute", "duck"].contains(&a.as_str()) && !p.assets.contains_key(a) {
                        d.push(Diagnostic::error(
                            "E_UNKNOWN_ASSET",
                            format!("{base}/audio"),
                            format!("clip audio must be keep, mute, duck or an asset id; got `{a}`"),
                        ));
                    }
                }
                if frames < 1 {
                    d.push(Diagnostic::error("E_EMPTY_RANGE", base.clone(), "clip is shorter than one frame"));
                    ok = false;
                    continue;
                }
                let index = clips.len();
                let mut start = clips.last().map(|l: &TClip| l.end()).unwrap_or(0);
                if let Some((tmi, t)) = pending_trans.take() {
                    let tf = if t.kind == "cut" { 0 } else { fps.secs_to_frames(t.duration) };
                    let prev = clips.last().unwrap();
                    let maxf = prev.frames.min(frames) - 1;
                    if tf > maxf {
                        d.push(
                            Diagnostic::error(
                                "E_TRANSITION_TOO_LONG",
                                format!("/main/{tmi}/transition/duration"),
                                format!("transition of {} frames exceeds the neighbouring clip length minus one frame ({maxf} frames)", tf),
                            )
                            .hint(format!("use at most {:.3} s", fps.frames_to_secs(maxf.max(0)))),
                        );
                        ok = false;
                    } else {
                        start -= tf;
                        transitions.push(TTransition {
                            main_index: tmi,
                            after: index - 1,
                            frames: tf,
                            start,
                            transition: t,
                        });
                    }
                }
                let mut clip = (**c).clone();
                if fit_frames.is_some() && !is_visual_still(kind) {
                    // keep the clip copy consistent with what `end_at` resolved to
                    clip.speed = Some(crate::time::round6(speed));
                    clip.out = Some(TimeValue::Seconds(crate::time::round6(src_out)));
                }
                clips.push(TClip {
                    main_index: mi,
                    index,
                    id: c.id.clone(),
                    asset: c.clip.clone(),
                    kind,
                    src_in,
                    src_out,
                    speed,
                    start,
                    frames,
                    clip,
                });
            }
        }
    }
    if let Some((tmi, _)) = pending_trans {
        d.push(Diagnostic::error(
            "E_TRANSITION_POSITION",
            format!("/main/{tmi}"),
            "a transition cannot end the storyline",
        ));
        ok = false;
    }
    let total = clips.last().map(|c| c.end()).unwrap_or(0);
    if let Some(maxd) = spec.max_duration {
        let secs = fps.frames_to_secs(total);
        if secs > maxd {
            d.push(
                Diagnostic::warning(
                    "W_SHORTS_OVER_LIMIT",
                    "/main",
                    format!("timeline is {secs:.1}s; Shorts must be ≤ {maxd:.0}s"),
                )
                .hint("trim clips or use a long-form target"),
            );
        }
    }

    // ---- layers
    let mut layers = vec![];
    for (li, l) in p.layers.iter().enumerate() {
        let base = format!("/layers/{li}");
        if let Some(a) = l.asset() {
            let is_sfx = matches!(l, Layer::Sfx { .. });
            if !p.assets.contains_key(a) && !(is_sfx && ctx.sfx_aliases.iter().any(|x| x == a)) {
                d.push(Diagnostic::error(
                    if is_sfx { "E_UNKNOWN_SFX" } else { "E_UNKNOWN_ASSET" },
                    format!("{base}/asset"),
                    format!("unknown asset `{a}`"),
                ));
                continue;
            }
        }
        if let Layer::Text { template, .. } = l {
            if !catalog::TEXT_TEMPLATES.contains(&template.as_str()) {
                d.push(
                    Diagnostic::error(
                        "E_UNKNOWN_TEMPLATE",
                        format!("{base}/template"),
                        format!("unknown template `{template}`"),
                    )
                    .hint(format!("available: {}", catalog::TEXT_TEMPLATES.join(", "))),
                );
            }
        }
        if let Layer::Mask { rect, strength, text_size, .. } = l {
            let inside = |v: f64| (0.0..=1.0).contains(&v);
            if !(inside(rect.x) && inside(rect.y) && rect.w > 0.0 && rect.h > 0.0)
                || rect.x + rect.w > 1.0 + 1e-9
                || rect.y + rect.h > 1.0 + 1e-9
            {
                d.push(
                    Diagnostic::error(
                        "E_BAD_VALUE",
                        format!("{base}/rect"),
                        "mask rect must lie inside the frame: x, y, w, h are fractions (0–1) with x+w ≤ 1 and y+h ≤ 1",
                    )
                    .hint("e.g. {x: 0.2, y: 0.44, w: 0.75, h: 0.03}; divide pixel values by the frame width/height"),
                );
            }
            if strength.is_some_and(|s| !(0.0..=10.0).contains(&s) || s == 0.0) {
                d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/strength"), "strength must be within (0, 10]"));
            }
            if text_size.is_some_and(|s| s.is_nan() || s <= 0.0) {
                d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/text_size"), "text_size must be positive"));
            }
        }
        if let Some(v) = l.visual() {
            if let Some(o) = v.opacity {
                if !(0.0..=1.0).contains(&o) {
                    d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/opacity"), "opacity must be within 0–1"));
                }
            }
            if let Some(s) = v.scale {
                if !(s > 0.0 && s <= 4.0) {
                    d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/scale"), "scale must be within (0, 4]"));
                }
            }
        }
        // timing
        let media_len = l.asset().and_then(|a| facts.get(a)).filter(|f| !is_visual_still(f.kind)).map(|f| f.duration);
        let (start, end) = match l {
            Layer::Sfx { at, anchor, offset, .. } => {
                let s = match anchor {
                    Some(a) => match clips.iter().find(|c| &c.id == a) {
                        Some(c) => c.start + fps.secs_to_frames(offset.unwrap_or(0.0)),
                        None => {
                            d.push(Diagnostic::error(
                                "E_UNKNOWN_ANCHOR",
                                format!("{base}/anchor"),
                                format!("unknown anchor clip `{a}`"),
                            ));
                            continue;
                        }
                    },
                    None => at.map(|t| t.frames(fps, total)).unwrap_or(0),
                };
                let len = fps.secs_to_frames(media_len.unwrap_or(1.0)).max(1);
                (s, s + len)
            }
            _ => {
                let at = l.placement().unwrap();
                let s = match &at.anchor {
                    Some(a) => match clips.iter().find(|c| &c.id == a) {
                        Some(c) => c.start + fps.secs_to_frames(at.offset.unwrap_or(0.0)),
                        None => {
                            d.push(Diagnostic::error(
                                "E_UNKNOWN_ANCHOR",
                                format!("{base}/anchor"),
                                format!("unknown anchor clip `{a}`"),
                            ));
                            continue;
                        }
                    },
                    None => at.start.map(|t| t.frames(fps, total)).unwrap_or(0),
                };
                let src_len = match l {
                    Layer::Video { src_in, src_out, .. } | Layer::Broll { src_in, src_out, .. } => {
                        let ml = media_len.unwrap_or(0.0);
                        let i = src_in.map(|t| t.seconds(fps, ml)).unwrap_or(0.0);
                        let o = src_out.map(|t| t.seconds(fps, ml)).unwrap_or(ml);
                        if media_len.is_some() && (o > ml + 0.05 || i >= o) {
                            d.push(Diagnostic::error(
                                "E_TIME_OUT_OF_RANGE",
                                format!("{base}/src_out"),
                                format!("source range {i:.3}–{o:.3}s is outside the asset (duration {ml:.3}s)"),
                            ));
                        }
                        Some(o - i)
                    }
                    Layer::Audio { r#loop: Some(true), .. } => None,
                    // backtimed music runs to the timeline end unless told otherwise
                    Layer::Audio { align: Some(AudioAlign::End), .. } => None,
                    Layer::Audio { src_in, .. } => {
                        media_len.map(|ml| ml - src_in.map(|t| t.seconds(fps, ml)).unwrap_or(0.0))
                    }
                    Layer::Html { asset, .. } => {
                        p.assets.get(asset).and_then(|a| a.duration).map(|t| t.seconds(fps, 0.0))
                    }
                    _ => None,
                };
                let default_end = match l {
                    Layer::Text { .. } => s + fps.secs_to_frames(3.0),
                    _ => match src_len {
                        Some(sl) => s + fps.secs_to_frames(sl),
                        None => total,
                    },
                };
                let e = match (at.end, at.duration) {
                    (Some(e), _) => e.frames(fps, total),
                    (None, Some(dd)) => s + dd.frames(fps, total),
                    (None, None) => default_end,
                };
                (s, e.min(total.max(s + 1)))
            }
        };
        if end <= start {
            d.push(Diagnostic::error(
                "E_LAYER_TIME",
                base.clone(),
                format!("layer ends ({}) before it starts ({})", end, start),
            ));
            continue;
        }
        if start >= total && total > 0 {
            d.push(Diagnostic::error("E_LAYER_TIME", base.clone(), "layer starts after the timeline ends"));
            continue;
        }
        let z = l.visual().and_then(|v| v.z).unwrap_or(li as i32);
        let end = end.min(total);
        let (mut start, mut layer) = (start, l.clone());
        if let Layer::Audio { align: Some(AudioAlign::End), src_in, r#loop, .. } = &mut layer {
            if src_in.is_some() || *r#loop == Some(true) {
                d.push(
                    Diagnostic::error(
                        "E_FIELD_CONFLICT",
                        format!("{base}/align"),
                        "align: end chooses the source start itself, so it cannot be combined with `src_in` or `loop`",
                    )
                    .hint("remove `src_in`/`loop`, or remove `align`"),
                );
            } else if let Some(ml) = media_len {
                let span = fps.frames_to_secs(end - start);
                if ml >= span {
                    // trim the head so the asset's last sample lands on the layer end
                    *src_in = Some(TimeValue::Seconds(crate::time::round6(ml - span)));
                } else {
                    // shorter than the span: start later instead
                    start = end - fps.secs_to_frames(ml);
                }
            }
        }
        layers.push(TLayer { index: li, start, end, z, layer });
    }

    // ---- captions / audio settings
    if let Some(c) = &p.captions {
        if let Some(s) = &c.style {
            if !catalog::CAPTION_STYLES.contains(&s.as_str()) {
                d.push(
                    Diagnostic::error(
                        "E_UNKNOWN_CAPTION_STYLE",
                        "/captions/style",
                        format!("unknown caption style `{s}`"),
                    )
                    .hint(catalog::CAPTION_STYLES.join(", ")),
                );
            }
        }
        if let Some(f) = &c.font {
            if !ctx.fonts.is_empty() && !ctx.fonts.iter().any(|x| x.eq_ignore_ascii_case(f)) {
                d.push(
                    Diagnostic::error("E_FONT_MISSING", "/captions/font", format!("font `{f}` is not bundled"))
                        .hint(format!("bundled: {}", ctx.fonts.join(", "))),
                );
            }
        }
        if c.source == CaptionSource::File && c.file.is_none() {
            d.push(Diagnostic::error("E_MISSING_FIELD", "/captions/file", "captions.source=file needs `file`"));
        }
        for a in &c.assets {
            if !p.assets.contains_key(a) {
                d.push(Diagnostic::error("E_UNKNOWN_ASSET", "/captions/assets", format!("unknown asset `{a}`")));
            }
        }
    }
    if let Some(a) = &p.audio {
        for (k, v) in [("voice_chain", &a.voice_chain), ("dialogue_chain", &a.dialogue_chain)] {
            if let Some(v) = v {
                if !catalog::VOICE_CHAINS.contains(&v.as_str()) {
                    d.push(
                        Diagnostic::error(
                            "E_UNKNOWN_VOICE_CHAIN",
                            format!("/audio/{k}"),
                            format!("unknown voice chain `{v}`"),
                        )
                        .hint(catalog::VOICE_CHAINS.join(", ")),
                    );
                }
            }
        }
        if let Some(m) = a.music_level {
            if !(3.0..=40.0).contains(&m) {
                d.push(
                    Diagnostic::error(
                        "E_BAD_VALUE",
                        "/audio/music_level",
                        "music_level is how many LU the music sits below the voice: 3–40",
                    )
                    .hint("18 is a typical bed under narration; 12 for a livelier mix"),
                );
            }
        }
        if let Some(l) = &a.loudness {
            if !(-40.0..=-5.0).contains(&l.integrated) || !(-9.0..=0.0).contains(&l.true_peak) {
                d.push(Diagnostic::error(
                    "E_BAD_VALUE",
                    "/audio/loudness",
                    "loudness target out of range (integrated −40…−5 LUFS, true peak −9…0 dBTP)",
                ));
            }
        }
    }
    if let Some(sb) = p.auto.as_ref().and_then(|a| a.snap_to_beats.as_ref()) {
        if !(0.01..=1.0).contains(&sb.max_shift) {
            d.push(Diagnostic::error(
                "E_BAD_VALUE",
                "/auto/snap_to_beats/max_shift",
                "max_shift must be within 0.01–1 s",
            ));
        }
        if let Some(a) = &sb.asset {
            if !p.assets.contains_key(a) {
                d.push(Diagnostic::error(
                    "E_UNKNOWN_ASSET",
                    "/auto/snap_to_beats/asset",
                    format!("unknown asset `{a}`"),
                ));
            }
        }
    }
    if let Some(s) = p.auto.as_ref().and_then(|a| a.sfx_on_transitions.as_ref()) {
        check_sfx(s, p, ctx, "/auto/sfx_on_transitions", &mut d);
    }

    let tl = (ok && !clips.is_empty()).then(|| {
        layers.sort_by_key(|l| (l.z, l.index));
        Timeline { fps, target: spec, total_frames: total, clips, transitions, layers }
    });
    (tl, d)
}

fn check_sfx(s: &str, p: &Project, ctx: &Context, path: &str, d: &mut Vec<Diagnostic>) {
    if !p.assets.contains_key(s) && !ctx.sfx_aliases.iter().any(|x| x == s) {
        d.push(Diagnostic::error("E_UNKNOWN_SFX", path, format!("unknown sfx `{s}` (not an asset id or brand alias)")));
    }
}

fn check_effect(e: &Effect, clip_secs: f64, base: &str, d: &mut Vec<Diagnostic>) {
    let at = match e {
        Effect::PunchIn { at, scale, .. } => {
            if !(1.0..=3.0).contains(scale) {
                d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/scale"), "punch-in scale must be within 1–3"));
            }
            Some(*at)
        }
        Effect::Shake { at, .. }
        | Effect::Flash { at, .. }
        | Effect::Glitch { at, .. }
        | Effect::MotionBlur { at, .. }
        | Effect::Freeze { at, .. } => Some(*at),
        Effect::Zoom { start, end, to, from, .. } => {
            if !(1.0..=3.0).contains(to) || !(1.0..=3.0).contains(from) {
                d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/to"), "zoom scale must be within 1–3"));
            }
            if let Some(e) = end {
                if e <= start {
                    d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/end"), "zoom end must be after start"));
                }
            }
            Some(*start)
        }
        Effect::ZoomTo { rect, start, duration, padding, end, .. } => {
            let inside = |v: f64| (0.0..=1.0).contains(&v);
            if !(inside(rect.x) && inside(rect.y) && rect.w > 0.0 && rect.h > 0.0)
                || rect.x + rect.w > 1.0 + 1e-9
                || rect.y + rect.h > 1.0 + 1e-9
            {
                d.push(
                    Diagnostic::error(
                        "E_BAD_VALUE",
                        format!("{base}/rect"),
                        "zoom_to rect must lie inside the frame (fractions 0–1, x+w ≤ 1, y+h ≤ 1)",
                    )
                    .hint("e.g. {x: 0.2, y: 0.6, w: 0.4, h: 0.3}"),
                );
            } else {
                let (s, _) = zoom_to_params(rect, padding.unwrap_or(0.03), f64::INFINITY);
                if s > 3.0 {
                    d.push(
                        Diagnostic::warning(
                            "W_ZOOM_CAPPED",
                            format!("{base}/rect"),
                            format!("the rect needs a {s:.1}× zoom; capped at 3× (it will not fill the view)"),
                        )
                        .hint("use a larger rect, or accept the cap"),
                    );
                }
            }
            if duration.is_nan() || *duration <= 0.0 {
                d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/duration"), "duration must be positive"));
            }
            if padding.is_some_and(|p| !(0.0..=0.4).contains(&p)) {
                d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/padding"), "padding must be within 0–0.4"));
            }
            if end.is_some_and(|e| e < start + duration) {
                d.push(Diagnostic::error(
                    "E_BAD_VALUE",
                    format!("{base}/end"),
                    "zoom back out (end) must start after the zoom in finishes (start + duration)",
                ));
            }
            Some(*start)
        }
        _ => None,
    };
    if let Some(at) = at {
        if at < 0.0 || at >= clip_secs {
            d.push(
                Diagnostic::error(
                    "E_EFFECT_OUT_OF_RANGE",
                    format!("{base}/at"),
                    format!("effect time {at:.3}s is outside the clip (0–{clip_secs:.3}s, clip-relative)"),
                )
                .hint("effect times are relative to the clip start on the timeline"),
            );
        }
    }
}

fn check_color(c: &ColorGrade, ctx: &Context, base: &str, d: &mut Vec<Diagnostic>) {
    if let Some(p) = &c.preset {
        if !catalog::COLOR_PRESETS.contains(&p.as_str()) {
            d.push(
                Diagnostic::error(
                    "E_UNKNOWN_COLOR_PRESET",
                    format!("{base}/preset"),
                    format!("unknown color preset `{p}`"),
                )
                .hint(catalog::COLOR_PRESETS.join(", ")),
            );
        }
    }
    if let Some(l) = &c.lut {
        if !catalog::BUNDLED_LUTS.contains(&l.as_str()) && !ctx.project_dir.join(l).exists() {
            d.push(
                Diagnostic::error(
                    "E_ASSET_NOT_FOUND",
                    format!("{base}/lut"),
                    format!("LUT `{l}` is neither bundled nor a file"),
                )
                .hint(catalog::BUNDLED_LUTS.join(", ")),
            );
        }
    }
    if let Some(i) = c.intensity {
        if !(0.0..=1.0).contains(&i) {
            d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/intensity"), "intensity must be within 0–1"));
        }
    }
}

fn check_reframe(r: &Reframe, base: &str, d: &mut Vec<Diagnostic>) {
    if r.mode == ReframeMode::Manual && r.keys.is_empty() {
        d.push(Diagnostic::error("E_MISSING_FIELD", format!("{base}/keys"), "manual reframe needs at least one key"));
    }
    for (i, k) in r.keys.iter().enumerate() {
        if !(0.0..=1.0).contains(&k.x) || !(0.0..=1.0).contains(&k.y) || !(1.0..=4.0).contains(&k.zoom) {
            d.push(Diagnostic::error(
                "E_BAD_VALUE",
                format!("{base}/keys/{i}"),
                "reframe key x/y must be within 0–1 and zoom within 1–4",
            ));
        }
    }
    if r.mode == ReframeMode::Split && !r.regions.is_empty() && r.regions.len() != 2 {
        d.push(Diagnostic::error("E_BAD_VALUE", format!("{base}/regions"), "split needs exactly two regions"));
    }
}

/// Snap a time value at the timeline fps (helper for callers).
pub fn frames_of(t: &TimeValue, fps: Fps, total: i64) -> i64 {
    t.frames(fps, total)
}
