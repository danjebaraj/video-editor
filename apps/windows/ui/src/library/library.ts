// The App Library window (spec §8, §9.1; Ctrl+Shift+L): brand presets (create, edit, import,
// export, duplicate, delete) and output presets (built in and yours).

import { h, clear, button, iconButton, Icons, field, textInput, numberInput, select, checkbox, debounce, emptyState, fileName } from '../dom';
import { core, request, on, errorText, shellInfo } from '../bridge';
import { Signal, effect } from '../store';
import { brands, platforms, reloadLibrary } from '../app-state';
import { prompt, confirm, openFiles, saveFile, Filters, toast } from '../dialogs';
import type { BrandPreset, AssetRef, OutputPreset } from '../types';

export async function mount(root: HTMLElement) {
  const info = await shellInfo();
  const opts = (info.options ?? {}) as { tab?: string; brand?: string };
  const tab = new Signal<'brands' | 'presets'>(opts.tab === 'presets' ? 'presets' : 'brands');
  const selBrand = new Signal<string | null>(opts.brand ?? brands.value[0]?.id ?? null);
  const selPreset = new Signal<string | null>(null);
  const tabs = h('div', { class: 'segmented', role: 'tablist', 'aria-label': 'Library' });
  const body = h('div', { class: 'library-body' });
  clear(root, h('div', { class: 'library' }, h('header', { class: 'toolbar' }, tabs), body));

  on('open_files', async (d: { paths: string[] }) => {
    for (const p of d.paths) await importBrand(p);
  });
  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.key === 'w') {
      e.preventDefault();
      request('shell.close_window');
    }
  });

  async function importBrand(file: string) {
    try {
      const b = await core<BrandPreset>('brand.import', { file, replace: false });
      await reloadLibrary();
      selBrand.value = b.id;
      tab.value = 'brands';
      toast(`Imported “${b.name}”.`);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  effect([tab, brands, platforms, selBrand, selPreset], () => {
    clear(tabs, (['brands', 'presets'] as const).map((t) => h('button', { role: 'tab', 'aria-selected': String(tab.value === t), class: tab.value === t ? 'selected' : '', onclick: () => (tab.value = t) }, t === 'brands' ? 'Brand Presets' : 'Output Presets')));
    if (tab.value === 'brands') renderBrands();
    else renderPresets();
  });

  function renderBrands() {
    const list = h('ul', { class: 'master', role: 'listbox', 'aria-label': 'Brand presets' }, brands.value.map((b) =>
      h('li', { role: 'option', 'aria-selected': String(selBrand.value === b.id), class: selBrand.value === b.id ? 'selected' : '', tabindex: '0', onclick: () => (selBrand.value = b.id), onkeydown: (e: KeyboardEvent) => e.key === 'Enter' && (selBrand.value = b.id) },
        h('span', { class: 'swatches', 'aria-hidden': 'true' }, [b.colors.primary, b.colors.secondary, b.colors.accent].map((c) => h('span', { class: 'swatch', style: { background: c } }))),
        h('div', null, h('strong', null, b.name), h('div', { class: 'muted small' }, b.description)))));
    const footer = h('div', { class: 'sidebar-footer' },
      iconButton(Icons.add, 'New brand preset', async () => {
        const name = await prompt('New brand preset', 'Name', 'New brand');
        if (!name) return;
        let b = await core<BrandPreset>('brand.new', { name });
        b = await core<BrandPreset>('brand.save', { brand: b });
        await reloadLibrary();
        selBrand.value = b.id;
      }),
      button('Import…', async () => {
        const [f] = await openFiles('Import a brand preset', Filters.brand);
        if (f) importBrand(f);
      }, { appearance: 'subtle' }));
    const b = brands.value.find((x) => x.id === selBrand.value);
    clear(body, h('div', { class: 'master-detail' }, h('div', { class: 'master-col' }, brands.value.length ? list : emptyState('No brand presets', 'A preset holds logos, colors, fonts, voice and rules for the Assistant.'), footer), h('div', { class: 'detail' }, b ? brandEditor(structuredClone(b)) : emptyState('Select a brand preset'))));
  }

  function brandEditor(b: BrandPreset): HTMLElement {
    const save = debounce(async () => {
      try {
        const saved = await core<BrandPreset>('brand.save', { brand: b });
        b.modified = saved.modified;
        brands.value = brands.value.map((x) => (x.id === saved.id ? saved : x));
      } catch (e) {
        toast(errorText(e), 'error');
      }
    }, 600);
    const set = (f: () => void) => {
      f();
      save();
    };
    const asset = (label: string, get: () => AssetRef | null | undefined, put: (a: AssetRef | null) => void, filters = Filters.images) => {
      const a = get();
      return field(label, h('div', { class: 'row' },
        h('span', { class: 'path small grow', title: a?.path ?? '' }, a?.path ? fileName(a.path) : 'None'),
        button('Choose…', async () => {
          const [f] = await openFiles(label, filters);
          if (!f) return;
          const ref = await core<AssetRef>('brand.asset', { path: f });
          set(() => put(ref));
          renderBrands();
        }),
        a ? iconButton(Icons.remove, `Remove ${label}`, () => (set(() => put(null)), renderBrands())) : null));
    };
    const color = (key: keyof BrandPreset['colors'], label: string) => {
      const picker = h('input', { type: 'color', value: b.colors[key] || '#000000', 'aria-label': `${label} color` }) as HTMLInputElement;
      const hex = textInput(b.colors[key], (v) => {
        if (/^#[0-9a-fA-F]{6}$/.test(v)) {
          set(() => (b.colors[key] = v));
          picker.value = v;
        }
      }, { 'aria-label': `${label} hex`, maxlength: 7, size: 8 });
      picker.addEventListener('input', () => {
        hex.value = picker.value;
        set(() => (b.colors[key] = picker.value));
      });
      return h('div', { class: 'row color-row' }, h('span', { class: 'grow' }, label), picker, hex);
    };
    return h('div', { class: 'form-tab' },
      h('section', null, h('h3', null, 'Preset'), field('Name', textInput(b.name, (v) => set(() => (b.name = v)))), field('Description', textInput(b.description, (v) => set(() => (b.description = v))))),
      h('section', null, h('h3', null, 'Logos'),
        asset('Primary', () => b.logos.primary, (a) => (b.logos.primary = a)),
        asset('Horizontal', () => b.logos.horizontal, (a) => (b.logos.horizontal = a)),
        asset('Icon', () => b.logos.icon, (a) => (b.logos.icon = a)),
        asset('For light backgrounds', () => b.logos.light, (a) => (b.logos.light = a)),
        asset('For dark backgrounds', () => b.logos.dark, (a) => (b.logos.dark = a))),
      h('section', null, h('h3', null, 'Colors'), color('primary', 'Primary'), color('secondary', 'Secondary'), color('accent', 'Accent'), color('background', 'Background'), color('text', 'Text')),
      h('section', null, h('h3', null, 'Fonts'),
        field('Heading family', textInput(b.fonts.heading.family ?? '', (v) => set(() => (b.fonts.heading.family = v || null)))),
        asset('Heading font file', () => b.fonts.heading.file, (a) => (b.fonts.heading.file = a), Filters.fonts),
        field('Body family', textInput(b.fonts.body.family ?? '', (v) => set(() => (b.fonts.body.family = v || null)))),
        asset('Body font file', () => b.fonts.body.file, (a) => (b.fonts.body.file = a), Filters.fonts)),
      h('section', null, h('h3', null, 'Bumpers and watermark'),
        asset('Intro', () => b.intro, (a) => (b.intro = a), Filters.movieOrImage),
        asset('Outro', () => b.outro, (a) => (b.outro = a), Filters.movieOrImage),
        asset('Watermark', () => b.watermark.asset, (a) => (b.watermark.asset = a)),
        field('Watermark position', select([['top_left', 'Top left'], ['top_right', 'Top right'], ['bottom_left', 'Bottom left'], ['bottom_right', 'Bottom right']], b.watermark.position, (v) => set(() => (b.watermark.position = v)))),
        field('Watermark opacity (%)', numberInput(Math.round((b.watermark.opacity ?? 1) * 100), (v) => set(() => (b.watermark.opacity = Math.max(10, Math.min(100, v ?? 100)) / 100)), { min: 10, max: 100 }))),
      h('section', null, h('h3', null, 'Style'),
        field('Lower third', textInput(b.lower_third_style, (v) => set(() => (b.lower_third_style = v)))),
        field('Captions', textInput(b.caption_style, (v) => set(() => (b.caption_style = v)))),
        field('Music mood', textInput(b.music_mood, (v) => set(() => (b.music_mood = v))))),
      h('section', null, h('h3', null, 'Voice'),
        field('ElevenLabs voice ID', textInput(b.voice.voice_id, (v) => set(() => (b.voice.voice_id = v)))),
        field('Name', textInput(b.voice.name ?? '', (v) => set(() => (b.voice.name = v || null)))),
        field('Model', textInput(b.voice.model ?? '', (v) => set(() => (b.voice.model = v || null)))),
        field('Settings (JSON)', textInput(b.voice.settings ? JSON.stringify(b.voice.settings) : '', (v) => {
          try {
            const o = v.trim() ? JSON.parse(v) : null;
            if (o === null || (typeof o === 'object' && !Array.isArray(o))) set(() => (b.voice.settings = o));
          } catch {
            toast('The voice settings must be a JSON object.', 'error');
          }
        }))),
      h('section', null, h('h3', null, 'Rules for the Assistant'), (() => {
        const t = h('textarea', { rows: 6, 'aria-label': 'Rules for the Assistant' }, b.rules) as HTMLTextAreaElement;
        t.addEventListener('input', () => set(() => (b.rules = t.value)));
        return t;
      })()),
      h('div', { class: 'row wrap' },
        button('Export…', async () => {
          save.flush();
          const dest = await saveFile('Export brand preset', Filters.brand, `${b.name}.cutbrand`);
          if (!dest) return;
          try {
            await core('brand.export', { brand: b, destination: dest });
            toast('Exported.');
          } catch (e) {
            toast(errorText(e), 'error');
          }
        }),
        button('Duplicate', async () => {
          const copy = await core<BrandPreset>('brand.save', { brand: { ...structuredClone(b), id: '', name: `${b.name} copy` } });
          await reloadLibrary();
          selBrand.value = copy.id;
        }),
        button('Delete Preset', async () => {
          const c = await confirm(`Delete “${b.name}”?`, 'Projects keep their snapshot of it.', ['Delete', 'Cancel'], true);
          if (c !== 0) return;
          await core('brand.delete', { id: b.id });
          await reloadLibrary();
          selBrand.value = brands.value[0]?.id ?? null;
        }, { appearance: 'subtle' })),
    );
  }

  function renderPresets() {
    const all = platforms.value?.presets ?? [];
    const item = (p: OutputPreset) => h('li', { role: 'option', 'aria-selected': String(selPreset.value === p.id), class: selPreset.value === p.id ? 'selected' : '', tabindex: '0', onclick: () => (selPreset.value = p.id) }, h('strong', null, p.name), h('div', { class: 'muted small' }, p.platform));
    const list = h('div', { class: 'master', role: 'listbox', 'aria-label': 'Output presets' },
      h('div', { class: 'group-title' }, 'Built in'), h('ul', null, all.filter((p) => !p.user).map(item)),
      h('div', { class: 'group-title' }, 'Yours'), h('ul', null, all.filter((p) => p.user).map(item)));
    const p = all.find((x) => x.id === selPreset.value);
    clear(body, h('div', { class: 'master-detail' }, h('div', { class: 'master-col' }, list, h('div', { class: 'sidebar-footer muted small' }, `Platform limits updated ${platforms.value?.updated ?? ''}`)), h('div', { class: 'detail' }, p ? presetEditor(structuredClone(p)) : emptyState('Select an output preset'))));
  }

  function presetEditor(p: OutputPreset): HTMLElement {
    const ro = !p.user;
    const save = async () => {
      try {
        platforms.value = await core('preset.save', { preset: p });
      } catch (e) {
        toast(errorText(e), 'error');
      }
    };
    const dis = ro ? { disabled: true } : {};
    const shapes = ['shorts', 'long_1080p', 'long_4k', 'square', 'portrait_4x5', 'custom'];
    return h('div', { class: 'form-tab' },
      h('div', { class: 'row' },
        button('Duplicate…', async () => {
          const name = await prompt('Duplicate output preset', 'Name', `${p.name} copy`);
          if (!name) return;
          const copy = await core<OutputPreset>('preset.duplicate', { id: p.id, name });
          await reloadLibrary();
          selPreset.value = copy.id;
        }),
        p.user ? button('Delete', async () => {
          const c = await confirm(`Delete “${p.name}”?`, 'Projects that use it keep working until you change their outputs.', ['Delete', 'Cancel'], true);
          if (c !== 0) return;
          platforms.value = await core('preset.delete', { id: p.id });
          selPreset.value = null;
        }, { appearance: 'subtle' }) : h('span', { class: 'muted small' }, 'Built-in presets can’t be changed. Duplicate one to make your own.')),
      field('Name', textInput(p.name, (v) => ((p.name = v), save()), dis)),
      field('Platform', textInput(p.platform, (v) => ((p.platform = v), save()), dis)),
      h('fieldset', null, h('legend', null, 'Shapes (the first is the default)'), shapes.map((s) => checkbox(s, p.targets.includes(s), (v) => {
        const next = v ? [...p.targets, s] : p.targets.filter((x) => x !== s);
        if (!next.length) return renderPresets();
        p.targets = next;
        save();
      }, dis))),
      field('Captions', select([['burn', 'Burned in'], ['sidecar', 'Sidecar'], ['none', 'None']], p.captions, (v) => ((p.captions = v), save()), dis)),
      checkbox('Audio', p.audio, (v) => ((p.audio = v), save()), dis),
      checkbox('Loop', p.loop, (v) => ((p.loop = v), save()), dis),
      field('Minimum length (s)', numberInput(p.limits.min_duration_s ?? null, (v) => ((p.limits.min_duration_s = v ?? undefined), save()), dis)),
      field('Maximum length (s)', numberInput(p.limits.max_duration_s ?? null, (v) => ((p.limits.max_duration_s = v ?? undefined), save()), dis)),
      field('Platform size limit (MB)', numberInput(p.limits.max_bytes ? p.limits.max_bytes / 1e6 : null, (v) => ((p.limits.max_bytes = v ? Math.round(v * 1e6) : undefined), save()), dis)),
      field('Maximum frame rate', numberInput(p.limits.max_fps ?? null, (v) => ((p.limits.max_fps = v ?? undefined), save()), dis)),
      field('Notes', textInput(p.notes, (v) => ((p.notes = v), save()), dis)),
    );
  }
}
