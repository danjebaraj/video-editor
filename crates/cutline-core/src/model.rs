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
    /// `screencast`: tuned for screen and terminal recordings (sharper text encoding; frozen
    /// frames are expected, so QA does not warn about them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    /// Chroma subsampling of the deliverable: `420` (default, plays everywhere) or `444` (full
    /// color resolution: no fringes on small colored text; not all players support it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chroma: Option<Chroma>,
    /// Named points in the voice-over, found in its transcript: `{clocks: "World clocks"}`.
    /// Timeline times (layer `start`/`end`, sfx `at`, clip `end_at`, marker `at`) can refer to
    /// them as `"cue:clocks"`, `"cue:clocks+0.4"` or `"cue:clocks-0.2"`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cues: BTreeMap<String, Cue>,
    /// Screen frame for every video clip that has no `frame` of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_frame: Option<ScreenFrame>,
    /// `false`: do not add the brand's `intro` / `outro` around `main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand_bumpers: Option<bool>,
    /// `false`: do not draw the brand logo as a watermark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark: Option<bool>,
}

/// `generated_by` of the clips and assets added for the brand's intro and outro.
pub const BRAND_INTRO: &str = "brand_intro";
pub const BRAND_OUTRO: &str = "brand_outro";

impl Clip {
    /// A brand intro/outro clip (added around `main`; timeline times do not count it).
    pub fn is_bumper(&self) -> bool {
        matches!(self.generated_by.as_deref(), Some(BRAND_INTRO) | Some(BRAND_OUTRO))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Standard,
    Screencast,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub enum Chroma {
    #[default]
    #[serde(rename = "420")]
    C420,
    #[serde(rename = "444")]
    C444,
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
    /// 1080×1350 feed video (LinkedIn, Instagram).
    #[serde(rename = "portrait_4x5")]
    Portrait4x5,
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
    /// Progress curve. Default: `in_out_cubic` for moving transitions (slides, covers, whips,
    /// zooms), `in_out_quad` for the rest (fades, wipes); `linear` for a constant rate.
    /// `back` and `spring` are not allowed (progress must not run backwards).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Ease>,
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
    /// `"cue:calendar"`, `"bar:12"`. The clip is fitted to end there at its `speed` (default
    /// 1): footage past the cut is trimmed (an `out` is then only a limit), and footage that
    /// runs out early holds its last frame (`W_FIT_HOLD`). Stills get the duration. Set
    /// `fit: "speed"` to change the speed instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at: Option<TimeValue>,
    /// With `end_at`, `in` and `out`: `trim` (default) keeps the speed and trims or holds;
    /// `speed` plays exactly `in`..`out` at whatever speed fills the slot (frame-blended).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<EndFit>,
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
    /// Draw the clip inside a window (browser, app window, phone, tablet) on a background,
    /// instead of full-bleed. `zoom_to` rects and annotations stay relative to the recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ScreenFrame>,
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

/// A screen frame: the recording inside generated window chrome on a background.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct ScreenFrame {
    #[serde(default)]
    pub style: FrameStyle,
    /// Address shown in a `browser` frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Title shown in a `window` frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `brand` (default: the brand background), a color, `{gradient: [from, to]}`, or the id
    /// of an image asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<FrameBackground>,
    /// Space around the window as a fraction of the frame's shorter side (default 0.06).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<f64>,
    /// Corner radius in output px (default 14 at 1080p; phones and tablets have their own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// Drop shadow strength 0–1 (default 0.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<f64>,
    /// Chrome colors: dark (default) or light.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<FrameTheme>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FrameStyle {
    /// Title bar with an address field.
    #[default]
    Browser,
    /// Title bar with a title.
    Window,
    /// Rounded bezel, portrait recordings.
    Phone,
    Tablet,
    /// No chrome: just rounded corners, shadow and background.
    None,
    /// No frame at all (opts a clip out of `default_frame`).
    Off,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FrameTheme {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum FrameBackground {
    Gradient {
        /// Two colors, top-left to bottom-right.
        gradient: Vec<String>,
    },
    /// `brand`, a color (`#101014`) or an image asset id.
    Name(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndFit {
    Trim,
    Speed,
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
    /// Zoom so `rect` (fractions of the frame) fills the view, starting at `start`
    /// (clip-relative) over `duration`; zoom back out from `end` if set.
    ZoomTo {
        rect: Rect,
        #[serde(default)]
        start: f64,
        #[serde(default = "zoom_to_dur")]
        duration: f64,
        #[serde(default = "in_out_cubic")]
        ease: Ease,
        /// Margin around the rect as a fraction of the frame (default 0.03).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        padding: Option<f64>,
        /// Clip-relative time to start zooming back out (same duration and ease).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end: Option<f64>,
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
fn zoom_to_dur() -> f64 {
    0.6
}
fn in_out_cubic() -> Ease {
    Ease::InOutCubic
}

/// Scale and focus that make `rect` (plus `padding` on each side) fill the frame, for the
/// zoom chain (`x' = s·x − (s−1)·focus`). The scale is capped at `max_scale`.
pub fn zoom_to_params(rect: &Rect, padding: f64, max_scale: f64) -> (f64, Point) {
    let rw = (rect.w + 2.0 * padding).clamp(1e-3, 1.0);
    let rh = (rect.h + 2.0 * padding).clamp(1e-3, 1.0);
    let s = (1.0 / rw).min(1.0 / rh).clamp(1.0, max_scale);
    if s <= 1.0 + 1e-9 {
        return (1.0, Point { x: 0.5, y: 0.5 });
    }
    let view = 1.0 / s;
    let axis = |c: f64| {
        // window start, kept inside the frame, then the fixed point that lands there
        let start = (c - view / 2.0).clamp(0.0, 1.0 - view);
        (start * s / (s - 1.0)).clamp(0.0, 1.0)
    };
    (s, Point { x: axis(rect.x + rect.w / 2.0), y: axis(rect.y + rect.h / 2.0) })
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
        /// `end`: backtime so the music's real ending lands on the layer end (default: the
        /// timeline end), instead of fading out mid-phrase.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        align: Option<AudioAlign>,
        /// Override the asset role (voice/music/sfx).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<Role>,
        #[serde(flatten)]
        at: Placement,
    },
    /// Hide part of the frame: usernames, paths, emails, keys. Drawn over the footage and other
    /// overlays (below text and captions).
    Mask {
        /// Region as fractions of the frame: `{x, y, w, h}` (top-left corner, size).
        rect: Rect,
        /// blur (default) | pixelate | fill. Use `fill` for secrets: blurred or pixelated text
        /// can sometimes be recovered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<MaskMode>,
        /// Fill color (default `#000000`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        /// Blur/pixelation strength multiplier (default 1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        strength: Option<f64>,
        /// Replacement text drawn in the region (left-aligned, vertically centered).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Text color (default `#9AA0A6`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_color: Option<String>,
        /// Text size in output px at full resolution (default 60 % of the region height).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_size: Option<f64>,
        #[serde(flatten)]
        at: Placement,
    },
    /// Outline, fill or underline a region to show where to look.
    Highlight {
        /// Region as fractions of the frame: `{x, y, w, h}`.
        rect: Rect,
        /// outline (default) | fill | underline
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<HighlightStyle>,
        /// `brand` (the accent, default) or a color.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        /// Stroke width in output px (default 4 at 1080p).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f64>,
        /// Corner radius in output px (default 8 at 1080p).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f64>,
        /// draw (default: the stroke draws itself) | fade | none
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enter: Option<AnnotEnter>,
        /// Move with the zoom of the clip underneath (default true).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follow: Option<bool>,
        #[serde(flatten)]
        at: Placement,
    },
    /// Dim everything except a region. Spotlights on screen together share one dim layer.
    Spotlight {
        rect: Rect,
        /// How dark the rest gets, 0–1 (default 0.6).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dim: Option<f64>,
        /// Soft edge in output px (default 12 at 1080p).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feather: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follow: Option<bool>,
        #[serde(flatten)]
        at: Placement,
    },
    /// Arrow from one point to another (fractions of the frame), with an optional label at
    /// its tail.
    Arrow {
        from: Point,
        to: Point,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        /// Shaft width in output px (default 6 at 1080p).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f64>,
        /// Label size in output px (default 34 at 1080p).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follow: Option<bool>,
        #[serde(flatten)]
        at: Placement,
    },
    /// Keycap badges for a shortcut: `keys: ["cmd", "K"]`.
    Keys {
        keys: Vec<String>,
        /// Default `bottom`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<Position>,
        /// Keycap height in output px (default 64 at 1080p).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f64>,
        #[serde(flatten)]
        at: Placement,
    },
    /// A drawn mouse pointer moving along `path`, with a ripple on clicks. Use it over
    /// recordings made without a cursor.
    Cursor {
        /// Keys: `t` seconds after the layer starts, `x`/`y` fractions of the frame.
        path: Vec<CursorKey>,
        /// Pointer size multiplier (default 1 = 28 px tall at 1080p).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f64>,
        /// Ripple on clicks (default true).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        click_ripple: Option<bool>,
        /// Ripple color: `brand` (default) or a color.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follow: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generated_by: Option<String>,
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
            Layer::Mask { .. } => "mask",
            Layer::Sfx { .. } => "sfx",
            Layer::Highlight { .. } => "highlight",
            Layer::Spotlight { .. } => "spotlight",
            Layer::Arrow { .. } => "arrow",
            Layer::Keys { .. } => "keys",
            Layer::Cursor { .. } => "cursor",
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
            | Layer::Mask { at, .. }
            | Layer::Highlight { at, .. }
            | Layer::Spotlight { at, .. }
            | Layer::Arrow { at, .. }
            | Layer::Keys { at, .. }
            | Layer::Cursor { at, .. }
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
    /// A text layer using a business card template (`title_card`, `feature_list`, …).
    pub fn is_card(&self) -> bool {
        matches!(self, Layer::Text { template, .. } if crate::catalog::CARD_TEMPLATES.contains(&template.as_str()))
    }
    /// highlight / spotlight / arrow / keys / cursor: drawn together in the annotation pass.
    pub fn is_annotation(&self) -> bool {
        matches!(
            self,
            Layer::Highlight { .. }
                | Layer::Spotlight { .. }
                | Layer::Arrow { .. }
                | Layer::Keys { .. }
                | Layer::Cursor { .. }
        )
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
pub enum HighlightStyle {
    #[default]
    Outline,
    Fill,
    Underline,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnnotEnter {
    #[default]
    Draw,
    Fade,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CursorKey {
    /// Seconds after the layer starts.
    pub t: f64,
    pub x: f64,
    pub y: f64,
    /// A click happens when the pointer arrives here.
    #[serde(default, skip_serializing_if = "is_false")]
    pub click: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MaskMode {
    #[default]
    Blur,
    Pixelate,
    Fill,
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
    /// How far the music sits below the voice, in LU, while ducked (e.g. 18). The music gain
    /// is measured and set at render time, replacing trial-and-error with `gain_db`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music_level: Option<f64>,
}

/// How an audio layer's asset lines up with the layer span.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioAlign {
    /// Backtime: the asset's own ending lands on the layer's end (the start is trimmed, or
    /// the layer starts later when the asset is shorter).
    End,
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
    /// Move cuts onto the music's beats (fitting each clip with `end_at`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snap_to_beats: Option<SnapToBeats>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SnapToBeats {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Asset whose beats are used (default: the only music-role audio layer's asset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// Largest move of a cut, in seconds (default 0.15). Cuts farther from a beat stay put.
    #[serde(default = "snap_shift")]
    pub max_shift: f64,
    /// A cut within this many seconds of the first beat of a bar goes there rather than to a
    /// nearer weak beat (default 0.35; 0 treats all beats alike). Used when the bars are clear.
    #[serde(default = "snap_downbeat")]
    pub downbeat_reach: f64,
}
fn snap_shift() -> f64 {
    0.15
}
fn snap_downbeat() -> f64 {
    0.35
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
    /// Default: the project's target (and size).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<Fps>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crf: Option<u32>,
    /// Overrides the project's `chroma` for this export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chroma: Option<Chroma>,
    /// Output path (relative to the project). Default `out/<name>.mp4` (or `.webm`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
    /// `mp4` (H.264 + AAC, default) or `webm` (VP9 + Opus).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ExportFormat>,
    /// `false`: no audio track (silent autoplay embeds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
    /// `true`: make the video loop seamlessly (its end dissolves into its start; the export is
    /// half a second shorter). Needs `audio: false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#loop: Option<bool>,
    /// Largest file size, e.g. `"8MB"`, `"900KB"` or bytes. The bitrate is solved for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<ByteSize>,
    /// Export only this part of the edit: `[from, to]` (times on the edit; cues allowed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[TimeValue; 2]>,
    /// `burn` (default: captions in the picture), `sidecar` (only the .srt/.vtt files) or `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captions: Option<CaptionDelivery>,
    /// Time on the edit of the poster frame written next to the video (default 0.5 s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<TimeValue>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    #[default]
    Mp4,
    Webm,
}

impl ExportFormat {
    pub fn ext(&self) -> &'static str {
        match self {
            ExportFormat::Mp4 => "mp4",
            ExportFormat::Webm => "webm",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptionDelivery {
    #[default]
    Burn,
    Sidecar,
    None,
}

/// A size in bytes: a number, or a string like `"8MB"`, `"900KB"`, `"1.5GB"` (powers of 1000).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum ByteSize {
    Bytes(u64),
    Text(String),
}

impl ByteSize {
    pub fn bytes(&self) -> Option<u64> {
        match self {
            ByteSize::Bytes(b) => Some(*b),
            ByteSize::Text(t) => {
                let t = t.trim().to_ascii_uppercase().replace(' ', "");
                let (num, mul) = if let Some(n) = t.strip_suffix("GB") {
                    (n, 1e9)
                } else if let Some(n) = t.strip_suffix("MB") {
                    (n, 1e6)
                } else if let Some(n) = t.strip_suffix("KB") {
                    (n, 1e3)
                } else if let Some(n) = t.strip_suffix('B') {
                    (n, 1.0)
                } else {
                    (t.as_str(), 1.0)
                };
                num.parse::<f64>().ok().filter(|v| *v > 0.0).map(|v| (v * mul) as u64)
            }
        }
    }
}

impl Project {
    pub fn clips(&self) -> impl Iterator<Item = &Clip> {
        self.main.iter().filter_map(|m| match m {
            MainItem::Clip(c) => Some(c.as_ref()),
            _ => None,
        })
    }
}
