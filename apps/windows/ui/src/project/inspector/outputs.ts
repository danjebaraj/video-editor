// Inspector › Outputs (spec §9): run configs (switcher, new, duplicate, rename, delete), the
// outputs in each, production settings, the output folder with recents, and limits; plus the
// output editor sheet.

import { h, clear, field, textInput, numberInput, select, checkbox, button, iconButton, Icons, emptyState, setDisabled } from '../../dom';
import { core, request } from '../../bridge';
import { effect } from '../../store';
import { platforms, settings, saveSettings } from '../../app-state';
import { prompt, confirm, openFolders, sheet } from '../../dialogs';
import { modelPicker, effortPicker } from '../../widgets';
import type { ProjectWindow } from '../window';
import type { OutputConfig, RunConfig, OutputPreset } from '../../types';

function hexId(prefix: string): string {
  const b = crypto.getRandomValues(new Uint8Array(8));
  return `${prefix}_${[...b].map((x) => x.toString(16).padStart(2, '0')).join('')}`;
}

export function outputsTab(w: ProjectWindow): HTMLElement {
  const root = h('div', { class: 'form-tab' });
  const d = w.doc;
  let outputRoot = '';

  const refreshRoot = async () => {
    const rc = d.currentRunConfig();
    outputRoot = rc ? await core<string>('outputs.root', { project: d.p, run_config: rc.id }).catch(() => '') : '';
  };

  effect([d.project, d.runConfig, d.outputs, platforms], () => {
    refreshRoot().then(() => render());
  });

  function addRunConfig(name: string) {
    const cur = d.currentRunConfig();
    const s = settings.value!;
    const rc: RunConfig = {
      id: hexId('rc'),
      name,
      outputs: cur ? [...cur.outputs] : d.p.output_configs.map((o) => o.id),
      production_model: s.default_production_model,
      quality: s.default_quality === 'draft' ? 'draft' : 'final',
      output_folder: '',
      eleven_credit_cap: s.eleven_default_credit_cap,
      dry_run_first: true,
      web_access: s.web_access_default,
      locales: [],
    } as unknown as RunConfig;
    d.update((p) => p.run_configs.push(rc));
    d.runConfig.value = rc.id;
  }

  function render() {
    const p = d.p;
    const rc = d.currentRunConfig();
    if (!rc) {
      clear(root, emptyState('No run config', 'A run config chooses the outputs, model and folder for a run.', button('Add Run Config', () => addRunConfig('Default'), { appearance: 'primary' })));
      return;
    }
    const upd = (f: (r: RunConfig) => void) => d.update((x) => f(x.run_configs.find((r) => r.id === rc.id)!));
    const switcher = p.run_configs.length <= 4
      ? h('div', { class: 'segmented', role: 'tablist', 'aria-label': 'Run config' }, p.run_configs.map((r) => h('button', { role: 'tab', 'aria-selected': String(r.id === rc.id), class: r.id === rc.id ? 'selected' : '', onclick: () => (d.runConfig.value = r.id) }, r.name)))
      : select(p.run_configs.map((r) => [r.id, r.name]), rc.id, (v) => (d.runConfig.value = v), { 'aria-label': 'Run config' });
    const more = h('div', { class: 'row' },
      iconButton(Icons.add, 'New run config', async () => {
        const name = await prompt('New run config', 'Name', 'New run config');
        if (name) addRunConfig(name);
      }),
      iconButton(Icons.copy, 'Duplicate run config', () => {
        const copy = { ...structuredClone(rc), id: hexId('rc'), name: `${rc.name} copy` };
        d.update((x) => x.run_configs.push(copy));
        d.runConfig.value = copy.id;
      }),
      iconButton(Icons.edit, 'Rename run config', async () => {
        const name = await prompt('Rename run config', 'Name', rc.name);
        if (name) upd((r) => (r.name = name));
      }),
      iconButton(Icons.delete, 'Delete run config', async () => {
        const c = await confirm(`Delete “${rc.name}”?`, 'Past runs keep their folders and history.', ['Delete', 'Cancel'], true);
        if (c !== 0) return;
        d.update((x) => (x.run_configs = x.run_configs.filter((r) => r.id !== rc.id)));
        d.runConfig.value = d.p.run_configs[0]?.id ?? '';
      }),
    );

    const outputs = h('ul', { class: 'output-list', 'aria-label': 'Outputs in this run' }, p.output_configs.map((o) => outputRow(o, rc)));
    const presets = platforms.value?.presets ?? [];
    const byPlatform = new Map<string, OutputPreset[]>();
    for (const pr of presets) byPlatform.set(pr.platform, [...(byPlatform.get(pr.platform) ?? []), pr]);
    const addSelect = h('select', { 'aria-label': 'Add output' },
      h('option', { value: '' }, 'Add output…'),
      [...byPlatform.entries()].map(([plat, list]) => h('optgroup', { label: plat }, list.map((pr) => h('option', { value: pr.id }, pr.name)))),
      h('option', { value: '__presets__' }, 'Edit Output Presets…'),
    ) as HTMLSelectElement;
    addSelect.addEventListener('change', () => {
      const v = addSelect.value;
      addSelect.value = '';
      if (v === '__presets__') return void request('shell.open_window', { kind: 'library', options: { tab: 'presets' } });
      if (!v) return;
      let id = `out_${v}`;
      let n = 2;
      while (d.p.output_configs.some((o) => o.id === id)) id = `out_${v}_${n++}`;
      d.update((x) => {
        x.output_configs.push({ id, preset: v, overrides: {} } as OutputConfig);
        x.run_configs.find((r) => r.id === rc.id)!.outputs.push(id);
      });
    });

    const recent = settings.value?.recent_output_folders ?? [];
    clear(
      root,
      h('section', null, h('div', { class: 'row' }, h('div', { class: 'grow' }, switcher), more)),
      h('section', null, h('h3', null, 'Outputs in this run'), outputs, addSelect),
      h('section', null,
        h('h3', null, 'Production'),
        field('Model', modelPicker(rc.production_model, (v) => upd((r) => (r.production_model = v)), { label: 'Production model' })),
        (() => {
          const e = effortPicker(rc.effort ?? null, (v) => upd((r) => (r.effort = v)), 'Production effort');
          return e ? field('Effort', e) : null;
        })(),
        field('Quality', select([['final', 'Final'], ['draft', 'Draft']], rc.quality, (v) => upd((r) => (r.quality = v as any)))),
        checkbox('Dry run first', rc.dry_run_first, (v) => upd((r) => (r.dry_run_first = v))),
        checkbox('Allow web access', !!rc.web_access, (v) => upd((r) => (r.web_access = v))),
        field('Languages', textInput((rc.locales ?? []).join(', '), (v) => upd((r) => (r.locales = v.split(',').map((s) => s.trim()).filter(Boolean)))), 'Empty: the project’s languages'),
      ),
      h('section', null,
        h('h3', null, 'Output folder'),
        h('div', { class: 'path small', title: outputRoot }, outputRoot || '—'),
        h('div', { class: 'row wrap' },
          button('Choose…', async () => {
            const [dir] = await openFolders('Output folder for this run config', false, outputRoot || undefined);
            if (!dir) return;
            upd((r) => (r.output_folder = dir));
            await saveSettings((s) => {
              s.recent_output_folders = [dir, ...s.recent_output_folders.filter((x) => x !== dir)].slice(0, 8);
            });
          }),
          select([['', `Default (${settings.value?.default_output_folder || 'Videos'})`], ...recent.map((r) => [r, r] as [string, string])], rc.output_folder ?? '', (v) => upd((r) => (r.output_folder = v)), { 'aria-label': 'Recent output folders' }),
          outputRoot ? button('Reveal', () => request('shell.reveal', { path: outputRoot })) : null,
        ),
      ),
      h('section', null,
        h('h3', null, 'Limits'),
        field('ElevenLabs credit cap', numberInput(rc.eleven_credit_cap ?? null, (v) => upd((r) => (r.eleven_credit_cap = v ?? 0)), { min: 0 })),
        field('Assistant spend limit (USD)', numberInput(rc.max_spend_usd ?? null, (v) => upd((r) => (r.max_spend_usd = v || null)), { min: 0, step: '0.5' })),
        field('Turn limit', numberInput(rc.max_turns ?? null, (v) => upd((r) => (r.max_turns = v || null)), { min: 0 })),
        field('Question timeout (seconds)', numberInput(rc.question_timeout_s ?? null, (v) => upd((r) => (r.question_timeout_s = v || null)), { min: 0 }), 'With no answer by then, the Assistant proceeds on its stated assumption.'),
      ),
    );
  }

  function outputRow(o: OutputConfig, rc: RunConfig): HTMLElement {
    const resolved = d.outputs.value.find((r) => r.id === o.id);
    const on = rc.outputs.includes(o.id);
    const desc = resolved?.error
      ? resolved.error
      : resolved
        ? `${resolved.width}×${resolved.height}${resolved.limits.max_duration_s ? ` · ≤ ${resolved.limits.max_duration_s} s` : ''} · ${resolved.platform}`
        : '';
    const input = h('input', { type: 'checkbox', checked: on, 'aria-label': resolved?.name ?? o.id }) as HTMLInputElement;
    input.addEventListener('change', () =>
      d.update((x) => {
        const r = x.run_configs.find((y) => y.id === rc.id)!;
        r.outputs = input.checked ? [...r.outputs, o.id] : r.outputs.filter((id) => id !== o.id);
      }),
    );
    return h('li', { class: 'row' }, h('label', { class: 'check grow' }, input, h('span', null, h('strong', null, resolved?.name ?? o.name ?? o.id), h('span', { class: `small ${resolved?.error ? 'danger' : 'muted'}` }, ` ${desc}`))), iconButton(Icons.edit, `Edit ${resolved?.name ?? o.id}`, () => outputEditor(w, o.id)));
  }

  return root;
}

/** The output editor sheet: preset, shape, size, frame rate, captions, audio, loop, limits, formats. */
export async function outputEditor(w: ProjectWindow, id: string) {
  const d = w.doc;
  const s = sheet('Output', { wide: true });
  const render = () => {
    const o = d.p.output_configs.find((x) => x.id === id);
    if (!o) return s.close();
    const preset = platforms.value?.presets.find((p) => p.id === o.preset);
    const ov = (o.overrides ??= {} as any);
    const set = (f: (x: any) => void) => {
      d.update((p) => f(p.output_configs.find((x) => x.id === id)!.overrides));
      render();
    };
    const resolved = d.outputs.value.find((r) => r.id === id);
    clear(
      s.body,
      field('Name', textInput(o.name ?? '', (v) => d.update((p) => (p.output_configs.find((x) => x.id === id)!.name = v || null)), { placeholder: preset?.name })),
      field('Preset', select((platforms.value?.presets ?? []).map((p) => [p.id, `${p.platform} — ${p.name}`]), o.preset, (v) => {
        d.update((p) => {
          const x = p.output_configs.find((y) => y.id === id)!;
          x.preset = v;
          x.overrides = {} as any;
        });
        render();
      })),
      preset && preset.targets.length > 1 ? field('Shape', select(preset.targets.map((t) => [t, t]), ov.target ?? preset.targets[0], (v) => set((x) => (x.target = v === preset.targets[0] ? null : v)))) : null,
      (ov.target ?? preset?.targets[0]) === 'custom'
        ? h('div', { class: 'row' },
            preset?.sizes.length ? field('Device size', select([['', 'Choose…'], ...preset.sizes.map((z) => [`${z.width}x${z.height}`, `${z.name} (${z.width}×${z.height})`] as [string, string])], '', (v) => {
              const [wd, ht] = v.split('x').map(Number);
              if (wd) set((x) => ((x.width = wd), (x.height = ht)));
            })) : null,
            field('Width', numberInput(ov.width ?? preset?.width ?? null, (v) => set((x) => (x.width = v)))),
            field('Height', numberInput(ov.height ?? preset?.height ?? null, (v) => set((x) => (x.height = v)))))
        : null,
      field('Frame rate', numberInput(ov.fps ?? preset?.fps ?? null, (v) => set((x) => (x.fps = v === preset?.fps ? null : v)))),
      field('Captions', select([['burn', 'Burned in'], ['sidecar', 'Sidecar file'], ['none', 'None']], ov.captions ?? preset?.captions ?? 'burn', (v) => set((x) => (x.captions = v === preset?.captions ? null : v)))),
      checkbox('Audio', ov.audio ?? preset?.audio ?? true, (v) => set((x) => (x.audio = v === preset?.audio ? null : v))),
      checkbox('Seamless loop', ov.loop ?? preset?.loop ?? false, (v) => set((x) => (x.loop = v === preset?.loop ? null : v))),
      field('Max length (s)', numberInput(ov.max_duration_s ?? preset?.limits.max_duration_s ?? null, (v) => set((x) => (x.max_duration_s = v === preset?.limits.max_duration_s ? null : v)))),
      field('Size cap (MB)', numberInput(ov.max_bytes ? ov.max_bytes / 1e6 : preset?.max_bytes ? preset.max_bytes / 1e6 : null, (v) => set((x) => (x.max_bytes = v ? Math.round(v * 1e6) : null)))),
      h('fieldset', null, h('legend', null, 'Formats'), ['mp4', 'webm'].map((f) => {
        const list: string[] = ov.formats ?? preset?.formats ?? ['mp4'];
        const box = checkbox(f, list.includes(f), (v) => {
          const next = v ? [...new Set([...list, f])] : list.filter((x) => x !== f);
          if (!next.length) return render();
          set((x) => (x.formats = JSON.stringify(next) === JSON.stringify(preset?.formats) ? null : next));
        });
        return box;
      })),
      resolved && !resolved.error ? h('p', { class: 'hint' }, `Renders ${resolved.width}×${resolved.height}${resolved.fps ? ` at ${resolved.fps} fps` : ''}.`) : null,
    );
  };
  render();
  const del = button('Delete Output', async () => {
    const c = await confirm('Delete this output?', 'It is removed from every run config.', ['Delete', 'Cancel'], true);
    if (c !== 0) return;
    d.update((p) => {
      p.output_configs = p.output_configs.filter((x) => x.id !== id);
      for (const r of p.run_configs) r.outputs = r.outputs.filter((x) => x !== id);
    });
    s.close();
  }, { appearance: 'subtle' });
  setDisabled(del, false);
  s.footer.append(del, button('Done', () => s.close(), { appearance: 'primary' }));
  await s.done;
}
