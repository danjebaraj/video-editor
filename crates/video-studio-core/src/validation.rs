//! Pre-submission validation (spec §11.1): static checks with no model calls. Each issue has a
//! severity and a link to the place to fix it. A run can't start with blockers.

use std::collections::HashSet;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::brand;
use crate::claude::locate::{self, ClaudeInfo, CutlineInfo, DoctorSummary};
use crate::media::{format_duration, MediaKind, MediaRole, MediaStatus, Resolution};
use crate::models::ModelList;
use crate::paths;
use crate::presets::{estimate_output_bytes, resolve_output, Platforms, ResolvedOutput};
use crate::project::{IntentKind, Project, RunConfig, RunKind};
use crate::run::readiness::ReadinessReport;
use crate::settings::Settings;
use crate::spec;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocker,
    Warning,
    Info,
}

/// Where to go to fix an issue.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FixLink {
    /// `spec`, `media`, `brand`, `outputs`, `run_config`, `project` or `settings`.
    pub kind: String,
    /// A media ID, output config ID, or settings pane. `update` on a `brand` link offers the
    /// library's newer copy of the preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// 1-based spec line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

impl FixLink {
    fn to(kind: &str) -> Option<FixLink> {
        Some(FixLink { kind: kind.into(), target: None, line: None })
    }
    fn target(kind: &str, target: &str) -> Option<FixLink> {
        Some(FixLink { kind: kind.into(), target: Some(target.into()), line: None })
    }
    fn line(line: usize) -> Option<FixLink> {
        Some(FixLink { kind: "spec".into(), target: None, line: Some(line) })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    /// Stable across runs of validation, so a warning can be acknowledged: `rule` or `rule:target`.
    pub id: String,
    pub rule: String,
    pub severity: Severity,
    /// `environment`, `media`, `spec`, `brand`, `outputs`, `budget` or `dry_run`.
    pub category: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixLink>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Estimates {
    /// Planned video length in seconds: the intent's target, else the sum of scene durations.
    pub duration_s: Option<f64>,
    pub voice_over_words: usize,
    pub eleven_characters: u64,
    pub eleven_credits: u64,
    pub eleven_cap: u64,
    pub eleven_remaining: Option<u64>,
    /// Approximate Claude cost range for the production model, in USD. Always labelled approximate.
    pub claude_usd_low: f64,
    pub claude_usd_high: f64,
    pub disk_bytes: u64,
    pub free_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub generated: String,
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_config: Option<String>,
    pub issues: Vec<Issue>,
    pub estimates: Estimates,
    pub blockers: usize,
    pub warnings: usize,
    pub infos: usize,
    /// Findings of the dry run, when one was made (spec §11.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<ReadinessReport>,
}

impl Report {
    pub fn recount(&mut self) {
        self.issues.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.category.cmp(&b.category)));
        self.blockers = self.issues.iter().filter(|i| i.severity == Severity::Blocker).count();
        self.warnings = self.issues.iter().filter(|i| i.severity == Severity::Warning).count();
        self.infos = self.issues.iter().filter(|i| i.severity == Severity::Info).count();
    }

    /// The findings as context for the spec assistant: attached to a message so Claude knows
    /// what validation reported without the user copying it over. `only` limits it to one
    /// issue ID. Empty when there is nothing to report.
    ///
    /// Findings are split by where they can be fixed, because the assistant edits spec.md
    /// only: a missing permission or an unset voice written into the spec as a note is worse
    /// than the finding itself.
    pub fn assistant_context(&self, only: Option<&str>) -> String {
        let line = |i: &Issue| {
            let sev = match i.severity {
                Severity::Blocker => "blocker",
                Severity::Warning => "warning",
                Severity::Info => "note",
            };
            let at = i.fix.as_ref().and_then(|f| f.line).map(|l| format!(" (spec.md line {l})")).unwrap_or_default();
            let detail = i
                .detail
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(|d| format!(" — {}", d.replace('\n', " ")))
                .unwrap_or_default();
            format!("- [{sev}] {}{at}{detail}\n", i.title.trim())
        };
        let in_spec = |i: &Issue| {
            i.category == "spec" || i.category == "dry_run" || i.fix.as_ref().is_some_and(|f| f.kind == "spec")
        };
        let chosen: Vec<&Issue> = self.issues.iter().filter(|i| only.map_or(true, |id| i.id == id)).collect();
        if chosen.is_empty() {
            return String::new();
        }
        let (spec, other): (Vec<&Issue>, Vec<&Issue>) = chosen.into_iter().partition(|i| in_spec(i));
        let mut s = format!(
            "Validation findings for this project{}. Line numbers refer to spec.md as it was then; if the text has moved, find it by what the finding says.\n",
            if self.generated.is_empty() { String::new() } else { format!(" (checked {})", self.generated) }
        );
        if !spec.is_empty() {
            s.push_str("\nAbout the spec (these can be fixed in spec.md):\n");
            spec.iter().for_each(|i| s.push_str(&line(i)));
        }
        if !other.is_empty() {
            s.push_str("\nOutside the spec (media, outputs, brand, settings, this computer). You can't fix these by editing spec.md and must not write them into it as notes; when one matters to what I asked, tell me what to change and where:\n");
            other.iter().for_each(|i| s.push_str(&line(i)));
        }
        s
    }

    /// The message behind "Ask Claude to Fix": the findings themselves travel as context.
    pub fn fix_request(&self, only: Option<&str>) -> String {
        match only.and_then(|id| self.issues.iter().find(|i| i.id == id)) {
            Some(i) => format!(
                "Fix this validation finding: \"{}\". If it can't be fixed in the spec, tell me what to change and where. Ask me about anything only I can decide.",
                i.title.trim()
            ),
            None => "Fix the validation findings that can be fixed in the spec, blockers first, and leave the rest of the spec as it is. For the findings outside the spec, tell me what to change and where. Ask me about anything only I can decide.".into(),
        }
    }

    pub fn has_blockers(&self) -> bool {
        self.blockers > 0
    }

    /// Warnings not in `acknowledged` (issue IDs).
    pub fn unacknowledged_warnings(&self, acknowledged: &[String]) -> Vec<&Issue> {
        self.issues.iter().filter(|i| i.severity == Severity::Warning && !acknowledged.contains(&i.id)).collect()
    }

    /// Adds dry run findings as warnings linked to spec lines (spec §11.2).
    pub fn add_dry_run(&mut self, readiness: &ReadinessReport) {
        self.issues.retain(|i| i.category != "dry_run");
        for (n, f) in readiness.findings().into_iter().enumerate() {
            self.issues.push(Issue {
                id: format!("dry_run:{}:{n}", f.0),
                rule: format!("dry_run_{}", f.0),
                severity: Severity::Warning,
                category: "dry_run".into(),
                title: f.1,
                detail: f.2,
                fix: match f.3 {
                    Some(line) => FixLink::line(line),
                    None => FixLink::to("spec"),
                },
            });
        }
        self.dry_run = Some(readiness.clone());
        self.recount();
    }
}

/// What validation needs to know about the machine. The real implementation runs the tools;
/// tests supply a fake.
pub trait EnvProbe {
    fn claude(&self) -> Option<ClaudeInfo>;
    fn claude_signed_in(&self, info: &ClaudeInfo) -> Option<bool>;
    fn cutline(&self) -> Option<CutlineInfo>;
    fn doctor(&self, cutline: &CutlineInfo) -> Option<DoctorSummary>;
    fn free_space(&self, path: &Utf8Path) -> Option<u64>;
    fn is_writable(&self, path: &Utf8Path) -> bool;
    fn eleven_key_present(&self) -> bool;
    /// Remaining ElevenLabs credits, when the key is present and the API answers.
    fn eleven_remaining(&self) -> Option<u64>;
    /// Whether this computer lets the app record the screen and control other apps.
    fn capture_permissions(&self) -> Option<crate::capture::Permissions> {
        None
    }
}

/// The real machine.
pub struct SystemEnv<'a> {
    pub settings: &'a Settings,
    pub secrets: &'a dyn crate::platform::Secrets,
    /// Overrides the ElevenLabs API base URL (tests).
    pub eleven_base_url: Option<String>,
}

impl EnvProbe for SystemEnv<'_> {
    fn claude(&self) -> Option<ClaudeInfo> {
        locate::find_claude(&self.settings.claude_path)
    }
    fn claude_signed_in(&self, info: &ClaudeInfo) -> Option<bool> {
        locate::claude_signed_in(info)
    }
    fn cutline(&self) -> Option<CutlineInfo> {
        locate::find_cutline(&self.settings.cutline_path, &self.settings.cutline_repo)
    }
    fn doctor(&self, cutline: &CutlineInfo) -> Option<DoctorSummary> {
        locate::cutline_doctor(Utf8Path::new(&cutline.path)).ok()
    }
    fn free_space(&self, path: &Utf8Path) -> Option<u64> {
        crate::platform::free_space(path)
    }
    fn is_writable(&self, path: &Utf8Path) -> bool {
        crate::platform::is_writable_dir(path)
    }
    fn eleven_key_present(&self) -> bool {
        matches!(self.secrets.get(crate::platform::ELEVEN_KEY), Ok(Some(k)) if !k.is_empty())
    }
    fn eleven_remaining(&self) -> Option<u64> {
        let key = self.secrets.get(crate::platform::ELEVEN_KEY).ok().flatten()?;
        let client = crate::eleven::Client::new(&key, self.eleven_base_url.as_deref());
        client.subscription().ok().map(|s| s.remaining())
    }
    fn capture_permissions(&self) -> Option<crate::capture::Permissions> {
        Some(crate::capture::permissions(false))
    }
}

pub struct ValidateInput<'a> {
    pub project: &'a Project,
    pub project_path: Option<&'a Utf8Path>,
    /// The run config being validated; `None` checks the project alone.
    pub run_config: Option<&'a RunConfig>,
    pub kind: RunKind,
    pub settings: &'a Settings,
    pub platforms: &'a Platforms,
    pub models: &'a ModelList,
    pub library: &'a Utf8Path,
    /// Media resolution results for `project.media`.
    pub resolutions: &'a [Resolution],
    pub env: &'a dyn EnvProbe,
    /// Overrides the run config's output folder (`--output`).
    pub output_override: Option<&'a Utf8Path>,
}

struct Ctx {
    issues: Vec<Issue>,
}

impl Ctx {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        rule: &str,
        target: Option<&str>,
        severity: Severity,
        category: &str,
        title: String,
        detail: Option<String>,
        fix: Option<FixLink>,
    ) {
        let id = match target {
            Some(t) => format!("{rule}:{t}"),
            None => rule.to_string(),
        };
        self.issues.push(Issue { id, rule: rule.into(), severity, category: category.into(), title, detail, fix });
    }
}

/// Intents whose spec should state a call to action.
fn intent_needs_cta(kind: IntentKind) -> bool {
    matches!(
        kind,
        IntentKind::Marketing
            | IntentKind::ProductDemo
            | IntentKind::SocialClip
            | IntentKind::Announcement
            | IntentKind::Testimonial
            | IntentKind::AppStorePreview
    )
}

/// Whether the spec asks for generated audio: voice-over text with no recorded voice in the
/// media list, or a music brief that isn't a file.
pub fn needs_generated_audio(project: &Project) -> (bool, bool) {
    let has_vo = !spec::voice_overs(&project.spec).is_empty();
    let has_voice_file = project.media.iter().any(|m| m.role == Some(MediaRole::Voice));
    let lower = project.spec.to_lowercase();
    let music_generated = lower.contains("```music") && lower.contains("source: generated");
    (has_vo && !has_voice_file, music_generated)
}

/// Approximate Claude cost range in USD for a production run (spec §22.2): from past runs of this
/// project when there are any, else from the spec size, media count and output count.
pub fn estimate_claude_cost(
    project: &Project,
    outputs: usize,
    model: &str,
    models: &ModelList,
    kind: RunKind,
) -> (f64, f64) {
    let past: Vec<f64> = project
        .runs
        .iter()
        .filter(|r| {
            r.kind == kind
                && r.cost_usd.is_some()
                && r.model
                    .as_deref()
                    .map(|m| models.family_of(m).map(|f| &f.id) == models.family_of(model).map(|f| &f.id))
                    .unwrap_or(false)
        })
        .filter_map(|r| r.cost_usd)
        .collect();
    if !past.is_empty() {
        let avg = past.iter().sum::<f64>() / past.len() as f64;
        return (avg * 0.6, avg * 1.6);
    }
    let Some(price) = models.family_of(model).and_then(|f| f.price.clone()) else { return (0.0, 0.0) };
    let spec_tokens = project.spec.len() as f64 / 4.0;
    // Context per turn: the CLI's own prompt, cutline's guide, the manifest and the spec.
    let context = 30_000.0 + spec_tokens + project.media.len() as f64 * 120.0;
    let turns = match kind {
        RunKind::DryRun => 12.0,
        RunKind::Production => 25.0 + 10.0 * outputs as f64 + project.media.len() as f64 * 0.5,
    };
    let output_tokens = match kind {
        RunKind::DryRun => 6_000.0,
        RunKind::Production => 12_000.0 + 3_000.0 * outputs as f64 + spec_tokens * 2.0,
    };
    // Most input is read from the prompt cache (about a tenth of the input price).
    let input_cost = (context * 1.25 + context * turns * 0.1 + turns * 2_000.0) * price.input / 1e6;
    let output_cost = output_tokens * price.output / 1e6;
    let mid = input_cost + output_cost;
    (mid * 0.6, mid * 2.5)
}

fn check_environment(c: &mut Ctx, input: &ValidateInput, est: &mut Estimates, needs_audio: bool) {
    let env = input.env;
    match env.claude() {
        None => c.add(
            "claude_missing",
            None,
            Severity::Blocker,
            "environment",
            "Claude Code isn't installed".into(),
            Some("Install the Claude Code CLI, then choose Re-check in Settings › Claude.".into()),
            FixLink::target("settings", "claude"),
        ),
        Some(info) => {
            if !info.supported {
                c.add(
                    "claude_unsupported",
                    None,
                    Severity::Blocker,
                    "environment",
                    format!("Claude Code {} is too old", info.version),
                    Some(format!(
                        "This app needs Claude Code {} or later. Update the CLI and re-check.",
                        locate::min_version_text()
                    )),
                    FixLink::target("settings", "claude"),
                );
            } else if cfg!(windows) && info.git_bash.is_none() {
                c.add(
                    "git_bash_missing",
                    None,
                    Severity::Blocker,
                    "environment",
                    "Git for Windows isn't installed".into(),
                    Some("Claude Code runs commands through Git Bash on Windows. Install Git for Windows (git-scm.com), or set CLAUDE_CODE_GIT_BASH_PATH to its bash.exe, then re-check.".into()),
                    FixLink::target("settings", "claude"),
                );
            } else if env.claude_signed_in(&info) == Some(false) {
                c.add(
                    "claude_signed_out",
                    None,
                    Severity::Blocker,
                    "environment",
                    "Claude Code isn't signed in".into(),
                    Some(
                        "Open Terminal, run `claude` and sign in. The app never asks for Anthropic credentials.".into(),
                    ),
                    FixLink::target("settings", "claude"),
                );
            }
        }
    }
    match env.cutline() {
        None => c.add(
            "cutline_missing",
            None,
            Severity::Blocker,
            "environment",
            "cutline wasn't found".into(),
            Some("Set the cutline path in Settings › cutline, or put `cutline` on your PATH.".into()),
            FixLink::target("settings", "cutline"),
        ),
        Some(info) => match env.doctor(&info) {
            None => c.add(
                "cutline_doctor",
                None,
                Severity::Blocker,
                "environment",
                "`cutline doctor` didn't run".into(),
                Some(format!("Run `{} doctor` in Terminal to see why.", info.path)),
                FixLink::target("settings", "cutline"),
            ),
            Some(d) => {
                if !d.ok {
                    let what = if d.missing_required.is_empty() {
                        "see Settings › cutline for the report".to_string()
                    } else {
                        d.missing_required.join(", ")
                    };
                    c.add("cutline_doctor", None, Severity::Blocker, "environment", "cutline is missing required tools".into(), Some(format!("Missing: {what}. Use Install Dependencies in Settings › cutline (`cutline deps install --all`).")), FixLink::target("settings", "cutline"));
                } else if !d.missing_optional.is_empty() {
                    c.add("cutline_optional", None, Severity::Info, "environment", "Some optional cutline tools aren't installed".into(), Some(format!("Not installed: {}. They are only needed for HTML graphics, captions from speech or reframing.", d.missing_optional.join(", "))), FixLink::target("settings", "cutline"));
                }
            }
        },
    }
    if needs_audio {
        if !env.eleven_key_present() {
            c.add("eleven_key_missing", None, Severity::Blocker, "environment", "The spec needs generated audio, but there is no ElevenLabs API key".into(), Some("Add the key in Settings › ElevenLabs, or attach recorded voice-over and music files and mark their roles.".into()), FixLink::target("settings", "elevenlabs"));
        } else if let Some(remaining) = env.eleven_remaining() {
            est.eleven_remaining = Some(remaining);
            if remaining < est.eleven_credits {
                c.add(
                    "eleven_credits_low",
                    None,
                    Severity::Blocker,
                    "budget",
                    "Not enough ElevenLabs credits".into(),
                    Some(format!(
                        "About {} credits are needed and {} remain on the account.",
                        est.eleven_credits, remaining
                    )),
                    FixLink::target("settings", "elevenlabs"),
                );
            }
        }
    }
}

fn check_media(c: &mut Ctx, input: &ValidateInput, outputs: &[ResolvedOutput]) {
    let used: HashSet<String> = spec::media_refs(&input.project.spec).into_iter().map(|r| r.id).collect();
    let smallest_target = outputs.iter().map(|o| o.width.min(o.height)).min();
    for m in &input.project.media {
        let res = input.resolutions.iter().find(|r| r.id == m.id);
        let name = m.file_name().unwrap_or(&m.id).to_string();
        let fix = FixLink::target("media", &m.id);
        match res.map(|r| r.status).unwrap_or(MediaStatus::Missing) {
            MediaStatus::Missing => {
                let sev = if used.contains(&m.id) { Severity::Blocker } else { Severity::Warning };
                let stored = res.and_then(|r| r.stored_path.clone()).unwrap_or_default();
                c.add(
                    "media_missing",
                    Some(&m.id),
                    sev,
                    "media",
                    format!("{name} is missing"),
                    Some(format!("Last seen at {stored}. Relink it from the Media panel.")),
                    fix,
                );
                continue;
            }
            MediaStatus::Changed => c.add(
                "media_changed",
                Some(&m.id),
                Severity::Warning,
                "media",
                format!("{name} has changed since it was added"),
                Some("Accept the new content or relink the original file.".into()),
                fix.clone(),
            ),
            MediaStatus::Ok | MediaStatus::Moved => {}
        }
        let Some(path) = res.and_then(|r| r.path.as_deref()) else { continue };
        match std::fs::File::open(path) {
            Err(e) => {
                c.add(
                    "media_unreadable",
                    Some(&m.id),
                    Severity::Blocker,
                    "media",
                    format!("{name} can't be read"),
                    Some(e.to_string()),
                    fix,
                );
                continue;
            }
            Ok(f) => {
                if f.metadata().map(|md| md.len() == 0).unwrap_or(false) {
                    c.add(
                        "media_zero_length",
                        Some(&m.id),
                        Severity::Blocker,
                        "media",
                        format!("{name} is empty (0 bytes)"),
                        None,
                        fix,
                    );
                    continue;
                }
            }
        }
        let Some(p) = &m.probe else { continue };
        if let Some(err) = &p.error {
            if m.kind.is_probeable() {
                c.add(
                    "media_probe_failed",
                    Some(&m.id),
                    Severity::Blocker,
                    "media",
                    format!("{name} couldn't be read as {}", m.kind.key()),
                    Some(format!("ffprobe: {err}. The codec may be unsupported or the file damaged.")),
                    fix,
                );
            }
            continue;
        }
        if m.kind == MediaKind::Video && p.video_codec.is_none() {
            c.add(
                "media_probe_failed",
                Some(&m.id),
                Severity::Blocker,
                "media",
                format!("{name} has no video stream"),
                None,
                fix.clone(),
            );
        }
        if matches!(m.kind, MediaKind::Video | MediaKind::Audio) && p.duration_s.unwrap_or(0.0) <= 0.0 {
            c.add(
                "media_zero_length",
                Some(&m.id),
                Severity::Blocker,
                "media",
                format!("{name} has no duration"),
                None,
                fix.clone(),
            );
        }
        if p.vfr == Some(true) {
            c.add(
                "media_vfr",
                Some(&m.id),
                Severity::Info,
                "media",
                format!("{name} has a variable frame rate"),
                Some("cutline conforms it to a constant rate; cuts may land a frame off the original.".into()),
                fix.clone(),
            );
        }
        if let Some(r) = p.rotation {
            c.add(
                "media_rotation",
                Some(&m.id),
                Severity::Info,
                "media",
                format!("{name} is stored rotated by {r}°"),
                Some("It is shown upright; the stored frame is turned.".into()),
                fix.clone(),
            );
        }
        if let (MediaKind::Video, Some(w), Some(h), Some(target)) = (m.kind, p.width, p.height, smallest_target) {
            // Reference clips and logos aren't footage: only footage is expected to fill the frame.
            let is_footage = !matches!(m.role, Some(MediaRole::Reference) | Some(MediaRole::Logo));
            if is_footage && (w.min(h) as f64) < target as f64 * 0.6 {
                c.add("media_low_res", Some(&m.id), Severity::Warning, "media", format!("{name} is low resolution for the target ({w}×{h})"), Some(format!("The smallest selected output is {target} px on its short side; this clip will look soft when it fills the frame.")), fix);
            }
        }
    }
}

fn check_spec(c: &mut Ctx, input: &ValidateInput, est: &Estimates) {
    let p = input.project;
    if p.spec.trim().is_empty() {
        c.add(
            "spec_empty",
            None,
            Severity::Blocker,
            "spec",
            "The spec is empty".into(),
            Some("Describe the video, or start from a template.".into()),
            FixLink::line(1),
        );
    }
    if p.intent.kind == IntentKind::Unset {
        c.add(
            "intent_unset",
            None,
            Severity::Blocker,
            "spec",
            "The video's intent isn't set".into(),
            Some("Choose an intent in the Project inspector: it sets defaults and is given to Claude.".into()),
            FixLink::to("project"),
        );
    } else if p.intent.kind == IntentKind::Other && p.intent.other.as_deref().unwrap_or("").trim().is_empty() {
        c.add(
            "intent_unset",
            None,
            Severity::Blocker,
            "spec",
            "The intent is \"Other\" with no description".into(),
            None,
            FixLink::to("project"),
        );
    }
    let ids: HashSet<String> = p.media.iter().map(|m| m.id.clone()).collect();
    let lint = spec::lint(
        &p.spec,
        &spec::LintContext {
            media_ids: &ids,
            target_length_s: p.intent.target_length_s,
            words_per_minute: input.settings.words_per_minute,
        },
    );
    for (n, i) in lint.into_iter().enumerate() {
        let (rule, sev) = match i.rule.as_str() {
            "unresolved_media" => ("spec_unresolved_media", Severity::Blocker),
            "vo_too_long" => ("spec_vo_too_long", Severity::Warning),
            "placeholder" => ("spec_placeholder", Severity::Warning),
            "scene_empty" => ("spec_scene_empty", Severity::Warning),
            "scene_vo_too_long" => ("spec_scene_vo_too_long", Severity::Warning),
            "scene_vo_tight" => ("spec_scene_vo_tight", Severity::Info),
            _ => ("spec_scene_length", Severity::Info),
        };
        c.add(rule, Some(&format!("{}:{n}", i.line)), sev, "spec", i.message, None, FixLink::line(i.line));
    }
    if intent_needs_cta(p.intent.kind)
        && !spec::has_call_to_action(&p.spec)
        && p.intent.cta.as_deref().unwrap_or("").trim().is_empty()
    {
        c.add("spec_missing_section", Some("cta"), Severity::Warning, "spec", format!("A {} video usually has a call to action", p.intent.kind.label().to_lowercase()), Some("Add a \"Call to action\" section (type /cta), or set the primary call to action in the Project inspector.".into()), FixLink::line(1));
    }
    if est.duration_s.is_none() {
        c.add(
            "spec_no_length",
            None,
            Severity::Info,
            "spec",
            "No target length is set".into(),
            Some("Without a target length, platform duration limits and voice-over length can't be checked.".into()),
            FixLink::to("project"),
        );
    }
}

/// What a run can only find out the expensive way: no voice chosen, and app recording that the
/// computer or the project doesn't allow.
fn check_run_needs(c: &mut Ctx, input: &ValidateInput, outputs: &[ResolvedOutput]) {
    let p = input.project;
    let (needs_voice, _) = needs_generated_audio(p);
    let brand_voice = p.brand.as_ref().map(|b| b.snapshot.voice.voice_id.trim()).unwrap_or("");
    if needs_voice && brand_voice.is_empty() && input.settings.eleven_default_voice.trim().is_empty() {
        // A voice added to the preset in the library doesn't reach a project that already holds a
        // copy of the preset: say so, or the warning looks wrong.
        let waiting_in_library = p.brand.as_ref().is_some_and(|b| {
            brand::get(input.library, &b.preset_id).is_some_and(|lib| !lib.voice.voice_id.trim().is_empty())
        });
        let (title, detail) = if waiting_in_library {
            (
                "This project's copy of the brand preset has no voice",
                "The preset in the library has a voice, but the project keeps the copy it was given earlier. Choose Update to Latest to take it.",
            )
        } else {
            (
                "No voice is chosen for the voice-over",
                "Claude would have to pick one itself. Choose a voice in the brand preset, or set a default voice in Settings › ElevenLabs.",
            )
        };
        c.add(
            "voice_unset",
            None,
            Severity::Warning,
            "brand",
            title.into(),
            Some(detail.into()),
            if waiting_in_library {
                // the app turns this link into a one-click Update to Latest button
                FixLink::target("brand", "update")
            } else if p.brand.is_some() {
                FixLink::to("brand")
            } else {
                FixLink::target("settings", "elevenlabs")
            },
        );
    }

    let recorded: Vec<spec::Scene> = spec::scenes(&p.spec).into_iter().filter(|s| s.records_app).collect();
    if let Some(first) = recorded.first() {
        let n = recorded.len();
        let scenes = if n == 1 { "1 scene asks".to_string() } else { format!("{n} scenes ask") };
        if !p.app_capture.enabled {
            c.add(
                "capture_off",
                None,
                Severity::Warning,
                "spec",
                "The spec asks to record an app, but App recording is off".into(),
                Some(format!("{scenes} for a recording. Turn on App recording in the Project inspector, or attach the recordings as media.")),
                FixLink::line(first.line),
            );
        } else if let Some(perm) = input.env.capture_permissions().filter(|perm| perm.supported) {
            // One warning per permission, so each can be fixed with its own button.
            for (ok, key, name, what) in [
                (perm.screen_recording, "screen_recording", "Screen & System Audio Recording", "record the screen"),
                (perm.accessibility, "accessibility", "Accessibility", "click, type and use menus in other apps"),
            ] {
                if ok {
                    continue;
                }
                c.add(
                    "capture_permission",
                    Some(key),
                    Severity::Warning,
                    "environment",
                    format!("{name} isn't allowed for Syncfusion Video Studio yet"),
                    Some(format!(
                        "{scenes} for a recording, and the run needs this to {what}. Allow opens System Settings with Syncfusion Video Studio in the list, switched off: turn its switch on. If the switch was already on, it was for an earlier build of the app, and Allow replaces it.{}",
                        if key == "screen_recording" { " macOS then offers to quit and reopen the app." } else { "" }
                    )),
                    FixLink::target("permission", key),
                );
            }
        }
    }

    // Burned-in captions asked for in words, on an output that delivers them another way. A line
    // that names the outputs it means is taken at its word: it only counts against an output it
    // names, and not when it also says that output gets a caption file.
    const BURNED: [&str; 5] =
        ["burned-in caption", "burned in caption", "burnt-in caption", "burn in caption", "burn-in caption"];
    const SIDECAR: [&str; 8] =
        ["separate file", "caption file", "captions file", "subtitle file", "sidecar", "srt", "vtt", "no captions"];
    let names: Vec<Vec<String>> = outputs.iter().map(output_names).collect();
    let named = |line: &str, i: usize| names[i].iter().any(|n| contains_word(line, n));
    let mut missed: Vec<usize> = Vec::new();
    let mut at_line = None;
    for (n, raw) in p.spec.lines().enumerate() {
        let line = raw.to_lowercase();
        if !BURNED.iter().any(|k| line.contains(k)) {
            continue;
        }
        let names_any = (0..outputs.len()).any(|i| named(&line, i));
        let explains = SIDECAR.iter().any(|k| contains_word(&line, k));
        for (i, o) in outputs.iter().enumerate() {
            let counts = if names_any { named(&line, i) && !explains } else { true };
            if o.captions != "burn" && counts {
                if !missed.contains(&i) {
                    missed.push(i);
                }
                at_line.get_or_insert(n + 1);
            }
        }
    }
    if let Some(line) = at_line {
        missed.sort_unstable();
        let others: Vec<String> = missed
            .iter()
            .map(|&i| &outputs[i])
            .map(|o| match o.captions.as_str() {
                "sidecar" => format!("{} delivers captions as a separate file", o.name),
                "none" => format!("{} has no captions", o.name),
                other => format!("{} has captions set to \"{other}\"", o.name),
            })
            .collect();
        c.add(
            "captions_not_burned",
            None,
            Severity::Info,
            "outputs",
            "The spec mentions burned-in captions, but not every output burns them in".into(),
            Some(format!(
                "{}. Change the output's captions setting under Outputs, or say in the spec which outputs get burned-in captions.",
                others.join("; ")
            )),
            FixLink::line(line),
        );
    }

    check_outputs_asked_for(c, input, outputs);

    // A watermark asked for in words, with a brand that has none to draw.
    let has_watermark = p.brand.as_ref().is_some_and(|b| b.snapshot.for_cutline().watermark);
    if !has_watermark {
        let asked = p.spec.lines().enumerate().find(|(_, l)| {
            let l = l.to_lowercase();
            contains_word(&l, "watermark")
                && !["no watermark", "without a watermark", "without watermark"].iter().any(|k| l.contains(k))
        });
        if let Some((n, _)) = asked {
            c.add(
                "watermark_missing",
                None,
                Severity::Info,
                "brand",
                "The spec mentions a watermark, but the brand preset has none".into(),
                Some("The video will have no watermark. Add a logo and a watermark image to the brand preset, or take the watermark out of the spec.".into()),
                FixLink::line(n + 1),
            );
        }
    }
}

fn check_brand(c: &mut Ctx, input: &ValidateInput) {
    let Some(b) = &input.project.brand else {
        c.add(
            "brand_none",
            None,
            Severity::Info,
            "brand",
            "No brand preset is selected".into(),
            Some("Claude will choose neutral colors and fonts.".into()),
            FixLink::to("brand"),
        );
        return;
    };
    let s = &b.snapshot;
    for (label, asset) in s.assets() {
        if !asset.exists() {
            c.add(
                "brand_asset_missing",
                Some(label),
                Severity::Blocker,
                "brand",
                format!("Brand asset missing: {label}"),
                Some(format!("{} wasn't found. Relink it in the brand preset.", asset.path)),
                FixLink::to("brand"),
            );
        } else if label.ends_with("font") {
            let ok = asset
                .resolved()
                .extension()
                .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
                .unwrap_or(false);
            if !ok {
                c.add(
                    "brand_font_unloadable",
                    Some(label),
                    Severity::Warning,
                    "brand",
                    format!(
                        "{label}: {} isn't a TrueType or OpenType font",
                        asset.resolved().file_name().unwrap_or("")
                    ),
                    Some("cutline loads .ttf, .otf and .ttc files.".into()),
                    FixLink::to("brand"),
                );
            }
        }
    }
    let mut parsed = std::collections::HashMap::new();
    for (role, value) in s.colors.roles() {
        match brand::parse_hex(value) {
            Some(rgb) => {
                parsed.insert(role, rgb);
            }
            None => c.add(
                "brand_color_invalid",
                Some(role),
                Severity::Blocker,
                "brand",
                format!("The {role} color \"{value}\" isn't a hex color"),
                Some("Use #RRGGBB.".into()),
                FixLink::to("brand"),
            ),
        }
    }
    if let (Some(text), Some(bg)) = (parsed.get("text"), parsed.get("background")) {
        let ratio = brand::contrast(*text, *bg);
        if ratio < 4.5 {
            c.add(
                "brand_text_contrast",
                None,
                Severity::Warning,
                "brand",
                format!("Text color has low contrast on the background ({ratio:.1}:1)"),
                Some("Captions and titles need at least 4.5:1 to stay readable.".into()),
                FixLink::to("brand"),
            );
        }
    }
    if let Some(bg) = parsed.get("background") {
        // The logo meant for this background: the light/dark variant when given, else the primary.
        let dark_bg = brand::luminance(*bg) < 0.4;
        let logo = if dark_bg { s.logos.dark.as_ref() } else { s.logos.light.as_ref() }.or(s.logos.primary.as_ref());
        if let Some(avg) = logo.filter(|l| l.exists()).and_then(|l| brand::logo_average_color(&l.resolved())) {
            let ratio = brand::contrast(avg, *bg);
            if ratio < 1.6 {
                c.add(
                    "brand_logo_contrast",
                    None,
                    Severity::Warning,
                    "brand",
                    format!("The logo barely stands out from the background color ({ratio:.1}:1)"),
                    Some("Add a logo variant for this background, or change the background color.".into()),
                    FixLink::to("brand"),
                );
            }
        }
    }
    if brand::library_is_newer(input.library, b) {
        c.add(
            "brand_outdated",
            None,
            Severity::Info,
            "brand",
            format!("The brand preset \"{}\" has been updated in the library", s.name),
            Some("The project uses the copy it was given earlier. Choose Update to Latest to take the changes.".into()),
            FixLink::target("brand", "update"),
        );
    }
}

/// An output the spec asks for by name or shape that this run doesn't make. Only lines that are
/// about delivery count (before the first scene, and saying or sitting under "outputs", "export",
/// "deliver" and the like): a scene may show or talk about a platform without asking for it.
fn check_outputs_asked_for(c: &mut Ctx, input: &ValidateInput, outputs: &[ResolvedOutput]) {
    const DELIVERY: [&str; 9] =
        ["output", "outputs", "export", "exports", "deliver", "delivery", "deliverables", "aspect", "formats"];
    const SHAPES: [(&str, f64); 4] = [("9:16", 9.0 / 16.0), ("4:5", 0.8), ("1:1", 1.0), ("16:9", 16.0 / 9.0)];
    if outputs.is_empty() {
        return;
    }
    let spec = &input.project.spec;
    let first_scene = spec::scenes(spec).first().map(|s| s.line).unwrap_or(usize::MAX);
    let about_delivery = |l: &str| DELIVERY.iter().any(|k| contains_word(l, k));
    let has_shape =
        |ratio: f64| outputs.iter().any(|o| o.height > 0 && (o.width as f64 / o.height as f64 - ratio).abs() < 0.02);
    let mut missing: Vec<String> = Vec::new();
    let mut at_line = None;
    let mut under_delivery = false;
    for (n, raw) in spec.lines().enumerate() {
        if n + 1 >= first_scene {
            break;
        }
        let line = raw.to_lowercase();
        if line.trim_start().starts_with('#') {
            under_delivery = about_delivery(&line);
            continue;
        }
        if !(under_delivery || about_delivery(&line)) {
            continue;
        }
        let mut found: Vec<String> = Vec::new();
        for preset in input.platforms.presets.iter().filter(|p| p.id != "custom") {
            let name = preset.name.to_lowercase();
            // "Shorts" and "Reels" are said on their own
            let short = name.split_whitespace().nth(1).filter(|w| ["shorts", "reels"].contains(w));
            let asked = contains_word(&line, &name) || short.is_some_and(|w| contains_word(&line, w));
            if asked && !outputs.iter().any(|o| o.preset == preset.id) {
                found.push(preset.name.clone());
            }
        }
        if found.is_empty() {
            // a shape with no platform named on the line
            let names_output =
                (0..outputs.len()).any(|i| output_names(&outputs[i]).iter().any(|x| contains_word(&line, x)));
            if !names_output {
                found.extend(
                    SHAPES
                        .iter()
                        .filter(|(t, r)| contains_word(&line, t) && !has_shape(*r))
                        .map(|(t, _)| format!("{t} output")),
                );
            }
        }
        for f in found {
            if !missing.contains(&f) {
                missing.push(f);
                at_line.get_or_insert(n + 1);
            }
        }
    }
    let Some(line) = at_line else { return };
    let has: Vec<&str> = outputs.iter().map(|o| o.name.as_str()).collect();
    let title = if missing.len() == 1 {
        let m = &missing[0];
        let article = if m.starts_with(|ch: char| ch.is_ascii_digit()) { "a " } else { "" };
        format!("The spec asks for {article}{m}, but this run doesn't make one")
    } else {
        format!("The spec asks for outputs this run doesn't make: {}", missing.join(", "))
    };
    c.add(
        "output_not_in_run",
        None,
        Severity::Warning,
        "outputs",
        title,
        Some(format!(
            "This run makes {}. Add the output under Outputs and to this run configuration, or take it out of the spec.",
            has.join(", ")
        )),
        FixLink::line(line),
    );
}

/// What a spec might call an output: its name, its platform, and the first word of its name
/// ("YouTube" for "YouTube 1080p"), lowercased.
fn output_names(o: &ResolvedOutput) -> Vec<String> {
    let mut names = vec![o.name.to_lowercase(), o.platform.replace(['_', '-'], " ").to_lowercase()];
    if let Some(first) = o.name.split_whitespace().next() {
        if first.chars().count() >= 4 && first.chars().all(char::is_alphabetic) {
            names.push(first.to_lowercase());
        }
    }
    names.retain(|n| !n.trim().is_empty());
    names.dedup();
    names
}

/// True when `needle` is in `text` and not inside a longer word.
fn contains_word(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(i, m)| {
        let before = text[..i].chars().next_back().is_some_and(char::is_alphanumeric);
        let after = text[i + m.len()..].chars().next().is_some_and(char::is_alphanumeric);
        !before && !after
    })
}

fn check_outputs(
    c: &mut Ctx,
    input: &ValidateInput,
    outputs: &[ResolvedOutput],
    out_root: Option<&Utf8PathBuf>,
    est: &mut Estimates,
) {
    let Some(rc) = input.run_config else { return };
    if rc.outputs.is_empty() {
        c.add(
            "outputs_none",
            None,
            Severity::Blocker,
            "outputs",
            format!("The run config \"{}\" has no outputs selected", rc.name),
            Some("Select at least one output config.".into()),
            FixLink::target("run_config", &rc.id),
        );
    }
    let duration = est.duration_s;
    // The main footage's aspect: the largest video by pixel count.
    let source_aspect = input
        .project
        .media
        .iter()
        .filter(|m| m.kind == MediaKind::Video && !matches!(m.role, Some(MediaRole::Reference)))
        .filter_map(|m| m.probe.as_ref().and_then(|p| Some((p.width? as f64, p.height? as f64))))
        .max_by(|a, b| (a.0 * a.1).total_cmp(&(b.0 * b.1)))
        .map(|(w, h)| w / h.max(1.0));
    for o in outputs {
        let fix = FixLink::target("outputs", &o.id);
        if let Some(d) = duration {
            if let Some(max) = o.limits.max_duration_s {
                if d > max + 0.01 {
                    c.add(
                        "output_duration_limit",
                        Some(&o.id),
                        Severity::Blocker,
                        "outputs",
                        format!("{}: the target length is over the {} limit", o.name, o.platform),
                        Some(format!(
                            "The video is planned at {} and {} allows at most {}.",
                            format_duration(d),
                            o.name,
                            format_duration(max)
                        )),
                        fix.clone(),
                    );
                }
            }
            if let Some(min) = o.limits.min_duration_s {
                if d + 0.01 < min {
                    c.add(
                        "output_duration_limit",
                        Some(&o.id),
                        Severity::Blocker,
                        "outputs",
                        format!("{}: the target length is under the {} minimum", o.name, o.platform),
                        Some(format!(
                            "The video is planned at {} and {} needs at least {}.",
                            format_duration(d),
                            o.name,
                            format_duration(min)
                        )),
                        fix.clone(),
                    );
                }
            }
            if let Some(max_bytes) = o.limits.max_bytes {
                let bytes = estimate_output_bytes(o, d) / o.exports.len().max(1) as u64;
                if bytes > max_bytes {
                    c.add(
                        "output_size_limit",
                        Some(&o.id),
                        Severity::Warning,
                        "outputs",
                        format!("{}: the file may exceed the {} size limit", o.name, o.platform),
                        Some(format!(
                            "About {} is expected and the limit is {}. Set a size cap on this output.",
                            paths::format_bytes(bytes),
                            paths::format_bytes(max_bytes)
                        )),
                        fix.clone(),
                    );
                }
            }
        }
        if let (Some(fps), Some(max)) = (o.fps, o.limits.max_fps) {
            if fps > max + 0.01 {
                c.add(
                    "output_fps_limit",
                    Some(&o.id),
                    Severity::Blocker,
                    "outputs",
                    format!("{}: {fps} fps is over the {} limit of {max} fps", o.name, o.platform),
                    None,
                    fix.clone(),
                );
            }
        }
        if let Some(src) = source_aspect {
            let ratio = (src / o.aspect()).max(o.aspect() / src);
            if ratio > 1.7 {
                c.add("output_aspect", Some(&o.id), Severity::Warning, "outputs", format!("{}: heavy reframing ahead", o.name), Some(format!("The main footage is {} and this output is {}. More than {}% of each frame is cropped away; check the review pack's reframe sheet.", aspect_text(src), o.aspect_label(), ((1.0 - 1.0 / ratio) * 100.0).round())), fix.clone());
            }
        }
    }
    let d = duration.unwrap_or(60.0);
    // Finals plus drafts, proxies and the segment cache: about three times the deliverables.
    est.disk_bytes = outputs.iter().map(|o| estimate_output_bytes(o, d)).sum::<u64>() * 3 + 200_000_000;
    if let Some(root) = out_root {
        if !input.env.is_writable(root) {
            c.add("output_folder_unwritable", None, Severity::Blocker, "outputs", "The output folder can't be written".into(), Some(format!("{root} doesn't exist and can't be created, or it is read-only. Choose another folder in the run config.")), FixLink::target("run_config", &rc.id));
        } else if let Some(free) = input.env.free_space(root) {
            est.free_bytes = Some(free);
            if free < est.disk_bytes {
                c.add(
                    "disk_space",
                    None,
                    Severity::Blocker,
                    "environment",
                    "Not enough free disk space for this run".into(),
                    Some(format!(
                        "About {} is needed in {root} and {} is free.",
                        paths::format_bytes(est.disk_bytes),
                        paths::format_bytes(free)
                    )),
                    FixLink::target("run_config", &rc.id),
                );
            } else if free < est.disk_bytes * 2 {
                c.add(
                    "disk_space",
                    None,
                    Severity::Warning,
                    "environment",
                    "Free disk space is tight".into(),
                    Some(format!(
                        "About {} is needed and {} is free.",
                        paths::format_bytes(est.disk_bytes),
                        paths::format_bytes(free)
                    )),
                    FixLink::target("run_config", &rc.id),
                );
            }
        }
    }
}

fn aspect_text(a: f64) -> String {
    for (label, v) in [
        ("16:9", 16.0 / 9.0),
        ("9:16", 9.0 / 16.0),
        ("1:1", 1.0),
        ("4:5", 0.8),
        ("4:3", 4.0 / 3.0),
        ("3:4", 0.75),
        ("21:9", 21.0 / 9.0),
    ] {
        if (a - v).abs() < 0.03 {
            return label.to_string();
        }
    }
    format!("{a:.2}:1")
}

fn check_budget(c: &mut Ctx, input: &ValidateInput, est: &Estimates, outputs: usize) {
    let Some(rc) = input.run_config else { return };
    if est.eleven_credits > est.eleven_cap {
        c.add("budget_eleven_cap", None, Severity::Blocker, "budget", "Generated audio is over this run's ElevenLabs credit cap".into(), Some(format!("About {} credits are needed ({} characters of voice-over) and the cap is {}. Raise the cap in the run config or shorten the script.", est.eleven_credits, est.eleven_characters, est.eleven_cap)), FixLink::target("run_config", &rc.id));
    }
    if let Some(cap) = rc.max_spend_usd.filter(|c| *c > 0.0) {
        if est.claude_usd_low > cap {
            c.add("budget_claude_cap", None, Severity::Blocker, "budget", "The Claude spend limit is below the estimated cost".into(), Some(format!("The estimate is about ${:.2}–${:.2} (approximate) and the limit is ${cap:.2}. The run would stop before finishing.", est.claude_usd_low, est.claude_usd_high)), FixLink::target("run_config", &rc.id));
        } else if est.claude_usd_high > cap {
            c.add(
                "budget_claude_cap",
                None,
                Severity::Warning,
                "budget",
                "The Claude spend limit may be reached".into(),
                Some(format!(
                    "The estimate is about ${:.2}–${:.2} (approximate) and the limit is ${cap:.2}.",
                    est.claude_usd_low, est.claude_usd_high
                )),
                FixLink::target("run_config", &rc.id),
            );
        }
    }
    let dry = &input.project.models.dry_run_or(input.settings);
    if rc.dry_run_first && input.models.is_cheaper(dry, &rc.production_model) == Some(false) && outputs > 0 {
        c.add(
            "dry_run_model",
            None,
            Severity::Info,
            "budget",
            "The dry run model isn't cheaper than the production model".into(),
            Some(format!(
                "Dry runs use {} and production uses {}.",
                input.models.display_name(dry),
                input.models.display_name(&rc.production_model)
            )),
            FixLink::to("project"),
        );
    }
}

/// Runs every static check and returns the report.
pub fn validate(input: &ValidateInput) -> Report {
    let p = input.project;
    let mut c = Ctx { issues: Vec::new() };
    let mut est = Estimates::default();

    // Outputs first: several checks need them.
    let mut outputs: Vec<ResolvedOutput> = Vec::new();
    let mut out_root: Option<Utf8PathBuf> = None;
    if let Some(rc) = input.run_config {
        let mut taken = HashSet::new();
        for id in &rc.outputs {
            match p.output_config(id) {
                None => c.add(
                    "output_preset_missing",
                    Some(id),
                    Severity::Blocker,
                    "outputs",
                    format!("The output \"{id}\" no longer exists"),
                    None,
                    FixLink::target("run_config", &rc.id),
                ),
                Some(cfg) => match resolve_output(cfg, input.platforms, &mut taken) {
                    Ok(o) => outputs.push(o),
                    Err(e) => c.add(
                        "output_preset_missing",
                        Some(id),
                        Severity::Blocker,
                        "outputs",
                        e.to_string(),
                        None,
                        FixLink::target("outputs", id),
                    ),
                },
            }
        }
        out_root = Some(match input.output_override {
            Some(o) => o.to_owned(),
            None => rc.output_root(&p.name, Some(&input.settings.default_output_folder)),
        });
        est.eleven_cap = rc.eleven_credit_cap;
    } else {
        est.eleven_cap = input.settings.eleven_default_credit_cap;
    }

    // Estimates
    let scene_total: f64 = spec::scenes(&p.spec).iter().filter_map(|s| s.duration_s).sum();
    est.duration_s = p.intent.target_length_s.filter(|d| *d > 0.0).or((scene_total > 0.0).then_some(scene_total));
    let vos = spec::voice_overs(&p.spec);
    est.voice_over_words = vos.iter().map(|v| spec::word_count(&v.text)).sum();
    let (needs_tts, needs_music) = needs_generated_audio(p);
    let locales = input.run_config.map(|r| r.locales.len()).unwrap_or(0).max(p.intent.locales.len()).max(1) as u64;
    let pricing = crate::eleven::Pricing::bundled();
    if needs_tts {
        est.eleven_characters = vos.iter().map(|v| v.text.chars().count() as u64).sum::<u64>() * locales;
        est.eleven_credits += (est.eleven_characters as f64 * pricing.tts_rate(None)).ceil() as u64;
    }
    if needs_music {
        est.eleven_credits += (est.duration_s.unwrap_or(60.0) * pricing.music_credits_per_second).ceil() as u64;
    }
    let model = match input.kind {
        RunKind::DryRun => p.models.dry_run_or(input.settings),
        RunKind::Production => input
            .run_config
            .map(|r| r.production_model.clone())
            .unwrap_or_else(|| input.settings.default_production_model.clone()),
    };
    let (lo, hi) = estimate_claude_cost(p, outputs.len(), &model, input.models, input.kind);
    est.claude_usd_low = lo;
    est.claude_usd_high = hi;

    // A dry run generates no audio, so the key and credits aren't needed for it.
    let needs_audio = (needs_tts || needs_music) && input.kind == RunKind::Production;
    check_environment(&mut c, input, &mut est, needs_audio);
    check_media(&mut c, input, &outputs);
    check_spec(&mut c, input, &est);
    check_brand(&mut c, input);
    check_run_needs(&mut c, input, &outputs);
    check_outputs(&mut c, input, &outputs, out_root.as_ref(), &mut est);
    if input.kind == RunKind::Production {
        check_budget(&mut c, input, &est, outputs.len());
    }

    let mut report = Report {
        generated: crate::timeutil::now_rfc3339(),
        project_id: p.id.clone(),
        run_config: input.run_config.map(|r| r.id.clone()),
        issues: c.issues,
        estimates: est,
        ..Report::default()
    };
    report.recount();
    report
}

/// A fake machine for tests (also used by the CLI's own tests).
#[derive(Debug, Clone)]
pub struct FakeEnv {
    pub claude: Option<ClaudeInfo>,
    pub signed_in: Option<bool>,
    pub cutline: Option<CutlineInfo>,
    pub doctor: Option<DoctorSummary>,
    pub free: Option<u64>,
    pub writable: bool,
    pub eleven_key: bool,
    pub eleven_remaining: Option<u64>,
    pub capture: Option<crate::capture::Permissions>,
}

impl Default for FakeEnv {
    fn default() -> Self {
        FakeEnv {
            claude: Some(ClaudeInfo {
                path: "/bin/claude".into(),
                version: "2.1.311".into(),
                version_text: "2.1.311 (Claude Code)".into(),
                effort_levels: vec![],
                login_path: None,
                supported: true,
                git_bash: Some("bash".into()),
            }),
            signed_in: Some(true),
            cutline: Some(CutlineInfo { path: "/bin/cutline".into(), version: "0.1.0".into() }),
            doctor: Some(DoctorSummary { ok: true, ..Default::default() }),
            free: Some(500_000_000_000),
            writable: true,
            eleven_key: true,
            eleven_remaining: Some(1_000_000),
            capture: Some(crate::capture::Permissions { screen_recording: true, accessibility: true, supported: true }),
        }
    }
}

impl EnvProbe for FakeEnv {
    fn claude(&self) -> Option<ClaudeInfo> {
        self.claude.clone()
    }
    fn claude_signed_in(&self, _: &ClaudeInfo) -> Option<bool> {
        self.signed_in
    }
    fn cutline(&self) -> Option<CutlineInfo> {
        self.cutline.clone()
    }
    fn doctor(&self, _: &CutlineInfo) -> Option<DoctorSummary> {
        self.doctor.clone()
    }
    fn free_space(&self, _: &Utf8Path) -> Option<u64> {
        self.free
    }
    fn is_writable(&self, _: &Utf8Path) -> bool {
        self.writable
    }
    fn eleven_key_present(&self) -> bool {
        self.eleven_key
    }
    fn eleven_remaining(&self) -> Option<u64> {
        self.eleven_remaining
    }
    fn capture_permissions(&self) -> Option<crate::capture::Permissions> {
        self.capture
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::{AssetRef, BrandPreset, BrandRef};
    use crate::media::{MediaItem, Probe};
    use crate::presets::{OutputConfig, OutputOverrides};

    struct Fx {
        dir: tempfile::TempDir,
        project: Project,
        settings: Settings,
        platforms: Platforms,
        models: ModelList,
        env: FakeEnv,
    }

    const GOOD_SPEC: &str = "# Launch\n\n## Scene 1: Hook\n- **Duration:** 20 s\n- **Media:** ![hero](media:m-hero)\n\n```vo\nMeet the fastest way to ship your product today.\n```\n\n## Scene 2: Close\n- **Duration:** 25 s\n- **On-screen text:** Try it\n\n## Call to action\nStart a free trial.\n";

    impl Fx {
        fn new() -> Fx {
            let dir = tempfile::tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap().to_owned();
            let hero = root.join("hero.mov");
            std::fs::write(&hero, b"video-bytes").unwrap();
            let mut project = Project::new("Launch");
            project.spec = GOOD_SPEC.into();
            project.intent.kind = IntentKind::Marketing;
            project.intent.target_length_s = Some(45.0);
            let mut m = MediaItem::with_path("m-hero", MediaKind::Video, hero.as_str());
            m.role = Some(MediaRole::Footage);
            m.probe = Some(Probe {
                duration_s: Some(32.0),
                width: Some(1920),
                height: Some(1080),
                fps: Some("30/1".into()),
                video_codec: Some("h264".into()),
                ..Probe::default()
            });
            project.media.push(m);
            project.output_configs.push(OutputConfig {
                id: "out_yt".into(),
                preset: "youtube_1080p".into(),
                name: None,
                overrides: OutputOverrides::default(),
            });
            let mut rc = RunConfig::new("YouTube final");
            rc.id = "rc_yt".into();
            rc.outputs = vec!["out_yt".into()];
            rc.output_folder = root.join("out").to_string();
            project.run_configs.push(rc);
            Fx {
                dir,
                project,
                settings: Settings { eleven_default_voice: "voice_default".into(), ..Settings::default() },
                platforms: Platforms::bundled(),
                models: ModelList::bundled(),
                env: FakeEnv::default(),
            }
        }
        fn root(&self) -> Utf8PathBuf {
            Utf8Path::from_path(self.dir.path()).unwrap().to_owned()
        }
        fn run_kind(&self, kind: RunKind) -> Report {
            let res = crate::media::resolve_all(&self.project, None, &[], &crate::platform::NoFileRefs);
            let lib = self.root().join("lib");
            validate(&ValidateInput {
                project: &self.project,
                project_path: None,
                run_config: self.project.run_configs.first(),
                kind,
                settings: &self.settings,
                platforms: &self.platforms,
                models: &self.models,
                library: &lib,
                resolutions: &res,
                env: &self.env,
                output_override: None,
            })
        }
        fn run(&self) -> Report {
            self.run_kind(RunKind::Production)
        }
        fn rc(&mut self) -> &mut RunConfig {
            &mut self.project.run_configs[0]
        }
    }

    fn has(r: &Report, rule: &str, sev: Severity) -> bool {
        r.issues.iter().any(|i| i.rule == rule && i.severity == sev)
    }
    fn rules(r: &Report) -> Vec<String> {
        r.issues.iter().map(|i| format!("{:?}:{}", i.severity, i.rule)).collect()
    }

    #[test]
    fn finds_what_a_run_would_only_learn_the_expensive_way() {
        let mut f = Fx::new();
        // no voice anywhere
        f.settings.eleven_default_voice = String::new();
        let r = f.run();
        assert!(has(&r, "voice_unset", Severity::Warning), "{:?}", rules(&r));
        assert_eq!(
            r.issues.iter().find(|i| i.rule == "voice_unset").unwrap().title,
            "No voice is chosen for the voice-over"
        );
        // the voice was added to the preset in the library after the project took its copy
        let lib = f.root().join("lib");
        let mut preset = brand::BrandPreset::new("Acme");
        brand::save(&lib, &mut preset).unwrap();
        f.project.brand = Some(brand::BrandRef::of(&preset));
        preset.voice.voice_id = "voice_lib".into();
        brand::save(&lib, &mut preset).unwrap();
        let r = f.run();
        let i = r.issues.iter().find(|i| i.rule == "voice_unset").expect("still a warning: the run uses the copy");
        assert_eq!(i.title, "This project's copy of the brand preset has no voice");
        assert!(i.detail.as_deref().unwrap().contains("Update to Latest"));
        assert_eq!(i.fix.as_ref().unwrap().target.as_deref(), Some("update"));
        f.project.brand = Some(brand::BrandRef::of(&preset));
        assert!(!has(&f.run(), "voice_unset", Severity::Warning));
        f.project.brand = None;
        f.settings.eleven_default_voice = "voice_1".into();
        assert!(!has(&f.run(), "voice_unset", Severity::Warning));

        // a recorded scene needs the computer's permissions and the project's switch
        f.project.spec = format!("{GOOD_SPEC}\n## Scene 3: Demo\n- **Duration:** 30 s\n- **Record:** Safari, open the pricing page\n\nAll outputs get burned-in captions.\n");
        assert!(!f.run().issues.iter().any(|i| i.rule.starts_with("capture_")));
        f.env.capture =
            Some(crate::capture::Permissions { screen_recording: true, accessibility: false, supported: true });
        let r = f.run();
        let found: Vec<&Issue> = r.issues.iter().filter(|i| i.rule == "capture_permission").collect();
        assert_eq!(found.len(), 1, "only the missing permission is reported");
        let i = found[0];
        assert_eq!((i.severity, i.id.as_str()), (Severity::Warning, "capture_permission:accessibility"));
        assert!(i.title.starts_with("Accessibility isn't allowed"));
        assert!(i.detail.as_deref().unwrap().contains("1 scene asks"));
        // the app turns this link into a one-click Allow button
        assert_eq!(
            i.fix,
            Some(FixLink { kind: "permission".into(), target: Some("accessibility".into()), line: None })
        );
        f.env.capture =
            Some(crate::capture::Permissions { screen_recording: false, accessibility: false, supported: true });
        assert_eq!(f.run().issues.iter().filter(|i| i.rule == "capture_permission").count(), 2);
        f.env.capture =
            Some(crate::capture::Permissions { screen_recording: false, accessibility: false, supported: false });
        assert!(
            !f.run().issues.iter().any(|i| i.rule == "capture_permission"),
            "not on platforms without app recording"
        );
        f.project.app_capture.enabled = false;
        assert!(has(&f.run(), "capture_off", Severity::Warning));

        // burned-in captions in words against an output that delivers a sidecar
        let r = f.run();
        let i = r.issues.iter().find(|i| i.rule == "captions_not_burned").expect("caption note");
        assert_eq!(i.severity, Severity::Info);
        assert!(i.detail.as_deref().unwrap().contains("delivers captions as a separate file"), "{:?}", i.detail);
        assert_eq!(i.fix.as_ref().and_then(|f| f.line), Some(22));

        // a spec that names the outputs it means is taken at its word
        let with = |last: &str| format!("{GOOD_SPEC}\n{last}\n");
        f.project.output_configs.push(OutputConfig {
            id: "out_li".into(),
            preset: "linkedin".into(),
            name: None,
            overrides: OutputOverrides::default(),
        });
        f.project.run_configs[0].outputs.push("out_li".into());
        let noted = |f: &mut Fx, last: &str| {
            f.project.spec = with(last);
            f.run().issues.iter().any(|i| i.rule == "captions_not_burned")
        };
        assert!(noted(&mut f, "Every output gets burned-in captions."));
        assert!(noted(&mut f, "YouTube gets burned-in captions."), "names the output that doesn't burn them in");
        assert!(!noted(&mut f, "LinkedIn gets burned-in captions."), "about another output");
        assert!(!noted(&mut f, "LinkedIn gets burned-in captions, YouTube gets a caption file."));
        assert!(!noted(&mut f, "- Captions: burned-in captions for LinkedIn\n- YouTube 1080p: SRT"));

        // an output the spec asks for that the run doesn't make (the run has YouTube 1080p and LinkedIn)
        let asked = |f: &mut Fx, head: &str| {
            f.project.spec = format!("# Launch\n\n{head}\n\n{}", GOOD_SPEC.trim_start_matches("# Launch\n\n"));
            f.run().issues.into_iter().find(|i| i.rule == "output_not_in_run")
        };
        let i = asked(&mut f, "Outputs: YouTube 1080p (16:9), LinkedIn (4:5) and YouTube Shorts (9:16).")
            .expect("Shorts is missing");
        assert_eq!(i.severity, Severity::Warning);
        assert_eq!(i.title, "The spec asks for YouTube Shorts, but this run doesn't make one");
        assert!(i.detail.as_deref().unwrap().starts_with("This run makes YouTube 1080p, LinkedIn."), "{:?}", i.detail);
        assert_eq!(i.fix.as_ref().and_then(|f| f.line), Some(3));
        let i = asked(&mut f, "## Deliverables\n- Shorts\n- TikTok\n- LinkedIn").unwrap();
        assert_eq!(i.title, "The spec asks for outputs this run doesn't make: YouTube Shorts, TikTok");
        let i = asked(&mut f, "Export a 9:16 version too.").unwrap();
        assert_eq!(i.title, "The spec asks for a 9:16 output, but this run doesn't make one");
        assert!(asked(&mut f, "Outputs: YouTube 1080p and LinkedIn (4:5).").is_none());
        assert!(asked(&mut f, "Audience: people who scroll TikTok and Shorts.").is_none(), "not about delivery");
        f.project.spec = format!("{GOOD_SPEC}\nOutputs shown on screen: YouTube Shorts.\n");
        assert!(!f.run().issues.iter().any(|i| i.rule == "output_not_in_run"), "inside the scenes");

        // a watermark in words, with a brand that has none
        f.project.spec = format!("{GOOD_SPEC}\nKeep captions clear of the bottom-right watermark.\n");
        let i = f.run().issues.into_iter().find(|i| i.rule == "watermark_missing").expect("watermark note");
        assert_eq!((i.severity, i.fix.as_ref().and_then(|f| f.line)), (Severity::Info, Some(18)));
        f.project.spec = format!("{GOOD_SPEC}\nNo watermark.\n");
        assert!(!f.run().issues.iter().any(|i| i.rule == "watermark_missing"));
    }

    #[test]
    fn clean_project_has_no_blockers_or_warnings() {
        let f = Fx::new();
        let r = f.run();
        assert_eq!((r.blockers, r.warnings), (0, 0), "{:?}", rules(&r));
        assert!(has(&r, "brand_none", Severity::Info));
        assert_eq!(r.estimates.duration_s, Some(45.0));
        assert_eq!(r.estimates.voice_over_words, 9);
        assert_eq!(r.estimates.eleven_characters, 48);
        assert!(r.estimates.claude_usd_low > 0.0 && r.estimates.claude_usd_high > r.estimates.claude_usd_low);
        assert!(r.estimates.disk_bytes > 200_000_000);
        assert_eq!(r.run_config.as_deref(), Some("rc_yt"));
    }

    #[test]
    fn environment_rules() {
        let mut f = Fx::new();
        f.env.claude = None;
        assert!(has(&f.run(), "claude_missing", Severity::Blocker));
        f.env = FakeEnv::default();
        f.env.claude.as_mut().unwrap().supported = false;
        assert!(has(&f.run(), "claude_unsupported", Severity::Blocker));
        f.env = FakeEnv::default();
        f.env.signed_in = Some(false);
        assert!(has(&f.run(), "claude_signed_out", Severity::Blocker));
        // Windows: no Git Bash means no shell tool for Claude
        f.env = FakeEnv::default();
        f.env.claude.as_mut().unwrap().git_bash = None;
        assert_eq!(has(&f.run(), "git_bash_missing", Severity::Blocker), cfg!(windows));
        f.env = FakeEnv::default();
        f.env.cutline = None;
        assert!(has(&f.run(), "cutline_missing", Severity::Blocker));
        f.env = FakeEnv::default();
        f.env.doctor =
            Some(DoctorSummary { ok: false, missing_required: vec!["filter xfade".into()], ..Default::default() });
        let r = f.run();
        assert!(has(&r, "cutline_doctor", Severity::Blocker));
        assert!(r
            .issues
            .iter()
            .find(|i| i.rule == "cutline_doctor")
            .unwrap()
            .detail
            .as_ref()
            .unwrap()
            .contains("filter xfade"));
        f.env.doctor = None;
        assert!(has(&f.run(), "cutline_doctor", Severity::Blocker));
        f.env.doctor =
            Some(DoctorSummary { ok: true, missing_optional: vec!["tool chrome".into()], ..Default::default() });
        assert!(has(&f.run(), "cutline_optional", Severity::Info));
        f.env = FakeEnv::default();
        f.env.free = Some(1_000);
        assert!(has(&f.run(), "disk_space", Severity::Blocker));
        let need = f.run().estimates.disk_bytes;
        f.env.free = Some(need + need / 2);
        assert!(has(&f.run(), "disk_space", Severity::Warning));
        f.env = FakeEnv::default();
        f.env.writable = false;
        assert!(has(&f.run(), "output_folder_unwritable", Severity::Blocker));
    }

    #[test]
    fn eleven_key_and_credit_rules() {
        let mut f = Fx::new();
        f.env.eleven_key = false;
        assert!(has(&f.run(), "eleven_key_missing", Severity::Blocker));
        assert!(
            !has(&f.run_kind(RunKind::DryRun), "eleven_key_missing", Severity::Blocker),
            "a dry run generates no audio"
        );
        // a recorded voice file means no generated voice is needed
        let voice = f.root().join("vo.wav");
        std::fs::write(&voice, b"wav").unwrap();
        let mut m = MediaItem::with_path("m-vo", MediaKind::Audio, voice.as_str());
        m.role = Some(MediaRole::Voice);
        f.project.media.push(m);
        assert!(!has(&f.run(), "eleven_key_missing", Severity::Blocker));
        f.project.media.pop();
        f.env.eleven_key = true;
        f.env.eleven_remaining = Some(10);
        assert!(has(&f.run(), "eleven_credits_low", Severity::Blocker));
        f.env.eleven_remaining = Some(1_000_000);
        f.rc().eleven_credit_cap = 20;
        assert!(has(&f.run(), "budget_eleven_cap", Severity::Blocker));
        // a generated music brief adds credits
        f.rc().eleven_credit_cap = 20_000;
        let before = f.run().estimates.eleven_credits;
        f.project.spec.push_str("\n```music\nmood: calm\nsource: generated\n```\n");
        assert!(f.run().estimates.eleven_credits > before + 1000);
    }

    #[test]
    fn media_rules() {
        let mut f = Fx::new();
        let hero = f.root().join("hero.mov");
        // changed content
        f.project.media[0].fingerprint = Some(crate::media::fingerprint(&hero).unwrap());
        std::fs::write(&hero, b"other video bytes, edited").unwrap();
        assert!(has(&f.run(), "media_changed", Severity::Warning));
        f.project.media[0].fingerprint = None;
        // zero length
        std::fs::write(&hero, b"").unwrap();
        assert!(has(&f.run(), "media_zero_length", Severity::Blocker));
        std::fs::write(&hero, b"video-bytes").unwrap();
        // probe failures and notes
        f.project.media[0].probe = Some(Probe { error: Some("Invalid data found".into()), ..Probe::default() });
        assert!(has(&f.run(), "media_probe_failed", Severity::Blocker));
        f.project.media[0].probe = Some(Probe {
            duration_s: Some(10.0),
            width: Some(480),
            height: Some(270),
            video_codec: Some("h264".into()),
            vfr: Some(true),
            rotation: Some(90),
            ..Probe::default()
        });
        let r = f.run();
        assert!(
            has(&r, "media_vfr", Severity::Info)
                && has(&r, "media_rotation", Severity::Info)
                && has(&r, "media_low_res", Severity::Warning),
            "{:?}",
            rules(&r)
        );
        f.project.media[0].probe = Some(Probe {
            duration_s: Some(0.0),
            width: Some(1920),
            height: Some(1080),
            video_codec: Some("h264".into()),
            ..Probe::default()
        });
        assert!(has(&f.run(), "media_zero_length", Severity::Blocker));
        // missing: a blocker when the spec uses it, a warning otherwise
        std::fs::remove_file(&hero).unwrap();
        assert!(has(&f.run(), "media_missing", Severity::Blocker));
        f.project.spec = f.project.spec.replace("![hero](media:m-hero)", "a wide shot");
        assert!(has(&f.run(), "media_missing", Severity::Warning));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_media_is_a_blocker() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fx::new();
        let hero = f.root().join("hero.mov");
        std::fs::set_permissions(&hero, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&hero).is_err() {
            assert!(has(&f.run(), "media_unreadable", Severity::Blocker));
        }
        std::fs::set_permissions(&hero, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn spec_rules() {
        let mut f = Fx::new();
        f.project.spec = "  \n".into();
        assert!(has(&f.run(), "spec_empty", Severity::Blocker));
        f.project.spec = GOOD_SPEC.into();
        f.project.intent.kind = IntentKind::Unset;
        assert!(has(&f.run(), "intent_unset", Severity::Blocker));
        f.project.intent.kind = IntentKind::Other;
        assert!(has(&f.run(), "intent_unset", Severity::Blocker));
        f.project.intent.other = Some("Conference loop".into());
        assert!(!has(&f.run(), "intent_unset", Severity::Blocker));
        f.project.intent.kind = IntentKind::Marketing;
        f.project.spec = GOOD_SPEC.replace("media:m-hero", "media:m-nope");
        let r = f.run();
        assert!(has(&r, "spec_unresolved_media", Severity::Blocker));
        assert_eq!(
            r.issues.iter().find(|i| i.rule == "spec_unresolved_media").unwrap().fix.as_ref().unwrap().line,
            Some(5)
        );
        f.project.spec = GOOD_SPEC.replace("Start a free trial.", "{{what next}}");
        assert!(has(&f.run(), "spec_placeholder", Severity::Warning));
        f.project.spec = GOOD_SPEC.replace("## Call to action\nStart a free trial.\n", "");
        assert!(has(&f.run(), "spec_missing_section", Severity::Warning));
        f.project.intent.cta = Some("Start a free trial".into());
        assert!(!has(&f.run(), "spec_missing_section", Severity::Warning));
        f.project.intent.kind = IntentKind::Internal;
        f.project.intent.cta = None;
        assert!(!has(&f.run(), "spec_missing_section", Severity::Warning), "internal updates need no call to action");
        f.project.intent.kind = IntentKind::Marketing;
        f.project.spec = format!("{GOOD_SPEC}\n```vo\n{}\n```\n", "word ".repeat(400));
        assert!(has(&f.run(), "spec_vo_too_long", Severity::Warning));
        f.project.spec = GOOD_SPEC.replace("- **On-screen text:** Try it\n", "");
        assert!(has(&f.run(), "spec_scene_empty", Severity::Warning));
        f.project.spec = GOOD_SPEC.replace("25 s", "200 s");
        assert!(has(&f.run(), "spec_scene_length", Severity::Info));
        f.project.spec = "# T\n\nJust prose.\n\n## Call to action\nGo.\n".into();
        f.project.intent.target_length_s = None;
        assert!(has(&f.run(), "spec_no_length", Severity::Info));
    }

    #[test]
    fn brand_rules() {
        let mut f = Fx::new();
        let lib = f.root().join("lib");
        let logo = f.root().join("logo.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([250, 250, 250, 255])).save(&logo).unwrap();
        let mut b = BrandPreset::new("Acme");
        b.logos.primary = Some(AssetRef::new(&logo));
        b.fonts.heading.file = Some(AssetRef { path: f.root().join("gone.ttf").to_string(), fingerprint: None });
        b.colors.accent = "blue".into();
        b.colors.text = "#EEEEEE".into();
        brand::save(&lib, &mut b).unwrap();
        f.project.brand = Some(BrandRef::of(&b));
        let r = f.run();
        assert!(has(&r, "brand_asset_missing", Severity::Blocker), "{:?}", rules(&r));
        assert!(has(&r, "brand_color_invalid", Severity::Blocker));
        assert!(has(&r, "brand_text_contrast", Severity::Warning));
        assert!(has(&r, "brand_logo_contrast", Severity::Warning), "a near-white logo on a white background");
        assert!(!has(&r, "brand_none", Severity::Info));
        // a font file that isn't a font
        let txt = f.root().join("font.txt");
        std::fs::write(&txt, b"x").unwrap();
        f.project.brand.as_mut().unwrap().snapshot.fonts.heading.file = Some(AssetRef::new(&txt));
        assert!(has(&f.run(), "brand_font_unloadable", Severity::Warning));
        // the library copy moves ahead of the snapshot
        let mut newer = b.clone();
        newer.rules = "New rule".into();
        newer.modified = "2999-01-01T00:00:00Z".into();
        paths::write_json_atomic(&brand::brands_dir(&lib).join(format!("{}.json", b.id)), &newer).unwrap();
        assert!(has(&f.run(), "brand_outdated", Severity::Info));
        // a clean brand raises nothing
        let mut ok = BrandPreset::new("Clean");
        ok.modified = "2999-01-01T00:00:00Z".into();
        f.project.brand = Some(BrandRef::of(&ok));
        let r = f.run();
        assert!(!r.issues.iter().any(|i| i.category == "brand"), "{:?}", rules(&r));
    }

    #[test]
    fn output_rules() {
        let mut f = Fx::new();
        f.rc().outputs.clear();
        assert!(has(&f.run(), "outputs_none", Severity::Blocker));
        f.rc().outputs = vec!["out_gone".into()];
        assert!(has(&f.run(), "output_preset_missing", Severity::Blocker));
        f.project.output_configs.push(OutputConfig {
            id: "out_bad".into(),
            preset: "no_such_preset".into(),
            name: None,
            overrides: OutputOverrides::default(),
        });
        f.rc().outputs = vec!["out_bad".into()];
        assert!(has(&f.run(), "output_preset_missing", Severity::Blocker));
        // duration limits: 45 s is over Reels' 90? no — over the App Store's 30 s, and X is fine
        let add = |f: &mut Fx, id: &str, preset: &str| {
            f.project.output_configs.push(OutputConfig {
                id: id.into(),
                preset: preset.into(),
                name: None,
                overrides: OutputOverrides::default(),
            });
            f.project.run_configs[0].outputs = vec![id.into()];
        };
        add(&mut f, "out_app", "app_store_preview");
        let r = f.run();
        assert!(has(&r, "output_duration_limit", Severity::Blocker), "{:?}", rules(&r));
        assert!(has(&r, "output_aspect", Severity::Warning), "16:9 footage into a 9:19.5 preview");
        f.project.intent.target_length_s = Some(10.0);
        assert!(has(&f.run(), "output_duration_limit", Severity::Blocker), "under the 15 s minimum");
        f.project.intent.target_length_s = Some(20.0);
        assert!(!has(&f.run(), "output_duration_limit", Severity::Blocker));
        f.project.output_configs.last_mut().unwrap().overrides.fps = Some(60.0);
        assert!(has(&f.run(), "output_fps_limit", Severity::Blocker));
        // size limit: an hour at 4K against X's 512 MB
        add(&mut f, "out_x", "x");
        f.project.intent.target_length_s = Some(130.0);
        f.project.output_configs.last_mut().unwrap().overrides.target = Some("long_1080p".into());
        assert!(!has(&f.run(), "output_size_limit", Severity::Warning));
        f.platforms.presets.iter_mut().find(|p| p.id == "x").unwrap().limits.max_bytes = Some(10_000_000);
        assert!(has(&f.run(), "output_size_limit", Severity::Warning));
        // shorts from landscape footage is heavy reframing
        add(&mut f, "out_shorts", "youtube_shorts");
        assert!(has(&f.run(), "output_aspect", Severity::Warning));
        f.project.intent.target_length_s = Some(200.0);
        assert!(has(&f.run(), "output_duration_limit", Severity::Blocker), "Shorts stop at 180 s");
    }

    #[test]
    fn budget_and_model_rules() {
        let mut f = Fx::new();
        f.rc().max_spend_usd = Some(0.0001);
        assert!(has(&f.run(), "budget_claude_cap", Severity::Blocker));
        let (lo, hi) = (f.run().estimates.claude_usd_low, f.run().estimates.claude_usd_high);
        f.rc().max_spend_usd = Some((lo + hi) / 2.0);
        assert!(has(&f.run(), "budget_claude_cap", Severity::Warning));
        f.rc().max_spend_usd = Some(hi * 2.0);
        assert!(!f.run().issues.iter().any(|i| i.rule == "budget_claude_cap"));
        f.project.models.dry_run = "opus".into();
        f.rc().production_model = "haiku".into();
        assert!(has(&f.run(), "dry_run_model", Severity::Info));
        // past runs of the same model family replace the heuristic
        f.rc().production_model = "opus".into();
        for cost in [4.0, 6.0] {
            f.project.runs.push(crate::project::RunRecord {
                id: format!("r{cost}"),
                run_config: "rc_yt".into(),
                run_config_name: None,
                kind: RunKind::Production,
                folder: "/x".into(),
                status: crate::project::RunStatus::Succeeded,
                started: "2026-01-01T00:00:00Z".into(),
                finished: None,
                claude_session: None,
                model: Some("claude-opus-5-5".into()),
                previous_run: None,
                cost_usd: Some(cost),
                eleven_credits: None,
            });
        }
        let e = f.run().estimates;
        assert!((e.claude_usd_low - 3.0).abs() < 1e-9 && (e.claude_usd_high - 8.0).abs() < 1e-9);
    }

    #[test]
    fn findings_travel_to_the_assistant_split_by_where_they_are_fixed() {
        let issue = |id: &str, sev, category: &str, title: &str, fix: Option<FixLink>| Issue {
            id: id.into(),
            rule: id.into(),
            severity: sev,
            category: category.into(),
            title: title.into(),
            detail: Some("Two lines\nof detail".into()),
            fix,
        };
        let mut r = Report {
            generated: "2026-11-15T10:00:00Z".into(),
            issues: vec![
                issue(
                    "vo_long",
                    Severity::Warning,
                    "spec",
                    "Scene 2 narration is too long",
                    Some(FixLink { kind: "spec".into(), target: None, line: Some(31) }),
                ),
                issue("voice_unset", Severity::Blocker, "brand", "No voice is chosen", FixLink::to("brand")),
            ],
            ..Default::default()
        };
        r.recount();
        let c = r.assistant_context(None);
        let (spec, outside) = c.split_once("Outside the spec").unwrap();
        assert!(
            spec.contains("- [warning] Scene 2 narration is too long (spec.md line 31) — Two lines of detail"),
            "{c}"
        );
        assert!(
            outside.contains("- [blocker] No voice is chosen") && outside.contains("must not write them into it"),
            "{c}"
        );
        // one finding on its own, and nothing when there is nothing
        let one = r.assistant_context(Some("vo_long"));
        assert!(one.contains("Scene 2") && !one.contains("No voice"));
        assert!(r.fix_request(Some("vo_long")).contains("\"Scene 2 narration is too long\""));
        assert!(r.fix_request(None).contains("blockers first"));
        assert_eq!(Report::default().assistant_context(None), "");
        assert_eq!(r.assistant_context(Some("gone")), "");
    }

    #[test]
    fn acknowledging_warnings_and_dry_run_findings() {
        let mut f = Fx::new();
        f.project.spec = GOOD_SPEC.replace("Start a free trial.", "TODO write this");
        let mut r = f.run();
        let unack = r.unacknowledged_warnings(&[]);
        assert_eq!(unack.len(), 1);
        let id = unack[0].id.clone();
        assert!(r.unacknowledged_warnings(std::slice::from_ref(&id)).is_empty());
        assert_eq!(
            f.run().issues.iter().find(|i| i.rule == "spec_placeholder").unwrap().id,
            id,
            "issue IDs are stable between validations"
        );
        let readiness = ReadinessReport {
            ambiguities: vec![crate::run::readiness::Finding {
                text: "Which logo opens the video?".into(),
                spec_line: Some(3),
                detail: None,
            }],
            assumptions: vec![crate::run::readiness::Finding {
                text: "Used the primary logo".into(),
                spec_line: None,
                detail: None,
            }],
            ..Default::default()
        };
        r.add_dry_run(&readiness);
        assert_eq!(r.issues.iter().filter(|i| i.category == "dry_run").count(), 2);
        assert_eq!(
            r.issues.iter().find(|i| i.rule == "dry_run_ambiguity").unwrap().fix.as_ref().unwrap().line,
            Some(3)
        );
        assert_eq!(r.warnings, 3);
        r.add_dry_run(&readiness);
        assert_eq!(r.warnings, 3, "adding again replaces the old findings");
        assert!(!r.has_blockers());
    }
}
