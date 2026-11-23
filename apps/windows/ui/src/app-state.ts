// App-wide data every window shares (as the macOS AppState): settings, the model list, output
// presets, brand presets, the detected environment and the ElevenLabs key status.

import { core, request, on, errorText } from './bridge';
import { Signal } from './store';
import type { BrandPreset, CodexStatus, Environment, KeyStatus, ModelList, Platforms, Settings } from './types';

export const settings = new Signal<Settings | null>(null);
export const models = new Signal<ModelList | null>(null);
export const platforms = new Signal<Platforms | null>(null);
export const brands = new Signal<BrandPreset[]>([]);
export const environment = new Signal<Environment | null>(null);
export const keyStatus = new Signal<KeyStatus | null>(null);
export const libraryDir = new Signal<string>('');

export async function reloadLibrary() {
  const [s, m, p, b, dir] = await Promise.all([
    core<Settings>('settings.load'),
    core<ModelList>('models.list'),
    core<Platforms>('platforms.list'),
    core<BrandPreset[]>('brand.list'),
    core<string>('library.dir'),
  ]);
  settings.value = s;
  models.value = m;
  platforms.value = p;
  brands.value = b;
  libraryDir.value = dir;
}

export async function saveSettings(change: (s: Settings) => void) {
  const s = structuredClone(settings.value!);
  change(s);
  // The shell sends the saved settings to every other open window (`settings_changed`).
  applySettings(await core<Settings>('settings.save', { settings: s }));
}

/** New settings, saved here or in another window: a provider switch re-checks readiness. */
function applySettings(s: Settings) {
  const before = settings.value ? provider() : null;
  settings.value = s;
  if (before !== null && provider() !== before) providerChanged();
}

// Settings are app-wide (FEAT-spry-alpaca-5RQZ): another window saved them.
on('settings_changed', (s: Settings) => {
  if (s && typeof s === 'object') applySettings(s);
});

let lastDetect = 0;
let detecting: Promise<void> | null = null;

/** Detects Claude Code, cutline and the key (throttled to 20 s unless forced), and checks Codex
 *  when it is the selected provider. */
export function detect(force = false): Promise<void> {
  if (provider() === 'codex' && !codexChecking.value && (force || !codexStatus.value)) checkCodex();
  if (detecting) return detecting;
  if (!force && Date.now() - lastDetect < 20000 && environment.value) return Promise.resolve();
  detecting = (async () => {
    try {
      const [env, key] = await Promise.all([
        core<Environment>('environment.detect'),
        request<KeyStatus>('secret.status').catch(() => ({ present: false }) as KeyStatus),
      ]);
      environment.value = env;
      keyStatus.value = key;
      lastDetect = Date.now();
      readiness.value++;
    } finally {
      detecting = null;
    }
  })();
  return detecting;
}

/** Claude Code specifically (its Settings pane, the Claude row of the checklist). */
export function claudeAvailable(): boolean {
  const e = environment.value;
  return !!e?.claude && e.claude.supported && e.signed_in !== false && (!!e.claude.git_bash || !isWindows());
}

// MARK: The selected provider (Settings › Assistant, FEAT-spry-alpaca-5RQZ)

/** Codex's readiness from `codex.check`, when Codex is the provider. */
export const codexStatus = new Signal<CodexStatus | null>(null);
export const codexChecking = new Signal<boolean>(false);
/** Bumped whenever the selected provider's readiness may have changed: views that enable
 *  Assistant actions re-render on it. */
export const readiness = new Signal<number>(0);
/** Bumped by every provider switch and Codex check, so a late answer from an older check can't
 *  overwrite the readiness of the provider selected now. */
let readinessGeneration = 0;

/** The provider new assistant turns and runs start with; the core reads it again at start. */
export function provider(): 'claude' | 'codex' {
  return settings.value?.provider === 'codex' ? 'codex' : 'claude';
}

/** The provider's product name, for provider-specific setup and diagnostics only. */
export function providerName(): string {
  return provider() === 'codex' ? 'Codex' : 'Claude Code';
}

/** The executable the selected provider is looked for as: the configured path, else its name. */
export function providerExecutable(): string {
  const s = settings.value;
  const configured = ((provider() === 'codex' ? s?.codex_path : s?.claude_path) ?? '').trim();
  return configured || (provider() === 'codex' ? 'codex' : 'claude');
}

/** Whether the selected provider can start assistant turns and runs now. */
export function assistantAvailable(): boolean {
  return provider() === 'codex' ? codexStatus.value?.state === 'ready' : claudeAvailable();
}

/** Whether the selected provider's check has answered. */
export function assistantChecked(): boolean {
  return provider() === 'codex' ? !!codexStatus.value : !!environment.value;
}

/** Why the selected provider can't be used, naming the executable, and the Settings pane to fix it. */
export function assistantSetupProblem(): { title: string; detail: string; pane: 'assistant' | 'claude' } {
  const exe = providerExecutable();
  if (provider() === 'codex') {
    const f = codexStatus.value?.finding;
    if (!codexStatus.value) return { title: 'Checking Codex…', detail: `Looking for ${exe}.`, pane: 'assistant' };
    return { title: f?.title ?? 'Codex isn’t ready', detail: `Looked for ${exe}. ${f?.remedy ?? 'Install the Codex CLI, or set its path in Settings › Assistant, then re-check.'}`, pane: 'assistant' };
  }
  const e = environment.value;
  const c = e?.claude;
  const detail = !c
    ? `Looked for ${exe}. Install Claude Code, or set its path in Settings › Claude, then re-check.`
    : !c.supported
      ? `${c.version_text} (${c.path}) is too old: ${e?.min_claude_version} or later is needed.`
      : !c.git_bash && isWindows()
        ? `${c.path} needs Git for Windows (Git Bash) to run commands.`
        : `${c.path} isn’t signed in. Run claude in a terminal and sign in.`;
  return { title: 'Claude Code isn’t available', detail, pane: 'claude' };
}

/** A provider switch: drop the old provider's readiness and check the new one. Runs and turns
 *  already started keep the provider they started with. */
function providerChanged() {
  readinessGeneration++;
  codexStatus.value = null;
  codexChecking.value = false;
  readiness.value++;
  if (provider() === 'codex') checkCodex();
  else detect(true).catch(() => {});
}

/** Asks Codex whether it is ready (Settings › Assistant › Re-check, a switch, focus). */
export async function checkCodex(): Promise<void> {
  const generation = ++readinessGeneration;
  codexChecking.value = true;
  let status: CodexStatus;
  try {
    status = await core<CodexStatus>('codex.check', { path: settings.value?.codex_path ?? '' });
  } catch (e) {
    status = { state: 'missing', finding: { code: 'codex_check_failed', title: 'Codex couldn’t be checked', remedy: errorText(e) } };
  }
  // A newer switch or check has started since: this answer is stale.
  if (generation !== readinessGeneration || provider() !== 'codex') return;
  codexStatus.value = status;
  codexChecking.value = false;
  readiness.value++;
}

function isWindows() {
  return navigator.userAgent.includes('Windows');
}

/** A model's display name: the family for an alias, the version for a full ID. */
export function modelName(id: string | undefined | null): string {
  if (!id) return 'Default';
  for (const f of models.value?.families ?? []) {
    if (f.alias === id || f.id === id) return `${f.name} (latest)`;
    const v = f.versions.find((v) => v.id === id);
    if (v) return v.name;
  }
  return id;
}

// Re-detect when the window comes back to the front, as the macOS app does on activation.
window.addEventListener('focus', () => {
  detect().catch(() => {});
});
