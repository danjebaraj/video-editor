// The start window: onboarding checklist, recent projects, New Project and Open (spec §17).

import { h, clear, button, icon, Icons, field, select, textInput, fileName, parentDir, spinner } from '../dom';
import { core, request, errorText } from '../bridge';
import { environment, keyStatus, settings, platforms, brands, detect, saveSettings, reloadLibrary, libraryDir } from '../app-state';
import { effect } from '../store';
import { sheet, saveFile, openFiles, openFolders, Filters, toast } from '../dialogs';
import { INTENTS } from '../types';

export function mount(root: HTMLElement) {
  const checklist = h('ul', { class: 'checklist', 'aria-label': 'Setup checklist' });
  const recentList = h('ul', { class: 'recent', 'aria-label': 'Recent projects' });
  clear(
    root,
    h(
      'main',
      { class: 'start' },
      h(
        'section',
        { class: 'start-left' },
        h('div', { class: 'app-title' }, h('img', { src: './icon.png', alt: '', width: 48, height: 48 }), h('div', null, h('h1', null, 'Syncfusion Video Studio'), h('p', { class: 'muted' }, 'Describe a video in Markdown, refine it with Claude, and let Claude Code render it with cutline.'))),
        h('h2', null, 'Get set up'),
        checklist,
        h('div', { class: 'row' }, button('New Project…', () => newProject(), { appearance: 'primary', glyph: Icons.add }), button('Open…', () => openProject(), { glyph: Icons.openFolder }), button('App Library', () => request('shell.open_window', { kind: 'library' }), { glyph: Icons.library })),
      ),
      h('section', { class: 'start-right' }, h('h2', null, 'Recent projects'), recentList),
    ),
  );

  effect([environment, keyStatus, settings, brands], () => renderChecklist(checklist));
  request<string[]>('shell.recent').then((list) => {
    if (!list.length) {
      clear(recentList, h('li', { class: 'muted' }, 'No recent projects'));
      return;
    }
    clear(
      recentList,
      list.map((p) =>
        h('li', null, h('button', { class: 'link-row', onclick: () => openPath(p), title: p }, icon(Icons.document), h('span', null, h('strong', null, fileName(p).replace(/\.cutproj$/i, '')), h('span', { class: 'muted small' }, parentDir(p))))),
      ),
    );
  });

  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === 'n') {
      e.preventDefault();
      newProject();
    } else if (e.ctrlKey && e.key.toLowerCase() === 'o') {
      e.preventDefault();
      openProject();
    } else if (e.ctrlKey && e.key === ',') {
      e.preventDefault();
      request('shell.open_window', { kind: 'settings' });
    } else if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === 'l') {
      e.preventDefault();
      request('shell.open_window', { kind: 'library' });
    }
  });
}

function row(ok: boolean | null, title: string, detail: string, action?: HTMLElement, optional = false): HTMLElement {
  const state = ok === null ? spinner('Checking') : icon(ok ? Icons.checkCircle : optional ? Icons.info : Icons.warning, ok ? 'ok' : optional ? 'muted' : 'warn');
  return h('li', { class: 'check-row' }, h('span', { class: 'check-icon' }, state), h('div', { class: 'grow' }, h('strong', null, title, optional ? h('span', { class: 'muted small' }, ' (optional)') : null), h('div', { class: 'muted small' }, detail)), action ?? null);
}

function renderChecklist(list: HTMLElement) {
  const env = environment.value;
  const s = settings.value;
  const rows: HTMLElement[] = [];
  const openSettings = (pane: string) => button('Settings…', () => request('shell.open_window', { kind: 'settings', options: { pane } }));
  if (!env) {
    rows.push(row(null, 'Claude Code', 'Checking…'), row(null, 'cutline', 'Checking…'));
  } else {
    const c = env.claude;
    let detail = 'Required. Not found. Install it with `irm https://claude.ai/install.ps1 | iex` in PowerShell.';
    let ok = false;
    if (c && !c.supported) detail = `Version ${c.version} is too old; ${env.min_claude_version} or later is needed.`;
    else if (c && !c.git_bash) detail = 'Installed, but Git for Windows (Git Bash) wasn’t found. Claude Code needs it to run commands.';
    else if (c && env.signed_in === false) detail = 'Installed, but not signed in. Run `claude` in a terminal and sign in.';
    else if (c) {
      detail = c.version_text;
      ok = true;
    }
    rows.push(row(ok, 'Claude Code', detail, openSettings('claude')));
    const cl = env.cutline;
    const cok = !!cl && !!env.doctor?.ok;
    const cdetail = !cl ? 'Required. Not found.' : cok ? `cutline ${cl.version}` : `Missing: ${(env.doctor?.missing_required ?? []).join(', ') || 'run cutline doctor'}`;
    const cbtn = cl && !cok ? button('Install Dependencies', async (e) => {
      const b = e.currentTarget as HTMLElement;
      b.setAttribute('disabled', '');
      toast('Installing cutline’s dependencies… this can take a few minutes.');
      try {
        const r = await request<{ code: number; output: string }>('shell.cutline', { action: 'install_deps' });
        toast(r.code === 0 ? 'Dependencies installed.' : `Install finished with exit ${r.code}.`, r.code === 0 ? 'info' : 'error');
      } catch (err) {
        toast(errorText(err), 'error');
      }
      await detect(true);
    }) : openSettings('cutline');
    rows.push(row(cok, 'cutline', cdetail, cbtn));
  }
  const key = keyStatus.value;
  rows.push(row(key ? key.present : null, 'ElevenLabs key', key?.present ? `Stored in Credential Manager (${key.masked})` : 'Only needed for generated voice-over and music.', openSettings('elevenlabs'), true));
  const hasBrand = brands.value.length > 0;
  rows.push(
    row(hasBrand, 'Brand preset', hasBrand ? brands.value.map((b) => b.name).join(', ') : 'Logos, colors, fonts and voice for your videos.', button(hasBrand ? 'Edit…' : 'Create Default', async () => {
      if (!hasBrand) {
        await core('brand.ensure_default');
        await reloadLibrary();
      }
      request('shell.open_window', { kind: 'library' });
    }), true),
  );
  const folder = s?.default_output_folder || 'Videos\\Syncfusion Video Studio';
  rows.push(
    row(true, 'Output folder', folder, button('Change…', async () => {
      const [dir] = await openFolders('Default output folder');
      if (dir) await saveSettings((x) => (x.default_output_folder = dir));
    })),
  );
  clear(list, rows);
}

async function openPath(path: string) {
  await request('shell.open_window', { kind: 'project', path });
  request('shell.close_window');
}

async function openProject() {
  const files = await openFiles('Open project', Filters.project, true);
  for (const f of files) await request('shell.open_window', { kind: 'project', path: f });
  if (files.length) request('shell.close_window');
}

/** The New Project sheet: name, intent, template (spec §7, §5). */
export async function newProject() {
  const s = sheet('New project');
  let name = 'Untitled video';
  let intent = 'marketing';
  let other = '';
  const templates = await core<{ id: string; title: string }[]>('spec.templates').catch(() => []);
  const defaults = () => platforms.value?.intents[intent];
  let template = defaults()?.template ?? 'blank';
  const templateSelect = select(templates.map((t) => [t.id, t.title]), template, (v) => (template = v), { 'aria-label': 'Spec template' });
  const suggested = h('div', { class: 'hint' });
  const otherField = field('Describe it', textInput('', (v) => (other = v)));
  const refresh = () => {
    const d = defaults();
    template = d?.template ?? template;
    templateSelect.value = template;
    const names = (d?.outputs ?? []).map((id) => platforms.value?.presets.find((p) => p.id === id)?.name ?? id);
    suggested.textContent = names.length ? `Suggested outputs: ${names.join(', ')}` : '';
    otherField.hidden = intent !== 'other';
  };
  s.body.append(
    field('Name', textInput(name, (v) => (name = v), { autofocus: true })),
    field('Intent', select(INTENTS, intent, (v) => {
      intent = v;
      refresh();
    })),
    otherField,
    field('Spec template', templateSelect),
    suggested,
    h('p', { class: 'hint' }, button('From a template file…', async () => {
      const [file] = await openFiles('Project template', Filters.project, false, `${libraryDir.value}\\templates`);
      if (file) s.close({ fromTemplate: file });
    }, { appearance: 'subtle' })),
  );
  refresh();
  s.footer.append(button('Cancel', () => s.close()), button('Create…', () => s.close({ create: true }), { appearance: 'primary' }));
  const r = await s.done;
  if (!r) return;
  const path = await saveFile('Save the new project', Filters.project, `${name || 'Untitled video'}.cutproj`);
  if (!path) return;
  try {
    const project = r.fromTemplate
      ? await core('project.from_template', { path: r.fromTemplate, name })
      : await core('project.new', { name, intent, intent_other: intent === 'other' ? other : null, template });
    await core('project.save', { project, path });
    await request('shell.add_recent', { path });
    await request('shell.open_window', { kind: 'project', path });
    const info = await request<{ window_kind: string }>('shell.info');
    if (info.window_kind === 'start') request('shell.close_window');
  } catch (e) {
    toast(`The project couldn’t be created: ${errorText(e)}`, 'error');
  }
}
