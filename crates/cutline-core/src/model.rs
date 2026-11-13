//! Project file model (schema v1). This is the user-facing contract: changes need a `version`
//! bump and a migration note.

use crate::time::{Fps, TimeValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA_VERSION: u32 = 1;

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Schema version. Must be 1.
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Output target preset (geometry, fps, encoder, safe zones).
    #[serde(default)]
    pub target: TargetName,
    /// Timeline fps. Defaults from the target (long-form targets follow the first clip's source).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<Fps>,
    /// Width/height for `target: custom`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Brand file path (relative to the project) or `default` for the bundled brand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(default)]
    pub assets: BTreeMap<String, Asset>,
    /// Main storyline: clips, with transitions between them.
    #[serde(default)]
    pub main: Vec<MainItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<Layer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captions: Option<Captions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto: Option<AutoDirectives>,
    /// Default geometry for clips whose aspect differs from the target (default `cover`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reframe: Option<Reframe>,
    /// Override the target's safe zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_zone: Option<SafeZone>,
    /// Chapter markers (long form).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<Export>,
    /// Named points in the voice-over, found in its transcript: `{clocks: "World clocks"}`.
    /// Timeline times (layer `start`/`end`, sfx `at`, clip `end_at`, marker `at`) can refer to
    /// them as `"cue:clocks"`, `"cue:clocks+0.4"` or `"cue:clocks-0.2"`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cues: BTreeMap<String, Cue>,
}

/// A cue: a phrase in a voice track. The short form is just the phrase.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum Cue {
    Phrase(String),
    Spec(CueSpec),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CueSpec {
    /// Words to find (case and punctuation are ignored; close transcription slips match).
    pub phrase: String,
    /// Asset whose transcript is searched. Default: the only voice-role audio layer's asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// Which occurrence of the phrase (1-based, default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<usize>,
    /// `start` (default): when the first word begins; `end`: when the last word ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<CueEdge>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CueEdge {
    #[default]
    Start,
    End,
}

impl Cue {
    pub fn phrase(&self) -> &str {
        match self {
            Cue::Phrase(p) => p,
            Cue::Spec(s) => &s.phrase,
        }
    }
    pub fn asset(&self) -> Option<&str> {
        match self {
            Cue::Phrase(_) => None,
            Cue::Spec(s) => s.asset.as_deref(),
        }
    }
    pub fn occurrence(&self) -> usize {
        match self {
            Cue::Phrase(_) => 1,
            Cue::Spec(s) => s.occurrence.unwrap_or(1),
        }
    }
    pub fn edge(&self) -> CueEdge {
        match self {
            Cue::Phrase(_) => CueEdge::Start,
            Cue::Spec(s) => s.edge.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TargetName {
    #[default]
    Shorts,
    #[serde(rename = "long_1080p")]
    Long1080p,
    #[serde(rename = "long_4k")]
    Long4k,
    Square,
    Custom,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Video,
    Image,
    Audio,
    ImageSequence,
    Html,
    Lottie,
    Svg,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Main clip audio.
    Dialogue,
    /// Voice-over.
    Voice,
    Music,
    Sfx,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    /// Path relative to the project file. Image sequences use a printf pattern (`frames/%05d.png`).
    pub path: String,
    /// Auto-detected from ffprobe and the extension when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AssetKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// Transcript for this asset: Scribe JSON or cutline transcript JSON (relative path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// The text that was spoken (relative path to a .txt file), e.g. the TTS script. Captions
    /// and cues then use the script's exact words, timed from the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// Duration for HTML/Lottie/SVG/still assets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<TimeValue>,
    /// Asset is expected to carry alpha (validated against probe).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<bool>,
    /// Frame rate for image sequences / HTML renders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<Fps>,
    /// Parameters injected into HTML templates as `window.__cutline_params`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// Render size for HTML/SVG/Lottie (defaults to the target size).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Mark this asset as AI-generated (affects the publishing checklist).
    #[serde(default, skip_serializing_if = "is_false")]
    pub synthetic: bool,
}

/// An element of `main`: either a clip or `{transition: {...}}` between two clips.
#[derive(Debug, Clone, Serialize, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum MainItem {
    Transition(TransitionItem),
    Clip(Box<Clip>),
}

impl<'de> Deserialize<'de> for MainItem {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        if v.get("transition").is_some() {
            TransitionItem::deserialize(v).map(MainItem::Transition).map_err(serde::de::Error::custom)
        } else {
            Clip::deserialize(v).map(|c| MainItem::Clip(Box::new(c))).map_err(serde::de::Error::custom)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransitionItem {
    pub transition: Transition,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// xfade name (fade, wipeleft, slideup, circleopen, …) or custom: whip_left/right/up/down,
    /// zoom_punch, flash_cut, glitch, cut.
    #[serde(rename = "type")]
    pub kind: String,
    /// Seconds.
    pub duration: f64,
    /// SFX asset id or brand alias played at the transition midpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sfx: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sfx_gain_db: Option<f64>,
    /// Audio behaviour: crossfade (default) or cut (hard cut with micro-fades).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<TransitionAudio>,
    /// acrossfade curve (tri, qsin, exp, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<String>,
    /// Flash/fill color for flash_cut and similar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Set by `auto` expansion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransitionAudio {
    Crossfade,
    Cut,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    /// Unique id (used for anchors, chapters, review output).
    pub id: String,
    /// Asset id.
    pub clip: String,
    /// Source in point (default 0).
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub in_: Option<TimeValue>,
    /// Source out point (default: end of the asset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<TimeValue>,
    /// Timeline duration for stills / generated assets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<TimeValue>,
    /// Timeline time of the cut to the next clip (the middle of the outgoing transition), e.g.
    /// `"cue:calendar"`. The clip is fitted to end there: with `in` and `out` both set its
    /// speed is computed; otherwise `out` is (at `speed`, default 1); stills get the duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at: Option<TimeValue>,
    /// Constant speed factor 0.25–4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    /// Keyframed speed curve (overrides `speed`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speed_ramp: Vec<SpeedKey>,
    /// Play the clip backwards (short clips only).
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverse: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reframe: Option<Reframe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorGrade>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    /// Clip audio: keep (default), mute, duck (−12 dB), or an asset id replacing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stabilize: Option<Stabilize>,
    /// Chapter title (long form); defaults to the clip id when the clip starts a chapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter: Option<String>,
    /// Set by `auto` expansion (e.g. `jump_cuts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpeedKey {
    /// Source-relative time (seconds from the clip's in point).
    pub t: f64,
    pub speed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stabilize {
    #[serde(default = "half")]
    pub strength: f64,
}
fn half() -> f64 {
    0.5
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReframeMode {
    /// Scale to cover and center-crop.
    #[default]
    Cover,
    /// Alias of cover.
    Center,
    /// Fit inside with a blurred, scaled copy as background.
    FitBlur,
    /// Fit inside, pad with the brand background color.
    FitPad,
    /// Follow the detected face.
    TrackFace,
    /// Keyframed crop rectangle.
    Manual,
    /// Two regions stacked vertically.
    Split,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Reframe {
    #[serde(default)]
    pub mode: ReframeMode,
    /// Manual keys: crop center (x, y in 0..1 of source) and zoom (1 = largest crop).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<ReframeKey>,
    /// Where the subject should sit in the output (default x=0.5, y=0.38).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bias: Option<Point>,
    /// largest | most_persistent | left | right | index | speaker (experimental)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_index: Option<usize>,
    /// Max pan speed in fractions of the frame width per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pan_speed: Option<f64>,
    /// Fallback when no faces are found (default fit_blur).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<ReframeMode>,
    /// Split layout regions (source-normalized rectangles), top then bottom.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<Rect>,
    /// fit_pad color (default brand background).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad_color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReframeKey {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    #[serde(default = "one_f")]
    pub zoom: f64,
}
fn one_f() -> f64 {
    1.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct ColorGrade {
    /// natural | punchy | warm_film | cool_clean | bw_contrast
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Bundled LUT name or a `.cube` path relative to the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lut: Option<String>,
    /// LUT blend 0..1 (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intensity: Option<f64>,
    /// Stops (−2..2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f64>,
    /// Kelvin (e.g. 5500 warm … 7500 cool); 6500 is neutral.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Green/magenta −1..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<f64>,
    /// unsharp amount 0..2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharpen: Option<f64>,
}

/// Easing curves.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    Linear,
    InQuad,
    OutQuad,
    InOutQuad,
    InCubic,
    #[default]
    OutCubic,
    InOutCubic,
    InExpo,
    OutExpo,
    InOutExpo,
    Back,
    Spring,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    /// Quick zoom in at `at` (clip-relative seconds), holding until `hold` ends, then optionally out.
    PunchIn {
        at: f64,
        #[serde(default = "punch_scale")]
        scale: f64,
        #[serde(default = "punch_dur")]
        duration: f64,
        #[serde(default)]
        ease: Ease,
        /// Seconds to hold the zoom (default: until clip end).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hold: Option<f64>,
        /// Zoom focus point in output-normalized coordinates (default center).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<Point>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generated_by: Option<String>,
    },
    /// Continuous zoom from→to across [start, end] (clip-relative).
    Zoom {
        #[serde(default = "one_f")]
        from: f64,
        to: f64,
        #[serde(default)]
        start: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end: Option<f64>,
        #[serde(default)]
        ease: Ease,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<Point>,
    },
    /// Slow pan+zoom (stills and B-roll).
    KenBurns {
        #[serde(default = "one_f")]
        from_scale: f64,
        #[serde(default = "kb_to")]
        to_scale: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Point>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<Point>,
        #[serde(default)]
        ease: Ease,
    },
    Shake {
        at: f64,
        #[serde(default = "shake_dur")]
        duration: f64,
        #[serde(default = "shake_int")]
        intensity: f64,
    },
    Flash {
        at: f64,
        #[serde(default = "flash_dur")]
        duration: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
    },
    BlurIn {
        #[serde(default = "blur_dur")]
        duration: f64,
    },
    Glitch {
        at: f64,
        #[serde(default = "flash_dur")]
        duration: f64,
        #[serde(default = "glitch_amt")]
        amount: f64,
    },
    Vignette {
        #[serde(default = "vig")]
        strength: f64,
    },
    MotionBlur {
        at: f64,
        #[serde(default = "flash_dur")]
        duration: f64,
    },
    /// Hold the frame at `at` (clip-relative) for `duration` seconds (extends the clip).
    Freeze { at: f64, duration: f64 },
}

fn punch_scale() -> f64 {
    1.15
}
fn punch_dur() -> f64 {
    0.25
}
fn kb_to() -> f64 {
    1.15
}
fn shake_dur() -> f64 {
    0.3
}
fn shake_int() -> f64 {
    0.02
}
fn flash_dur() -> f64 {
    0.15
}
fn blur_dur() -> f64 {
    0.4
}
fn glitch_amt() -> f64 {
    12.0
}
fn vig() -> f64 {
    0.4
}

/// Position: a preset name or normalized center coordinates.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum Position {
    /// center | top | bottom | left | right | top_left | top_right | bottom_left | bottom_right |
    /// lower_mid | upper_mid
    Preset(String),
    Point(Point),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Anim {
    None,
    Fade,
    SlideLeft,
    SlideRight,
    SlideUp,
    SlideDown,
    Pop,
    ScaleIn,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    Cover,
    Contain,
    Stretch,
}

/// Timing shared by all layers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
pub struct Placement {
    /// Absolute timeline start (seconds/timecode). Ignored when `anchor` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<TimeValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<TimeValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<TimeValue>,
    /// Main clip id this layer follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// Seconds after the anchor clip's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
}

/// Look shared by visual layers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
pub struct Visual {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Width as a fraction of the frame width (1 = full width).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// Degrees clockwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Stacking order (higher = on top). Default: order in `layers`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enter: Option<Anim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<Anim>,
    /// Enter/exit animation duration (default 0.3 s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anim_duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<Fit>,
    /// Corner radius in output pixels (PiP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// Border width (px) and color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_color: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub shadow: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Layer {
    /// Text template (hook_title, lower_third, callout, step_counter, big_number, quote,
    /// cta_subscribe, end_card, emoji_pop).
    Text {
        template: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        /// Template parameters (colors, font, size, emphasis, …).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<serde_json::Map<String, serde_json::Value>>,
        #[serde(flatten)]
        at: Placement,
        #[serde(flatten)]
        look: Visual,
    },
    /// Alpha or opaque video overlay.
    Video {
        asset: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_in: Option<TimeValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_out: Option<TimeValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gain_db: Option<f64>,
        #[serde(flatten)]
        at: Placement,
        #[serde(flatten)]
        look: Visual,
    },
    /// B-roll (full-frame cover by default, or PiP with scale/position).
    Broll {
        asset: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_in: Option<TimeValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_out: Option<TimeValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gain_db: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ken_burns: Option<bool>,
        #[serde(flatten)]
        at: Placement,
        #[serde(flatten)]
        look: Visual,
    },
    Image {
        asset: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ken_burns: Option<bool>,
        #[serde(flatten)]
        at: Placement,
        #[serde(flatten)]
        look: Visual,
    },
    /// HTML/Lottie/SVG asset rendered via headless Chrome.
    Html {
        asset: String,
        #[serde(flatten)]
        at: Placement,
        #[serde(flatten)]
        look: Visual,
    },
    /// Retention progress bar.
    ProgressBar {
        /// `brand` or a color.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
        /// top (default) | bottom
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edge: Option<String>,
        /// Height in px (default 12 for shorts, 8 otherwise).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(flatten)]
        at: Placement,
    },
    /// Music / VO bed.
    Audio {
        asset: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_in: Option<TimeValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gain_db: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fade_in: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fade_out: Option<f64>,
        /// Loop the asset to fill the span.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#loop: Option<bool>,
        /// Override the asset role (voice/music/sfx).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<Role>,
        #[serde(flatten)]
        at: Placement,
    },
    /// One-shot sound effect at `at` (or anchored).
    Sfx {
        /// Asset id, brand SFX alias, or a list to pick from deterministically.
        asset: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<TimeValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anchor: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gain_db: Option<f64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pick_from: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generated_by: Option<String>,
    },
}

impl Layer {
    pub fn kind(&self) -> &'static str {
        match self {
            Layer::Text { .. } => "text",
            Layer::Video { .. } => "video",
            Layer::Broll { .. } => "broll",
            Layer::Image { .. } => "image",
            Layer::Html { .. } => "html",
            Layer::ProgressBar { .. } => "progress_bar",
            Layer::Audio { .. } => "audio",
            Layer::Sfx { .. } => "sfx",
        }
    }
    pub fn placement(&self) -> Option<&Placement> {
        match self {
            Layer::Text { at, .. }
            | Layer::Video { at, .. }
            | Layer::Broll { at, .. }
            | Layer::Image { at, .. }
            | Layer::Html { at, .. }
            | Layer::ProgressBar { at, .. }
            | Layer::Audio { at, .. } => Some(at),
            Layer::Sfx { .. } => None,
        }
    }
    pub fn visual(&self) -> Option<&Visual> {
        match self {
            Layer::Text { look, .. }
            | Layer::Video { look, .. }
            | Layer::Broll { look, .. }
            | Layer::Image { look, .. }
            | Layer::Html { look, .. } => Some(look),
            _ => None,
        }
    }
    pub fn asset(&self) -> Option<&str> {
        match self {
            Layer::Video { asset, .. }
            | Layer::Broll { asset, .. }
            | Layer::Image { asset, .. }
            | Layer::Html { asset, .. }
            | Layer::Audio { asset, .. }
            | Layer::Sfx { asset, .. } => Some(asset),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptionSource {
    /// Transcribe / import transcripts and map words through the edit.
    #[default]
    Auto,
    /// Use `file` (SRT or cutline transcript JSON in timeline time).
    File,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Captions {
    #[serde(default)]
    pub source: CaptionSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// bold_pop | karaoke | word_by_word | clean_sentence | boxed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// lower_mid (default) | center | upper_mid | bottom | top, or {x, y}
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_words_per_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chars_per_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lines: Option<u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub corrections: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emphasis: Vec<String>,
    /// Emoji to append to emphasis words (word → emoji name).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub emoji: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Font size in px at the output resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uppercase: Option<bool>,
    /// Asset ids whose transcripts feed captions (default: all clip assets + voice layers).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<String>,
    /// Language for transcription (e.g. `en`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct AudioSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness: Option<LoudnessTarget>,
    /// clean_voice | podcast_warm | raw — applied to voice (VO) assets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_chain: Option<String>,
    /// Chain applied to dialogue (main clip audio); default same as voice_chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialogue_chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duck: Option<Duck>,
    /// Micro-fade length at cuts in seconds (default 0.01).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub micro_fade: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LoudnessTarget {
    #[serde(default = "lufs")]
    pub integrated: f64,
    #[serde(default = "tp")]
    pub true_peak: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lra: Option<f64>,
}
fn lufs() -> f64 {
    -14.0
}
fn tp() -> f64 {
    -1.0
}
impl Default for LoudnessTarget {
    fn default() -> Self {
        Self { integrated: -14.0, true_peak: -1.0, lra: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Duck {
    /// Roles whose speech triggers ducking.
    #[serde(default = "duck_under")]
    pub under: Vec<Role>,
    /// Roles that get ducked.
    #[serde(default = "duck_target")]
    pub target: Vec<Role>,
    #[serde(default = "duck_amt")]
    pub amount_db: f64,
    #[serde(default = "duck_att")]
    pub attack: f64,
    #[serde(default = "duck_rel")]
    pub release: f64,
    /// envelope (default) | sidechain
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}
fn duck_under() -> Vec<Role> {
    vec![Role::Voice, Role::Dialogue]
}
fn duck_target() -> Vec<Role> {
    vec![Role::Music]
}
fn duck_amt() -> f64 {
    -12.0
}
fn duck_att() -> f64 {
    0.12
}
fn duck_rel() -> f64 {
    0.45
}
impl Default for Duck {
    fn default() -> Self {
        Self { under: duck_under(), target: duck_target(), amount_db: -12.0, attack: 0.12, release: 0.45, mode: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct AutoDirectives {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump_cuts: Option<JumpCuts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_fillers: Option<RemoveFillers>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub punch_ins: Option<PunchIns>,
    /// SFX asset/alias auto-placed on every transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sfx_on_transitions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JumpCuts {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "min_sil")]
    pub min_silence: f64,
    #[serde(default = "keep_pad")]
    pub keep_padding: f64,
    /// Asset ids (default: all clip assets).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applies_to: Vec<String>,
    /// silencedetect noise floor in dB (default −35).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_db: Option<f64>,
}
fn yes() -> bool {
    true
}
fn min_sil() -> f64 {
    0.35
}
fn keep_pad() -> f64 {
    0.08
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemoveFillers {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "fillers")]
    pub words: Vec<String>,
}
fn fillers() -> Vec<String> {
    ["um", "uh", "erm", "uhm", "hmm"].iter().map(|s| s.to_string()).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PunchIns {
    #[serde(default = "every")]
    pub every: f64,
    /// [min, max] scale; alternates between them.
    #[serde(default = "pscale")]
    pub scale: [f64; 2],
    #[serde(default = "yes")]
    pub skip_if_other_motion: bool,
    #[serde(default = "punch_dur")]
    pub duration: f64,
}
fn every() -> f64 {
    4.0
}
fn pscale() -> [f64; 2] {
    [1.08, 1.15]
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SafeZone {
    /// Fractions of the frame kept clear.
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub at: TimeValue,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Export {
    /// Export name (used in the output file name). Defaults to the target name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub target: TargetName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<Fps>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crf: Option<u32>,
    /// Output path (relative to the project). Default `out/<name>.mp4`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
}

impl Project {
    pub fn clips(&self) -> impl Iterator<Item = &Clip> {
        self.main.iter().filter_map(|m| match m {
            MainItem::Clip(c) => Some(c.as_ref()),
            _ => None,
        })
    }
}
