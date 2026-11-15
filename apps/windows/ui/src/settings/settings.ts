// The Settings window (spec §18; Ctrl+,): General, Claude, cutline, ElevenLabs, Storage, Editor.
// Settings are the library's settings.json, shared with the CLI.

import { h, clear, button, iconButton, Icons, field, textInput, numberInput, select, checkbox, bytes, spinner, when } from '../dom';
import { core, request, errorText, shellInfo } from '../bridge';
import { Signal, effect } from '../store';
import { settings, saveSettings, environment, keyStatus, detect, models, libraryDir } from '../app-state';
import { openFolders, openFiles, confirm, toast, Filters } from '../dialogs';
import { modelPicker, effortPicker } from '../widgets';
import type { Settings } from '../types';

type Pane = 'general' | 'claude' | 'cutline' | 'elevenlabs' | 'storage' | 'editor';
const PANES: [Pane, string, string][] = [
  ['general', 'General', Icons.settings],
  ['claude', 'Claude', Icons.chat],
  ['cutline', 'cutline', Icons.video],
  ['elevenlabs', 'ElevenLabs', Icons.audio],
  ['storage', 'Storage', Icons.folder],
  ['editor', 'Editor', Icons.edit],
];

export async function mount(root: HTMLElement) {
  const info = await shellInfo();
  const want = (info.options as { pane?: Pane } | null)?.pane;
  const pane = new Signal<Pane>(PANES.some(([p]) => p === want) ? want! : 'general');
  const nav = h('nav', { class: 'settings-nav', role: 'tablist', 'aria-orientation': 'vertical', 'aria-label': 'Settings' });
  const body = h('div', { class: 'settings-body', role: 'tabpanel' });
  clear(root, h('div', { class: 'settings' }, nav, body));
  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.key === 'w') {
      e.preventDefault();
      request('shell.close_window');
    }
  });
  let cutlineOutput = '';
  let keyTest = '';
  let storage: any = null;

  effect([pane, settings, environment, keyStatus, models], () => {
    clear(nav, PANES.map(([p, label]) => h('button', { role: 'tab', id: `pane-${p}`, 'aria-selected': String(pane.value === p), class: pane.value === p ? 'selected' : '', onclick: () => (pane.value = p) }, label)));
    body.setAttribute('aria-labelledby', `pane-${pane.value}`);
    const s = settings.value;
    if (!s) return clear(body, spinner());
    const save = (f: (x: Settings) => void) => saveSettings(f).catch((e) => toast(errorText(e), 'error'));
    const env = environment.value;
    switch (pane.value) {
      case 'general':
        clear(body, h('h2', null, 'General'),
          field('Default output folder', h('div', { class: 'row' }, h('span', { class: 'path grow' }, s.default_output_folder || 'Videos\\Syncfusion Video Studio'),
            button('Choose…', async () => {
              const [d] = await openFolders('Default output folder');
              if (d) save((x) => (x.default_output_folder = d));
            }),
            button('Reset', () => save((x) => (x.default_output_folder = '')), { appearance: 'subtle' }))),
          field('Runs at once', numberInput(s.parallel_run_limit, (v) => save((x) => (x.parallel_run_limit = Math.max(1, Math.min(4, v ?? 1)))), { min: 1, max: 4 }), 'Runs of different projects; renders are CPU-heavy.'),
          checkbox('Notify when a run finishes or Claude has a question', s.notifications, (v) => save((x) => (x.notifications = v))),
          field('Reading speed (words per minute)', numberInput(s.words_per_minute, (v) => save((x) => (x.words_per_minute = v ?? 150)), { min: 60, max: 300 })),
          h('h3', null, 'Media search folders (all projects)'),
          h('ul', { class: 'plain-list' }, s.media_roots.map((r, i) => h('li', { class: 'row' }, h('span', { class: 'path grow' }, r), iconButton(Icons.remove, `Remove ${r}`, () => save((x) => x.media_roots.splice(i, 1)))))),
          button('Add Folder…', async () => {
            const dirs = await openFolders('Media search folder', true);
            if (dirs.length) save((x) => (x.media_roots = [...new Set([...x.media_roots, ...dirs])]));
          }, { glyph: Icons.add }));
        break;
      case 'claude': {
        const c = env?.claude;
        const status = !env ? 'Checking…' : !c ? 'Claude Code wasn’t found.' : !c.supported ? `${c.version_text} is too old: ${env.min_claude_version} or later is needed.` : !c.git_bash ? `${c.version_text}, but Git for Windows wasn’t found (Claude Code runs commands through Git Bash).` : env.signed_in === false ? `${c.version_text}, not signed in. Run claude in a terminal and sign in.` : `${c.version_text} — ${c.path}`;
        clear(body, h('h2', null, 'Claude'),
          h('div', { class: 'row' }, h('p', { class: 'grow', role: 'status' }, status), button('Re-check', () => detect(true), { glyph: Icons.refresh })),
          c?.git_bash ? h('p', { class: 'muted small' }, `Git Bash: ${c.git_bash}`) : null,
          field('Claude Code path', textInput(s.claude_path, (v) => save((x) => (x.claude_path = v)).then(() => detect(true)), { placeholder: 'Detect automatically' })),
          h('h3', null, 'Default models for new projects'),
          field('Spec assistant', modelPicker(s.default_assistant_model, (v) => save((x) => (x.default_assistant_model = v)), { label: 'Default spec assistant model' })),
          field('Dry run', modelPicker(s.default_dry_run_model, (v) => save((x) => (x.default_dry_run_model = v)), { label: 'Default dry run model' })),
          field('Production', modelPicker(s.default_production_model, (v) => save((x) => (x.default_production_model = v)), { label: 'Default production model' })),
          (() => {
            const e = effortPicker(s.effort || null, (v) => save((x) => (x.effort = v ?? '')), 'Default effort');
            return e ? field('Effort', e) : null;
          })(),
          h('p', { class: 'muted small' }, `Model list updated ${models.value?.updated ?? '—'}. To refresh it, place a newer models.json in the library folder.`),
          h('div', { class: 'row' }, button('Refresh', async () => {
            models.value = await core('models.list');
          }), button('Reveal Library', () => request('shell.reveal', { path: libraryDir.value }))),
          checkbox('Allow web access by default', s.web_access_default, (v) => save((x) => (x.web_access_default = v))),
          field('Question timeout in headless runs (seconds, 0 = wait)', numberInput(s.headless_question_timeout_s, (v) => save((x) => (x.headless_question_timeout_s = v ?? 0)), { min: 0 })));
        break;
      }
      case 'cutline': {
        const cl = env?.cutline;
        const d = env?.doctor;
        clear(body, h('h2', null, 'cutline'),
          h('div', { class: 'row' }, h('p', { class: 'grow', role: 'status' }, !env ? 'Checking…' : cl ? `cutline ${cl.version} — ${cl.path}` : 'cutline wasn’t found.'), button('Re-check', () => detect(true), { glyph: Icons.refresh })),
          d && d.missing_required.length ? h('p', { class: 'danger' }, `Missing: ${d.missing_required.join(', ')}`) : null,
          d && d.missing_optional.length ? h('p', { class: 'muted small' }, `Not installed (optional): ${d.missing_optional.join(', ')}`) : null,
          field('cutline path', h('div', { class: 'row' }, textInput(s.cutline_path, (v) => save((x) => (x.cutline_path = v)).then(() => detect(true)), { placeholder: 'cutline.exe on PATH', class: 'grow' }), button('Browse…', async () => {
            const [f] = await openFiles('cutline.exe', [{ name: 'cutline', spec: 'cutline.exe' }, ...Filters.any]);
            if (f) save((x) => (x.cutline_path = f)).then(() => detect(true));
          }))),
          field('cutline repository', textInput(s.cutline_repo, (v) => save((x) => (x.cutline_repo = v)), { placeholder: 'Optional: a source checkout' })),
          field('Default quality', select([['final', 'Final'], ['draft', 'Draft']], s.default_quality, (v) => save((x) => (x.default_quality = v)))),
          h('div', { class: 'row' },
            button('Run Doctor', () => runCutline('doctor'), { disabled: !cl }),
            button('Install Dependencies', () => runCutline('install_deps'), { disabled: !cl })),
          cutlineOutput ? h('pre', { class: 'log-view' }, cutlineOutput) : null);
        break;
      }
      case 'elevenlabs': {
        const k = keyStatus.value;
        const input = h('input', { type: 'password', placeholder: 'Paste a new key', 'aria-label': 'New ElevenLabs API key', autocomplete: 'off' }) as HTMLInputElement;
        clear(body, h('h2', null, 'ElevenLabs'),
          h('p', { role: 'status' }, k?.present ? `Key: ${k.masked} (stored in Windows Credential Manager)` : 'No key stored.'),
          h('div', { class: 'row' },
            button('Test', async () => {
              keyTest = 'Testing…';
              pane.emit();
              try {
                const r = await request<any>('secret.test');
                keyTest = `The key works. ${r.remaining} of ${r.limit} credits remain (${r.tier}).`;
              } catch (e) {
                keyTest = errorText(e);
              }
              pane.emit();
            }, { disabled: !k?.present }),
            button('Remove', async () => {
              await request('secret.delete');
              keyTest = '';
              await detect(true);
            }, { appearance: 'subtle', disabled: !k?.present })),
          keyTest ? h('p', { class: 'small' }, keyTest) : null,
          h('div', { class: 'row' }, input, button('Save', async () => {
            const v = input.value.trim();
            if (!v) return;
            try {
              await request('secret.set', { value: v });
              input.value = '';
              await detect(true);
              toast('Key stored.');
            } catch (e) {
              toast(errorText(e), 'error');
            }
          }, { appearance: 'primary' })),
          h('p', { class: 'muted small' }, 'The key is read only by the bundled helper during runs; Claude never sees it.'),
          field('Default voice ID', textInput(s.eleven_default_voice, (v) => save((x) => (x.eleven_default_voice = v)))),
          field('Default credit cap per run', numberInput(s.eleven_default_credit_cap, (v) => save((x) => (x.eleven_default_credit_cap = v ?? 0)), { min: 0 })),
          field('Cache size limit (GB)', numberInput(Math.round(s.eleven_cache_limit_bytes / 1e8) / 10, (v) => save((x) => (x.eleven_cache_limit_bytes = Math.round(Math.max(0.1, v ?? 2) * 1e9))), { min: 0.1, step: '0.1' })),
          h('div', { class: 'row' }, h('span', { class: 'muted' }, storage ? `Using ${bytes(storage.eleven_cache_bytes)}` : ''), button('Clear Cache', async () => {
            await core('storage.clear_eleven_cache');
            storage = null;
            loadStorage();
          })));
        if (!storage) loadStorage();
        break;
      }
      case 'storage':
        clear(body, h('h2', null, 'Storage'),
          storage ? h('dl', { class: 'estimates' },
            h('dt', null, 'ElevenLabs cache'), h('dd', null, bytes(storage.eleven_cache_bytes)),
            h('dt', null, 'Dry runs'), h('dd', null, bytes(storage.dry_runs_bytes)),
            h('dt', null, 'Runs'), h('dd', null, `${storage.runs.length} run folder(s)`)) : spinner(),
          field('Delete runs older than (days)', numberInput(s.cleanup_older_than_days, (v) => save((x) => (x.cleanup_older_than_days = v ?? 30)), { min: 1 })),
          button('Clean Up…', async () => {
            const folders = (storage?.runs ?? []).map((r: any) => r.folder);
            const c = await confirm('Delete old runs?', `Run folders older than ${s.cleanup_older_than_days} days are deleted, with their outputs and logs.`, ['Delete', 'Cancel'], true);
            if (c !== 0) return;
            const r = await core('storage.cleanup', { run_folders: folders, days: s.cleanup_older_than_days });
            toast(`Cleaned up: ${JSON.stringify(r)}`);
            storage = null;
            loadStorage();
          }),
          storage ? h('ul', { class: 'plain-list small' }, storage.runs.slice(0, 40).map((r: any) => h('li', { class: 'row' }, h('span', { class: 'grow path' }, r.folder), h('span', { class: 'muted' }, `${bytes(r.total_bytes)} · ${when(r.started)}`), r.exists ? iconButton(Icons.openFolder, 'Reveal', () => request('shell.reveal', { path: r.folder })) : null))) : null);
        if (!storage) loadStorage();
        break;
      case 'editor':
        clear(body, h('h2', null, 'Editor'),
          h('p', { class: 'muted' }, 'The editor follows the Windows theme and text scaling. Line wrapping, line numbers and spell checking are on; the preview uses the same theme.'),
          checkbox('Wrap long lines', localStorage.getItem('editor.wrap') !== 'off', (v) => localStorage.setItem('editor.wrap', v ? 'on' : 'off')));
        break;
    }
  });

  async function runCutline(action: 'doctor' | 'install_deps') {
    cutlineOutput = action === 'doctor' ? 'Running cutline doctor…' : 'Installing dependencies… this can take a few minutes.';
    pane.emit();
    try {
      const r = await request<{ code: number; output: string }>('shell.cutline', { action });
      cutlineOutput = `${r.output}\n(exit ${r.code})`;
    } catch (e) {
      cutlineOutput = errorText(e);
    }
    pane.emit();
    detect(true);
  }

  async function loadStorage() {
    try {
      storage = await core('storage.info', { run_folders: [] });
      pane.emit();
    } catch {
      /* shown as unavailable */
    }
  }
}
