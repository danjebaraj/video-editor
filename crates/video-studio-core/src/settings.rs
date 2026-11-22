//! App settings (spec §18), stored as `<library>/settings.json` and shared by the app and the CLI.

use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::{paths, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    // General
    /// Parent of run folders; empty means `~/Movies/Syncfusion Video Studio/`.
    pub default_output_folder: String,
    /// Runs of different projects allowed at once (renders are CPU-heavy).
    pub parallel_run_limit: u32,
    /// App-wide media search roots.
    pub media_roots: Vec<String>,
    pub notifications: bool,
    pub recent_output_folders: Vec<String>,
    // Claude
    /// Path to the `claude` CLI; empty means detect.
    pub claude_path: String,
    pub default_assistant_model: String,
    pub default_dry_run_model: String,
    pub default_production_model: String,
    pub effort: String,
    pub web_access_default: bool,
    /// Seconds a headless run waits on a question before Claude proceeds on its assumption.
    pub headless_question_timeout_s: u32,
    // Assistant provider (FEAT-spry-alpaca-5RQZ)
    /// Claude Code (default) or Codex App Server, for the assistant and runs. Settings saved
    /// before the choice existed have no value and mean Claude.
    pub provider: crate::codex::Provider,
    /// Path to the `codex` CLI; empty means PATH.
    pub codex_path: String,
    /// The Codex model; empty means Codex's own default. Claude's model choices are kept apart.
    pub codex_model: String,
    // cutline
    /// Path to the `cutline` binary; empty means PATH, then `bin/cutline` in `cutline_repo`.
    pub cutline_path: String,
    pub cutline_repo: String,
    /// `draft` or `final`.
    pub default_quality: String,
    // Voice
    /// Who speaks the voice-over: ElevenLabs or Cartesia.
    pub voice_provider: crate::eleven::VoiceProvider,
    /// The Cartesia voice used when the brand names none.
    pub cartesia_default_voice: String,
    // ElevenLabs
    pub eleven_default_voice: String,
    /// When no voice is fixed: prefer voices earlier videos used, or the newest ones.
    pub eleven_voice_preference: crate::eleven::VoicePreference,
    /// When no voice is fixed: any voice, or only female or male ones.
    pub eleven_voice_gender: crate::eleven::VoiceGender,
    pub eleven_default_credit_cap: u64,
    /// Cache size limit in bytes; the oldest entries go first when it is exceeded.
    pub eleven_cache_limit_bytes: u64,
    // Spec
    /// Reading speed used to check voice-over length.
    pub words_per_minute: u32,
    // Finish
    /// How videos are finished unless a project keeps its own choices: transitions, cuts on
    /// the beat, how the music comes in and goes out.
    pub finish: crate::finish::Finish,
    // Storage
    pub cleanup_older_than_days: u32,
    pub onboarding_done: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            default_output_folder: String::new(),
            parallel_run_limit: 1,
            media_roots: Vec::new(),
            notifications: true,
            recent_output_folders: Vec::new(),
            claude_path: String::new(),
            default_assistant_model: "sonnet".into(),
            default_dry_run_model: "haiku".into(),
            default_production_model: "opus".into(),
            effort: String::new(),
            web_access_default: false,
            headless_question_timeout_s: 0,
            provider: Default::default(),
            codex_path: String::new(),
            codex_model: String::new(),
            cutline_path: String::new(),
            cutline_repo: String::new(),
            default_quality: "final".into(),
            voice_provider: Default::default(),
            cartesia_default_voice: String::new(),
            eleven_default_voice: String::new(),
            eleven_voice_preference: Default::default(),
            eleven_voice_gender: Default::default(),
            eleven_default_credit_cap: 20_000,
            eleven_cache_limit_bytes: 2_000_000_000,
            words_per_minute: 150,
            finish: crate::finish::Finish::default(),
            cleanup_older_than_days: 30,
            onboarding_done: false,
        }
    }
}

impl Settings {
    pub fn load(library: &Utf8Path) -> Self {
        paths::read_json(&library.join("settings.json")).unwrap_or_default()
    }

    pub fn save(&self, library: &Utf8Path) -> Result<()> {
        paths::write_json_atomic(&library.join("settings.json"), self)
    }

    /// Remembers a folder for the output folder picker's recent list (newest first, at most 8).
    pub fn remember_output_folder(&mut self, folder: &str) {
        let f = folder.trim().to_string();
        if f.is_empty() {
            return;
        }
        self.recent_output_folders.retain(|x| x != &f);
        self.recent_output_folders.insert(0, f);
        self.recent_output_folders.truncate(8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let lib = Utf8Path::from_path(d.path()).unwrap();
        let mut s = Settings::load(lib);
        assert_eq!((s.parallel_run_limit, s.words_per_minute), (1, 150));
        s.claude_path = "/opt/claude".into();
        for i in 0..10 {
            s.remember_output_folder(&format!("/out/{i}"));
        }
        s.remember_output_folder("/out/5");
        assert_eq!(s.recent_output_folders.len(), 8);
        assert_eq!(s.recent_output_folders[0], "/out/5");
        s.save(lib).unwrap();
        assert_eq!(Settings::load(lib), s);
        // unknown and missing keys are tolerated
        std::fs::write(lib.join("settings.json"), "{\"claude_path\": \"x\", \"future\": 1}").unwrap();
        assert_eq!(Settings::load(lib).claude_path, "x");
    }

    /// FEAT-spry-alpaca-5RQZ: settings without a provider are Claude; a Codex choice and its
    /// model persist without touching Claude's model preferences.
    #[test]
    fn provider_defaults_to_claude_and_persists() {
        let d = tempfile::tempdir().unwrap();
        let lib = Utf8Path::from_path(d.path()).unwrap();
        std::fs::write(
            lib.join("settings.json"),
            "{\"claude_path\": \"/opt/claude\", \"default_assistant_model\": \"opus\"}",
        )
        .unwrap();
        let mut s = Settings::load(lib);
        assert_eq!(s.provider, crate::codex::Provider::Claude);
        s.provider = crate::codex::Provider::Codex;
        s.codex_model = "gpt-5.5-codex".into();
        s.save(lib).unwrap();
        let back = Settings::load(lib);
        assert_eq!((back.provider, back.codex_model.as_str()), (crate::codex::Provider::Codex, "gpt-5.5-codex"));
        assert_eq!(back.default_assistant_model, "opus", "Claude's model preference is kept");
        let mut claude_again = back.clone();
        claude_again.provider = crate::codex::Provider::Claude;
        assert_eq!(claude_again.default_assistant_model, "opus");
    }
}
