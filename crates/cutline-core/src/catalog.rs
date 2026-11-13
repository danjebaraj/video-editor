//! Names that are part of the user-facing contract.

/// ffmpeg `xfade` transitions supported by name.
pub const XFADE_TRANSITIONS: &[&str] = &[
    "fade",
    "fadeblack",
    "fadewhite",
    "fadegrays",
    "dissolve",
    "wipeleft",
    "wiperight",
    "wipeup",
    "wipedown",
    "slideleft",
    "slideright",
    "slideup",
    "slidedown",
    "smoothleft",
    "smoothright",
    "smoothup",
    "smoothdown",
    "circleopen",
    "circleclose",
    "circlecrop",
    "rectcrop",
    "radial",
    "zoomin",
    "pixelize",
    "hblur",
    "distance",
    "vertopen",
    "vertclose",
    "horzopen",
    "horzclose",
    "diagtl",
    "diagtr",
    "diagbl",
    "diagbr",
    "hlslice",
    "hrslice",
    "vuslice",
    "vdslice",
    "squeezeh",
    "squeezev",
    "wipetl",
    "wipetr",
    "wipebl",
    "wipebr",
    "coverleft",
    "coverright",
    "coverup",
    "coverdown",
    "revealleft",
    "revealright",
    "revealup",
    "revealdown",
];

/// Custom composite transitions.
pub const CUSTOM_TRANSITIONS: &[&str] =
    &["whip_left", "whip_right", "whip_up", "whip_down", "zoom_punch", "flash_cut", "glitch", "cut"];

/// Transitions only present in newer ffmpeg (≥ 7.0); gated at render time by capability.
pub const XFADE_NEWER: &[&str] =
    &["coverleft", "coverright", "coverup", "coverdown", "revealleft", "revealright", "revealup", "revealdown"];

pub fn is_transition(name: &str) -> bool {
    XFADE_TRANSITIONS.contains(&name) || CUSTOM_TRANSITIONS.contains(&name)
}

pub const TEXT_TEMPLATES: &[&str] = &[
    "hook_title",
    "lower_third",
    "callout",
    "step_counter",
    "big_number",
    "quote",
    "cta_subscribe",
    "end_card",
    "emoji_pop",
    "hook_title_html",
    "lower_third_html",
    "end_card_html",
];

pub const CAPTION_STYLES: &[&str] = &["bold_pop", "karaoke", "word_by_word", "clean_sentence", "boxed"];

pub const VOICE_CHAINS: &[&str] = &["clean_voice", "podcast_warm", "raw", "arnndn"];

pub const COLOR_PRESETS: &[&str] = &["natural", "punchy", "warm_film", "cool_clean", "bw_contrast"];

pub const BUNDLED_LUTS: &[&str] = &["warm_film", "cool_clean", "punchy", "bw_contrast", "teal_orange"];

pub const SFX_ALIASES: &[&str] = &["whoosh", "pop", "ding", "riser"];

/// Default progress curve of a transition: moving pictures accelerate and settle
/// (`in_out_cubic`), blends ease gently (`in_out_quad`).
pub fn default_transition_ease(kind: &str) -> crate::model::Ease {
    use crate::model::Ease;
    let moving = ["slide", "cover", "reveal", "smooth", "whip_", "zoom", "squeeze"];
    if moving.iter().any(|m| kind.starts_with(m)) {
        Ease::InOutCubic
    } else {
        Ease::InOutQuad
    }
}
