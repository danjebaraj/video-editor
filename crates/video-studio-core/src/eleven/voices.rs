//! Choosing a voice when none is fixed: Settings › ElevenLabs says whether to prefer voices that
//! earlier videos already used or the newest ones, and whether the voice should be female or
//! male. `video-studio eleven voices` lists the account's voices in that order, so a run takes
//! one from the top instead of whichever the API happened to return first.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::VoiceInfo;
use crate::{paths, timeutil};

/// Which voices come first when a run has to choose one.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoicePreference {
    /// Voices earlier videos used (most used first), then the longest-standing ones.
    #[default]
    Tried,
    /// The most recently added voices first.
    Latest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoiceGender {
    #[default]
    Any,
    Female,
    Male,
}

impl VoicePreference {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "tried" => Some(Self::Tried),
            "latest" | "newest" => Some(Self::Latest),
            _ => None,
        }
    }
    /// How the list is ordered, in words.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Tried => "voices used in earlier videos first (most used at the top), then the longest-standing ones",
            Self::Latest => "the most recently added voices first",
        }
    }
}

impl VoiceGender {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "any" | "" => Some(Self::Any),
            "female" | "f" | "woman" => Some(Self::Female),
            "male" | "m" | "man" => Some(Self::Male),
            _ => None,
        }
    }
    pub fn word(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Female => Some("female"),
            Self::Male => Some("male"),
        }
    }
}

/// How often a voice was used for a voice-over, and when last.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct VoiceUse {
    pub count: u64,
    pub last: String,
}

/// The record of voices used, kept beside the audio cache.
pub fn used_path(cache_dir: &Utf8Path) -> Utf8PathBuf {
    cache_dir.with_file_name("eleven-voices-used.json")
}

pub fn used(cache_dir: &Utf8Path) -> BTreeMap<String, VoiceUse> {
    paths::read_json(&used_path(cache_dir)).unwrap_or_default()
}

/// Counts one voice-over made with `voice`. Failing to write is not worth failing the call for.
pub fn note_used(cache_dir: &Utf8Path, voice: &str) {
    let voice = voice.trim();
    if voice.is_empty() {
        return;
    }
    let mut all = used(cache_dir);
    let e = all.entry(voice.to_string()).or_default();
    e.count += 1;
    e.last = timeutil::now_rfc3339();
    if let Some(dir) = used_path(cache_dir).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = paths::write_json_atomic(&used_path(cache_dir), &all);
}

/// A voice's gender as its labels give it, lowercase.
pub fn gender_of(v: &VoiceInfo) -> Option<String> {
    v.labels.get("gender").map(|g| g.trim().to_ascii_lowercase()).filter(|g| !g.is_empty())
}

/// The voices that fit `gender`, best first for `prefer`. A voice whose gender isn't labelled
/// can't be judged, so it stays in the list, after the ones that are known to fit.
pub fn rank(
    voices: Vec<VoiceInfo>,
    prefer: VoicePreference,
    gender: VoiceGender,
    used: &BTreeMap<String, VoiceUse>,
) -> Vec<VoiceInfo> {
    let mut list: Vec<(bool, VoiceInfo)> = voices
        .into_iter()
        .filter_map(|v| match (gender.word(), gender_of(&v)) {
            (None, _) => Some((true, v)),
            (Some(want), Some(have)) => (want == have).then_some((true, v)),
            (Some(_), None) => Some((false, v)),
        })
        .collect();
    list.sort_by(|(fa, a), (fb, b)| {
        fb.cmp(fa)
            .then_with(|| match prefer {
                VoicePreference::Latest => b.created_at_unix.unwrap_or(0).cmp(&a.created_at_unix.unwrap_or(0)),
                VoicePreference::Tried => {
                    let (ua, ub) = (used.get(&a.voice_id), used.get(&b.voice_id));
                    let key = |u: Option<&VoiceUse>| u.map(|u| (u.count, u.last.clone())).unwrap_or_default();
                    key(ub)
                        .cmp(&key(ua))
                        .then_with(|| a.created_at_unix.unwrap_or(0).cmp(&b.created_at_unix.unwrap_or(0)))
                }
            })
            .then_with(|| a.name.cmp(&b.name))
    });
    list.into_iter().map(|(_, v)| v).collect()
}

/// What a run is told when no voice is fixed.
pub fn choosing(prefer: VoicePreference, gender: VoiceGender) -> String {
    let who = match gender.word() {
        Some(g) => format!("a {g} voice"),
        None => "a voice of either gender".to_string(),
    };
    format!(
        "Settings › ElevenLabs asks for {who}, and for {}. `video-studio eleven voices` lists the voices that way, best first: take the first one whose description fits the tone of the video (add `--search <words>` to narrow the list), and name it in summary.md.",
        prefer.describe()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(id: &str, gender: Option<&str>, created: Option<i64>) -> VoiceInfo {
        VoiceInfo {
            voice_id: id.into(),
            name: id.to_uppercase(),
            category: None,
            description: None,
            labels: gender.map(|g| ("gender".to_string(), g.to_string())).into_iter().collect(),
            created_at_unix: created,
        }
    }

    fn ids(l: &[VoiceInfo]) -> Vec<&str> {
        l.iter().map(|v| v.voice_id.as_str()).collect()
    }

    #[test]
    fn tried_voices_come_first_then_the_long_standing_ones() {
        let all = vec![
            v("new", Some("female"), Some(300)),
            v("old", Some("Female"), None),
            v("mid", Some("male"), Some(200)),
        ];
        let mut used = BTreeMap::new();
        used.insert("mid".to_string(), VoiceUse { count: 2, last: "2026-11-01T00:00:00Z".into() });
        let r = rank(all.clone(), VoicePreference::Tried, VoiceGender::Any, &used);
        assert_eq!(ids(&r), ["mid", "old", "new"]);
        let r = rank(all.clone(), VoicePreference::Latest, VoiceGender::Any, &used);
        assert_eq!(ids(&r), ["new", "mid", "old"]);
        // gender: only matching voices, whatever was used before
        let r = rank(all, VoicePreference::Tried, VoiceGender::Female, &used);
        assert_eq!(ids(&r), ["old", "new"]);
    }

    #[test]
    fn a_voice_without_a_gender_label_stays_at_the_end() {
        let all =
            vec![v("unknown", None, Some(900)), v("m", Some("male"), Some(100)), v("f", Some("female"), Some(50))];
        let r = rank(all, VoicePreference::Latest, VoiceGender::Male, &BTreeMap::new());
        assert_eq!(ids(&r), ["m", "unknown"]);
    }

    #[test]
    fn uses_are_counted_beside_the_cache() {
        let d = tempfile::tempdir().unwrap();
        let cache = Utf8PathBuf::from_path_buf(d.path().join("cache").join("eleven")).unwrap();
        note_used(&cache, "abc");
        note_used(&cache, "abc");
        note_used(&cache, " ");
        let u = used(&cache);
        assert_eq!((u.len(), u["abc"].count), (1, 2));
        assert!(choosing(VoicePreference::Latest, VoiceGender::Female).contains("a female voice"));
        assert_eq!(VoicePreference::parse("Latest"), Some(VoicePreference::Latest));
        assert_eq!(VoiceGender::parse("nope"), None);
    }
}
