// Core JSON shapes the UI reads. The project model is generated from the core's JSON Schema
// (src/generated/project.ts); the rest mirror the core's serde types, which the macOS shell
// decodes the same way (apps/macos/Sources/VideoStudio/Core/Models.swift).

import type * as G from './generated/project';

/**
 * The schema marks `#[serde(default)]` fields optional, but the core always writes them: these
 * are the generated types with every property present (a `null` stays possible where the core
 * writes one).
 */
export type Filled<T> = T extends (infer U)[]
  ? Filled<U>[]
  : T extends object
    ? { [K in keyof T]-?: Filled<NonNullable<T[K]>> | Extract<T[K], null> }
    : T;

export type Project = Filled<G.Project>;
export type MediaItem = Filled<G.MediaItem>;
export type RunConfig = Filled<G.RunConfig>;
export type RunRecord = Filled<G.RunRecord>;
export type OutputConfig = Filled<G.OutputConfig>;
export type OutputOverrides = Filled<G.OutputOverrides>;
export type BrandPreset = Filled<G.BrandPreset>;
export type BrandRef = Filled<G.BrandRef>;
export type AssetRef = Filled<G.AssetRef>;
export type Intent = Filled<G.Intent>;
export type Finish = Filled<G.Finish>;
export type { MediaKind, MediaRole, IntentKind, RunStatus, RunKind } from './generated/project';

export interface Settings {
  default_output_folder: string;
  parallel_run_limit: number;
  media_roots: string[];
  notifications: boolean;
  recent_output_folders: string[];
  claude_path: string;
  default_assistant_model: string;
  default_dry_run_model: string;
  default_production_model: string;
  effort: string;
  web_access_default: boolean;
  headless_question_timeout_s: number;
  cutline_path: string;
  cutline_repo: string;
  default_quality: string;
  eleven_default_voice: string;
  eleven_default_credit_cap: number;
  eleven_cache_limit_bytes: number;
  words_per_minute: number;
  /** How videos are finished unless a project keeps its own. */
  finish: Finish;
  cleanup_older_than_days: number;
  onboarding_done: boolean;
}

export interface ModelVersion { id: string; name: string }
export interface ModelFamily { id: string; name: string; alias: string; tier: string; versions: ModelVersion[] }
export interface ModelList { version: number; updated: string; families: ModelFamily[]; defaults: { assistant: string; dry_run: string; production: string } }

export interface Limits { min_duration_s?: number; max_duration_s?: number; max_bytes?: number; max_fps?: number }
export interface NamedSize { name: string; width: number; height: number }
export interface OutputPreset {
  id: string; name: string; platform: string; targets: string[]; width?: number; height?: number; fps?: number;
  sizes: NamedSize[]; captions: string; audio: boolean; loop: boolean; max_bytes?: number; formats: string[];
  limits: Limits; notes: string; user: boolean;
}
export interface IntentDefaults { template: string; outputs: string[]; captions: string; pacing: string; music_level: string }
export interface Platforms { version: number; updated: string; presets: OutputPreset[]; intents: Record<string, IntentDefaults> }
export interface ResolvedOutput {
  id: string; name: string; slug: string; preset: string; platform: string; target: string; width: number; height: number;
  fps?: number; audio: boolean; loop: boolean; captions: string; max_bytes?: number; limits: Limits; notes: string;
  error?: string;
}

export interface ClaudeInfo { path: string; version: string; version_text: string; effort_levels: string[]; supported: boolean; git_bash?: string }
export interface CutlineInfo { path: string; version: string }
export interface DoctorSummary { ok: boolean; missing_required: string[]; missing_optional: string[] }
export interface Environment {
  claude: ClaudeInfo | null;
  signed_in: boolean | null;
  cutline: CutlineInfo | null;
  doctor: DoctorSummary | null;
  min_claude_version: string;
}
export interface KeyStatus { present: boolean; masked?: string | null }

export interface Resolution { id: string; status: 'ok' | 'moved' | 'changed' | 'missing'; path?: string | null; found_by?: string | null; stored_path?: string | null }
export interface OutlineItem { kind: 'scene' | 'heading'; title: string; level: number; line: number }
export interface LintIssue { rule: string; severity: 'warning' | 'info'; message: string; line: number }

export interface FixLink { kind: string; target?: string; line?: number }
export interface Issue { id: string; rule: string; severity: 'blocker' | 'warning' | 'info'; category: string; title: string; detail?: string; fix?: FixLink }
export interface Estimates {
  duration_s?: number; voice_over_words: number; eleven_characters: number; eleven_credits: number; eleven_cap: number;
  eleven_remaining?: number; claude_usd_low: number; claude_usd_high: number; disk_bytes: number; free_bytes?: number;
}
export interface Finding { text: string; spec_line?: number; detail?: string }
export interface Readiness {
  ready: boolean; ambiguities: Finding[]; assumptions: Finding[]; missing_media: Finding[]; risks: Finding[];
  /** What the production run records or generates itself; not a problem. */
  planned?: Finding[];
  estimated_durations: Record<string, number>; notes?: string;
}
export interface Report {
  generated: string; project_id: string; run_config?: string; issues: Issue[]; estimates: Estimates;
  blockers: number; warnings: number; infos: number; dry_run?: Readiness;
}

export interface Hunk { old_start: number; old_lines: string[]; new_lines: string[] }

export interface QuestionOption { label: string; description?: string }
export interface ClaudeQuestion { question: string; header?: string; options: QuestionOption[]; multiSelect?: boolean; multi_select?: boolean }

export interface QaFinding { check: string; severity: string; message: string }
export interface OutputFile {
  output_id: string; output_name: string; platform: string; export: string; path: string; exists: boolean; bytes: number;
  width: number; height: number; incomplete: boolean; qa_verdict: string; qa_findings: QaFinding[]; captions_srt?: string;
  poster?: string; duration_s?: number;
}
export interface LogFile { name: string; path: string; bytes: number; source: string }
export interface RunResults {
  folder: string; exists: boolean; run: any; outputs: OutputFile[]; summary?: string; readiness?: Readiness;
  project_json5?: string; logs: LogFile[];
}

export interface BrandSummary { id: string; name: string }

export const INTENTS: [string, string][] = [
  ['marketing', 'Marketing'],
  ['product_demo', 'Product demo'],
  ['tutorial', 'Tutorial or how-to'],
  ['explainer', 'Explainer'],
  ['social_clip', 'Social clip'],
  ['testimonial', 'Testimonial'],
  ['announcement', 'Announcement'],
  ['event_recap', 'Event recap'],
  ['internal', 'Internal communication'],
  ['app_store_preview', 'App store preview'],
  ['other', 'Other'],
];

export const MEDIA_KINDS: [string, string][] = [
  ['video', 'Video'],
  ['audio', 'Audio'],
  ['image', 'Images'],
  ['font', 'Fonts'],
  ['transcript', 'Transcripts'],
  ['tape', 'Terminal tapes'],
  ['demo_script', 'Demo scripts'],
  ['other', 'Other'],
];

export const MEDIA_ROLES: [string, string][] = [
  ['', 'Not set'],
  ['footage', 'Footage'],
  ['b_roll', 'B-roll'],
  ['screen_recording', 'Screen recording'],
  ['logo', 'Logo'],
  ['music', 'Music'],
  ['sfx', 'Sound effect'],
  ['voice', 'Voice'],
  ['reference', 'Reference'],
];

export const STATUS_LABEL: Record<string, string> = {
  running: 'Running',
  succeeded: 'Succeeded',
  succeeded_with_warnings: 'Succeeded with warnings',
  failed: 'Failed',
  cancelled: 'Cancelled',
};
