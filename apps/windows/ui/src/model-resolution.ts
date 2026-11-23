// The model the next operation uses, for display, with the same precedence as the core's
// `project::effective_model` (FFI `models.effective`, BUG-fond-crane-62ZP). Pure, so it can be
// checked without the app: an override for the active provider, then the project's (or run
// config's) choice for that provider, then that provider's default in Settings, then its CLI
// default. Claude always gets a model; for Codex `null` means Codex's own default. Claude's choices
// are never used for Codex and stay where they are for when Claude is selected again.

export type ModelUse = 'assistant' | 'dry_run' | 'production';

export interface ModelChoices {
  /** Claude's choices. */
  assistant?: string | null;
  dry_run?: string | null;
  /** Codex's choices; empty follows Settings › Codex. */
  codex_assistant?: string | null;
  codex_dry_run?: string | null;
}

export interface RunConfigChoices {
  production_model?: string | null;
  codex_production_model?: string | null;
}

export interface SettingsChoices {
  default_assistant_model?: string | null;
  default_dry_run_model?: string | null;
  default_production_model?: string | null;
  codex_model?: string | null;
}

const given = (s: string | null | undefined): string | null => {
  const t = (s ?? '').trim();
  return t ? t : null;
};

export function effectiveModel(
  models: ModelChoices | null | undefined,
  runConfig: RunConfigChoices | null | undefined,
  settings: SettingsChoices | null | undefined,
  provider: 'claude' | 'codex',
  use: ModelUse,
  override?: string | null,
): string | null {
  const o = given(override);
  if (o) return o;
  if (provider === 'codex') {
    const own = use === 'assistant' ? models?.codex_assistant : use === 'dry_run' ? models?.codex_dry_run : runConfig?.codex_production_model;
    return given(own) ?? given(settings?.codex_model);
  }
  switch (use) {
    case 'assistant':
      return given(models?.assistant) ?? given(settings?.default_assistant_model) ?? 'sonnet';
    case 'dry_run':
      return given(models?.dry_run) ?? given(settings?.default_dry_run_model) ?? 'haiku';
    default:
      return given(runConfig?.production_model) ?? given(settings?.default_production_model) ?? 'opus';
  }
}

/** What a picker or header shows for an effective model: "Codex default" when Codex chooses, a
 *  Codex model's name from its list (else its ID), or Claude's display name. */
export function modelLabel(effective: string | null, provider: 'claude' | 'codex', codexModels: { id: string; name: string }[], claudeName: (id: string) => string): string {
  if (effective === null) return provider === 'codex' ? 'Codex default' : 'Default';
  if (provider === 'codex') return codexModels.find((m) => m.id === effective)?.name ?? effective;
  return claudeName(effective);
}

/** The label of a Codex picker's empty choice: it follows Settings › Codex, which may itself be
 *  Codex's default. */
export function codexEmptyChoiceLabel(settingsCodexModel: string | null | undefined, codexModels: { id: string; name: string }[]): string {
  const s = given(settingsCodexModel);
  return s ? `Settings default (${modelLabel(s, 'codex', codexModels, (x) => x)})` : 'Codex default';
}
