// App-wide data every window shares (as the macOS AppState): settings, the model list, output
// presets, brand presets, the detected environment and the ElevenLabs key status.

import { core, request } from './bridge';
import { Signal } from './store';
import type { BrandPreset, Environment, KeyStatus, ModelList, Platforms, Settings } from './types';

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
  settings.value = await core<Settings>('settings.save', { settings: s });
}

let lastDetect = 0;
let detecting: Promise<void> | null = null;

/** Detects Claude Code, cutline and the key (throttled to 20 s unless forced). */
export function detect(force = false): Promise<void> {
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
    } finally {
      detecting = null;
    }
  })();
  return detecting;
}

export function claudeAvailable(): boolean {
  const e = environment.value;
  return !!e?.claude && e.claude.supported && e.signed_in !== false && (!!e.claude.git_bash || !isWindows());
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
