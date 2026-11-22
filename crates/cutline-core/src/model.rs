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
    /// Named cuts of the edit (`cutline render --variant teaser`); see `Variant`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variants: BTreeMap<String, Variant>,
    /// Languages (`cutline render --locale de`); see `Locale`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub locales: BTreeMap<String, Locale>,
    /// Strings that text can refer to as `"@key"` (the base language).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub strings: BTreeMap<String, String>,
    /// Review settings (text recognition for the demo checks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qa: Option<QaSettings>,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct QaSettings {
    /// Text recognition engine for `cutline review`: `auto` (default: macOS Vision on macOS,
    /// Tesseract elsewhere), `vision`, `tesseract` or `off`. The `CUTLINE_OCR` environment
    /// variable and `review --ocr` override it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr: Option<OcrEngine>,
    /// Smallest text height that counts as readable, in output px (default 2 % of the frame's
    /// shorter side; 2.4 % on vertical targets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_text_px: Option<f64>,
    /// Seconds with nothing said and nothing pointed at before review suggests tightening
    /// (default 8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_quiet: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OcrEngine {
    #[default]
    Auto,
    Vision,
    Tesseract,
    Off,
}

/// A cut of the edit. Clips and layers also choose their variants with `only` / `except`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    /// Clip ids to keep (default: all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Clip ids to leave out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Warn when this variant runs longer (seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<f64>,
    /// Export names to render for this variant (default: all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<String>,
    /// Asset fields to replace, by asset id: `{vo: {path: "vo.teaser.mp3", script: "vo.teaser.txt"}}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, serde_json::Value>,
}

/// A language of the video.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Locale {
    /// Asset fields to replace, by asset id (voice-over, its script and transcript, recordings).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, serde_json::Value>,
    /// Strings for `"@key"` text: a JSON5 file path, or the table itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strings: Option<serde_json::Value>,
    /// Cue phrases in this language, by cue name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cues: BTreeMap<String, Cue>,
    /// Language code for transcription and browser recordings (e.g. `de`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
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
    /// Pauses to add to a narration: silence inserted at a word boundary found by phrase in
    /// the asset's transcript, before cues are resolved. See `NarrationGap`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<NarrationGap>,
}

/// Silence added to a narration asset at a word boundary: `{before: "Need more room", add:
/// 0.7}` lengthens the pause in front of that phrase, `{after: "…", add: 0.3}` the pause
/// behind it. The phrase is looked up in the asset's transcript (its `script` wording when it
/// has one), so the gap stays on its words when the narration file is generated again.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NarrationGap {
    /// Add the silence in front of this phrase's first word: a phrase, or `"cue:name"` for a
    /// cue's phrase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// Add the silence behind this phrase's last word: a phrase, or `"cue:name"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Seconds of silence to add (above 0, at most 10).
    pub add: f64,
    /// Which occurrence of the phrase (default 1; a cue brings its own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<usize>,
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
    /// Constant speed factor 0.25–8.
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
    /// Variants this clip belongs to (default: all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
    /// Variants this clip is left out of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<String>,
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
    /// The crop of the target's aspect ratio around `rect` (fractions of the source).
    Rect,
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
    /// fit_pad color (default brand background); also fills the bars of a `rect` reframe
    /// whose rect is larger than any crop of the target's aspect ratio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad_color: Option<String>,
    /// `rect` mode: the part of the source to show, in fractions of the source (the same
    /// fractions `zoom_to` and annotations use).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
    /// `rect` mode: margin around the rect as a fraction of the source (default 0.03).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<f64>,
    /// `rect` mode: crop only in an output that is narrower than the source (4:5, 9:16). In an
    /// output as wide as the source or wider the clip is shown whole, as if it had no
    /// `reframe`, and takes the project's `default_frame`. For one project that exports both a
    /// 16:9 and vertical versions of a screen recording.
    #[serde(default, skip_serializing_if = "is_false")]
    pub narrower_only: bool,
}

/// The crop a `rect` reframe makes: a whole-pixel crop of the source and where it lands in
/// the output (the whole frame, unless the rect is too large for the target's aspect ratio).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectCrop {
    /// Crop of the source in pixels (even numbers): x, y, w, h.
    pub crop: (u32, u32, u32, u32),
    /// Where the crop is drawn, in fractions of the output frame.
    pub place: Rect,
}

impl RectCrop {
    /// Where the *whole* source picture sits in fractions of the output frame (it extends past
    /// the frame where it is cropped). Rects written in fractions of the source are mapped
    /// through this, like the content rect of a screen frame.
    pub fn content(&self, src: (u32, u32)) -> Rect {
        let (cx, cy, cw, ch) = self.crop;
        let (kx, ky) = (self.place.w / cw.max(1) as f64, self.place.h / ch.max(1) as f64);
        Rect {
            x: self.place.x - cx as f64 * kx,
            y: self.place.y - cy as f64 * ky,
            w: src.0 as f64 * kx,
            h: src.1 as f64 * ky,
        }
    }
}

/// The crop for `reframe: {mode: "rect"}`: the smallest window of the target's `aspect`
/// (width / height) that contains `rect` plus `padding` on each side, centred on the rect and
/// shifted to stay inside the `src` (w, h) picture.
///
/// When that window does not fit inside the source, the padding is given up first: the window
/// becomes the largest crop the source allows, as long as that still contains the bare rect.
/// When even the bare rect is too wide (or too tall) for a crop of this aspect ratio, the
/// window covers the rect and runs past the source on the other axis: the source is shown
/// whole along that axis, centred, with bars in the pad color.
pub fn rect_reframe(rect: &Rect, padding: f64, src: (u32, u32), aspect: f64) -> RectCrop {
    let (sw, sh) = (src.0.max(2) as f64, src.1.max(2) as f64);
    let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
    // smallest window of the aspect covering a (w, h) box, by its width
    let cover = |w: f64, h: f64| w.max(h * aspect);
    let x0 = (rect.x.clamp(0.0, 1.0)) * sw;
    let y0 = (rect.y.clamp(0.0, 1.0)) * sh;
    let x1 = ((rect.x + rect.w).clamp(0.0, 1.0) * sw).max(x0 + 1.0).min(sw);
    let y1 = ((rect.y + rect.h).clamp(0.0, 1.0) * sh).max(y0 + 1.0).min(sh);
    let pad = padding.clamp(0.0, 0.4);
    let (px0, px1) = ((x0 - pad * sw).max(0.0), (x1 + pad * sw).min(sw));
    let (py0, py1) = ((y0 - pad * sh).max(0.0), (y1 + pad * sh).min(sh));
    let fit = sw.min(sh * aspect);
    let padded = cover(px1 - px0, py1 - py0);
    let w = if padded <= fit + 1e-6 { padded } else { cover(x1 - x0, y1 - y0).max(fit) };
    let h = w / aspect;
    let even = |v: f64, max: f64| -> u32 { ((((v / 2.0).round() * 2.0).max(2.0)) as u32).min((max as u32) & !1) };
    // one axis: crop start and size in source px, place start and size in output fractions
    let axis = |size: f64, full: f64, centre: f64| -> (u32, u32, f64, f64) {
        if size <= full + 1e-6 {
            let c = even(size, full);
            let start = (centre - c as f64 / 2.0).clamp(0.0, full - c as f64);
            // an even start keeps 4:2:0 chroma aligned
            let start = (((start / 2.0).round() * 2.0) as u32).min(full as u32 - c);
            (start, c, 0.0, 1.0)
        } else {
            (0, full as u32, (1.0 - full / size) / 2.0, full / size)
        }
    };
    let (cx, cw, ox, ow) = axis(w, sw, (x0 + x1) / 2.0);
    let (cy, ch, oy, oh) = axis(h, sh, (y0 + y1) / 2.0);
    RectCrop { crop: (cx, cy, cw, ch), place: Rect { x: ox, y: oy, w: ow, h: oh } }
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
        #[serde(default)]
        at: f64,
        /// Instead of `at`: the moment in the asset (source seconds), mapped through the
        /// clip's `in` and speed, so the effect stays on it when the clip is retimed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_at: Option<f64>,
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
        /// Instead of `start` / `end`: source seconds of the asset (see `punch_in.src_at`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_start: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_end: Option<f64>,
    },
    /// Zoom so `rect` (fractions of the frame) fills the view, starting at `start`
    /// (clip-relative) over `duration` (0 = at once); zoom back out from `end` if set.
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
        /// Instead of `start`: the moment in the asset (source seconds) the zoom starts,
        /// mapped through the clip's `in` and speed (`speed`, `fit: "speed"`, `end_at`), so
        /// the zoom stays on that moment of the footage when the clip is retimed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_start: Option<f64>,
        /// Instead of `end`: source seconds at which to start zooming back out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_end: Option<f64>,
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
        #[serde(default)]
        at: f64,
        #[serde(default = "shake_dur")]
        duration: f64,
        #[serde(default = "shake_int")]
        intensity: f64,
        /// Instead of `at`: source seconds of the asset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_at: Option<f64>,
    },
    Flash {
        #[serde(default)]
        at: f64,
        #[serde(default = "flash_dur")]
        duration: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        /// Instead of `at`: source seconds of the asset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_at: Option<f64>,
    },
    BlurIn {
        #[serde(default = "blur_dur")]
        duration: f64,
    },
    Glitch {
        #[serde(default)]
        at: f64,
        #[serde(default = "flash_dur")]
        duration: f64,
        #[serde(default = "glitch_amt")]
        amount: f64,
        /// Instead of `at`: source seconds of the asset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_at: Option<f64>,
    },
    Vignette {
        #[serde(default = "vig")]
        strength: f64,
    },
    MotionBlur {
        #[serde(default)]
        at: f64,
        #[serde(default = "flash_dur")]
        duration: f64,
        /// Instead of `at`: source seconds of the asset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_at: Option<f64>,
    },
    /// Hold the frame at `at` (clip-relative) for `duration` seconds (extends the clip).
    Freeze { at: f64, duration: f64 },
}

impl Effect {
    /// The source time (seconds in the asset) this effect is pinned to, when it is written
    /// with `src_at` / `src_start` instead of a clip-relative time.
    pub fn src_anchor(&self) -> Option<f64> {
        match self {
            Effect::PunchIn { src_at, .. }
            | Effect::Shake { src_at, .. }
            | Effect::Flash { src_at, .. }
            | Effect::Glitch { src_at, .. }
            | Effect::MotionBlur { src_at, .. } => *src_at,
            Effect::Zoom { src_start, src_end, .. } | Effect::ZoomTo { src_start, src_end, .. } => {
                src_start.or(*src_end)
            }
            _ => None,
        }
    }

    /// The effect with its source-relative times turned into clip-relative ones by `map`
    /// (source seconds → seconds from the clip's start on the timeline). Effects without
    /// source times come back unchanged.
    pub fn at_clip_time(&self, map: impl Fn(f64) -> f64) -> Effect {
        let r = |v: f64| crate::time::round6(map(v));
        let mut e = self.clone();
        match &mut e {
            Effect::PunchIn { at, src_at, .. }
            | Effect::Shake { at, src_at, .. }
            | Effect::Flash { at, src_at, .. }
            | Effect::Glitch { at, src_at, .. }
            | Effect::MotionBlur { at, src_at, .. } => {
                if let Some(s) = src_at.take() {
                    *at = r(s);
                }
            }
            Effect::Zoom { start, end, src_start, src_end, .. }
            | Effect::ZoomTo { start, end, src_start, src_end, .. } => {
                if let Some(s) = src_start.take() {
                    *start = r(s);
                }
                if let Some(s) = src_end.take() {
                    *end = Some(r(s));
                }
            }
            _ => {}
        }
        e
    }
}

/// A clip's speed for on-screen text (`{speed}` in a badge): one decimal, none when whole
/// (3.11 → "3.1", 3.0 → "3", 2.72 → "2.7"); two decimals below 1 (0.25 stays "0.25").
pub fn speed_text(speed: f64) -> String {
    let s = if speed < 0.95 { format!("{speed:.2}") } else { format!("{speed:.1}") };
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
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

/// Move a zoom's fixed point so the view at scale `s` stays inside `bounds` (the clip's
/// picture in fractions of the frame, e.g. the recording inside a screen frame): the view is
/// shifted inward, and where it is larger than the picture it is centred on it. The view
/// never leaves the frame either. Scales of 1 or less have no view to move.
pub fn clamp_zoom_focus(s: f64, focus: Point, bounds: &Rect) -> Point {
    if s.is_nan() || s <= 1.0 + 1e-9 {
        return focus;
    }
    let view = 1.0 / s;
    let axis = |f: f64, b0: f64, b1: f64| {
        // the visible part of the picture on this axis
        let (lo, hi) = (b0.max(0.0), b1.min(1.0));
        let start = (s - 1.0) * f / s;
        let start = if hi - lo >= view { start.clamp(lo, hi - view) } else { (lo + hi - view) / 2.0 };
        (start.clamp(0.0, 1.0 - view) * s / (s - 1.0)).clamp(0.0, 1.0)
    };
    Point { x: axis(focus.x, bounds.x, bounds.x + bounds.w), y: axis(focus.y, bounds.y, bounds.y + bounds.h) }
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
    /// Variants this layer belongs to (default: all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
    /// Variants this layer is left out of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<String>,
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
        /// Gain over time: `[{at, db}]`, `at` in seconds from the layer's start (looped
        /// sources included). Linear in dB between keys, the nearest key's value outside them,
        /// added to `gain_db` before ducking and loudness. For a swell or a dip under one line
        /// without splitting the layer.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        gain_keys: Vec<GainKey>,
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
        /// `true`: the rect is in fractions of the clip's picture and the mask moves with the
        /// clip's screen frame and zoom (like an annotation). Needs `mode: "fill"`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follow: Option<bool>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generated_by: Option<String>,
        #[serde(flatten)]
        at: Placement,
    },
    /// The presenter's camera over the screen: a bubble, a side panel or the full frame,
    /// switchable over time with `layouts`.
    Presenter {
        asset: String,
        /// Camera time shown when the layer starts (to line the camera up with the screen).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_in: Option<TimeValue>,
        /// circle (default) | rounded
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shape: Option<PresenterShape>,
        /// Bubble position (default `bottom_right`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<Position>,
        /// Bubble width as a fraction of the frame width (default 0.18; 0.3 on vertical targets).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f64>,
        /// Where the face is in the camera picture (fractions). Default: detected, else the
        /// middle of the upper half.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<Point>,
        /// Border width in output px (default 4) and color (default white).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        border: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        border_color: Option<String>,
        /// Drop shadow (default true).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shadow: Option<bool>,
        /// Use the camera's sound as dialogue (default false).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gain_db: Option<f64>,
        /// Layout changes: `[{at: 0, layout: "bubble"}, {at: "cue:intro", layout: "presenter_full"}]`.
        /// Default: `bubble` throughout.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        layouts: Vec<LayoutKey>,
        #[serde(flatten)]
        at: Placement,
    },
    /// A small pill with a short text, e.g. the speed while fast-forwarding ("4×").
    Badge {
        text: String,
        /// Default `top_right`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<Position>,
        /// Text size in output px (default 30 at 1080p).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generated_by: Option<String>,
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        only: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        except: Vec<String>,
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
            Layer::Badge { .. } => "badge",
            Layer::Presenter { .. } => "presenter",
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
            | Layer::Badge { at, .. }
            | Layer::Presenter { at, .. }
            | Layer::Audio { at, .. } => Some(at),
            Layer::Sfx { .. } => None,
        }
    }
    pub fn placement_mut(&mut self) -> Option<&mut Placement> {
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
            | Layer::Badge { at, .. }
            | Layer::Presenter { at, .. }
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
    /// highlight / spotlight / arrow / keys / cursor (and masks that follow their clip): drawn
    /// together in the annotation pass.
    pub fn is_annotation(&self) -> bool {
        matches!(
            self,
            Layer::Mask { follow: Some(true), .. }
                | Layer::Highlight { .. }
                | Layer::Spotlight { .. }
                | Layer::Arrow { .. }
                | Layer::Keys { .. }
                | Layer::Cursor { .. }
                | Layer::Badge { .. }
        )
    }
    pub fn asset(&self) -> Option<&str> {
        match self {
            Layer::Video { asset, .. }
            | Layer::Broll { asset, .. }
            | Layer::Image { asset, .. }
            | Layer::Html { asset, .. }
            | Layer::Presenter { asset, .. }
            | Layer::Audio { asset, .. }
            | Layer::Sfx { asset, .. } => Some(asset),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PresenterShape {
    #[default]
    Circle,
    Rounded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PresenterLayout {
    /// Small camera picture in a corner.
    #[default]
    Bubble,
    /// The camera as a tall panel over the right third of the frame.
    SideBySide,
    /// The camera fills the frame.
    PresenterFull,
    /// The camera is hidden.
    ScreenFull,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutKey {
    /// Time on the edit from which this layout shows.
    pub at: TimeValue,
    pub layout: PresenterLayout,
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
    /// Bring every voice-over file to the same loudness before the voice chain, so passages
    /// recorded or generated one at a time don't jump in volume (default true; needs two or
    /// more voice files to do anything). `false` keeps each file's own level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level_voice: Option<bool>,
    /// The check that compares what was heard in a narration with its `script`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_check: Option<ScriptCheck>,
}

/// Settings of the script check: every asset with a `script` has its recognised words
/// compared with the script, and each place where they differ is a `W_SCRIPT_MISMATCH`
/// warning (a mispronounced or skipped word in a generated voice-over).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScriptCheck {
    /// `false` turns the check off.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// How alike (0–1, by letters) the script and the heard words must be to pass (default
    /// 0.8). Words that sound alike ("panes"/"pains", "Claude"/"Cloud") pass from 0.6. `1`
    /// reports every difference in letters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f64>,
    /// Report only differences at least this many words long, counted on the longer side
    /// (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_span_words: Option<usize>,
    /// Script words or phrases never reported (e.g. a name the recogniser cannot spell).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
}

impl Default for ScriptCheck {
    fn default() -> Self {
        Self { enabled: true, similarity: None, min_span_words: None, ignore: vec![] }
    }
}

/// One point of an audio layer's gain curve.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GainKey {
    /// Seconds from the layer's start.
    pub at: f64,
    /// Gain in dB at that time, added to the layer's `gain_db`.
    pub db: f64,
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
    /// Pauses in the speech shorter than this many seconds stay ducked (default 1.5), so the
    /// music does not swell and drop between sentences. `0`: only pauses shorter than
    /// `attack + release` stay ducked.
    #[serde(default = "duck_hold")]
    pub hold: f64,
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
    0.25
}
fn duck_rel() -> f64 {
    0.8
}
fn duck_hold() -> f64 {
    1.5
}
impl Default for Duck {
    fn default() -> Self {
        Self {
            under: duck_under(),
            target: duck_target(),
            amount_db: -12.0,
            attack: duck_att(),
            release: duck_rel(),
            hold: duck_hold(),
            mode: None,
        }
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
    /// Browser recordings: zoom in on each click and follow from one click to the next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom_on_clicks: Option<ZoomOnClicks>,
    /// Browser recordings: draw the mouse pointer (with click ripples) and shortcut badges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<AutoCursor>,
    /// Play stretches where little changes on screen and nobody speaks at a higher speed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_forward_idle: Option<FastForwardIdle>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ZoomOnClicks {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Strongest zoom (default 2; 1.2–3).
    #[serde(default = "zoc_scale")]
    pub max_scale: f64,
    /// Clicks closer together than this many seconds share one zoom (default 2.5).
    #[serde(default = "zoc_merge")]
    pub merge_within: f64,
    /// Seconds the zoom stays after the last click of a group (default 1.2).
    #[serde(default = "zoc_hold")]
    pub hold: f64,
    /// Zoom in/out time (default 0.6).
    #[serde(default = "zoom_to_dur")]
    pub duration: f64,
    /// Only these step names (default: every click and typed field).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
}
fn zoc_scale() -> f64 {
    2.0
}
fn zoc_merge() -> f64 {
    2.5
}
fn zoc_hold() -> f64 {
    1.2
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AutoCursor {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Pointer size multiplier (default 1.4).
    #[serde(default = "cursor_size")]
    pub size: f64,
    #[serde(default = "yes")]
    pub click_ripple: bool,
    /// Show a `keys` badge for shortcuts and Enter/Escape/Tab presses (default true).
    #[serde(default = "yes")]
    pub keys: bool,
}
fn cursor_size() -> f64 {
    1.4
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FastForwardIdle {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Shortest idle stretch worth speeding up, in seconds (default 2.5).
    #[serde(default = "ffi_min")]
    pub min_idle: f64,
    /// Playback speed of idle stretches (default 4; up to 8).
    #[serde(default = "ffi_speed")]
    pub speed: f64,
    /// Seconds kept at normal speed at each end of an idle stretch (default 0.4).
    #[serde(default = "ffi_keep")]
    pub keep: f64,
    /// Share of the picture that may change per tenth of a second and still count as idle
    /// (default 0.006: typing, a spinner or a blinking caret).
    #[serde(default = "ffi_activity")]
    pub activity: f64,
    /// Show a small speed badge ("4×") while fast-forwarding (default true).
    #[serde(default = "yes")]
    pub badge: bool,
    /// Asset ids (default: every video clip asset).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applies_to: Vec<String>,
}
fn ffi_min() -> f64 {
    2.5
}
fn ffi_speed() -> f64 {
    4.0
}
fn ffi_keep() -> f64 {
    0.4
}
fn ffi_activity() -> f64 {
    0.006
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
