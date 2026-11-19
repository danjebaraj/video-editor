// The sidebar (spec §17): Outline, Media (grouped by kind, status badges, thumbnails, detail),
// Runs (history with status chips).

import { h, clear, icon, Icons, button, iconButton, field, select, textInput, duration, when, usd, fileName, emptyState } from '../dom';
import { request } from '../bridge';
import { effect, Signal } from '../store';
import { openFiles, openFolders, Filters, confirm, toast } from '../dialogs';
import { MEDIA_KINDS, MEDIA_ROLES, STATUS_LABEL } from '../types';
import { exportLogs, copyIssueReport } from './inspector/run';
import { copyText } from '../text-tools';
import type { ProjectWindow } from './window';
import type { MediaItem, RunRecord } from '../types';

export type SidebarTab = 'outline' | 'media' | 'runs';

const STATUS_ICON: Record<string, string> = {
  running: Icons.spinner,
  succeeded: Icons.checkCircle,
  succeeded_with_warnings: Icons.warning,
  failed: Icons.error,
  cancelled: Icons.dismiss,
};

export function sidebar(w: ProjectWindow): { el: HTMLElement; tab: Signal<SidebarTab>; selectMedia(id: string): void } {
  const tab = new Signal<SidebarTab>('outline');
  const selectedMedia = new Signal<string | null>(null);
  const tabs: [SidebarTab, string, string][] = [
    ['outline', 'Outline', Icons.outline],
    ['media', 'Media', Icons.video],
    ['runs', 'Runs', Icons.history],
  ];
  const tablist = h('div', { class: 'segmented', role: 'tablist', 'aria-label': 'Sidebar' });
  const panel = h('div', { class: 'sidebar-panel', role: 'tabpanel' });
  const el = h('aside', { class: 'sidebar', 'aria-label': 'Sidebar' }, tablist, panel);

  effect([tab], () => {
    clear(
      tablist,
      tabs.map(([id, label, glyph]) =>
        h('button', { role: 'tab', 'aria-selected': String(tab.value === id), class: tab.value === id ? 'selected' : '', onclick: () => (tab.value = id), id: `sidebar-${id}` }, icon(glyph), label),
      ),
    );
    panel.setAttribute('aria-labelledby', `sidebar-${tab.value}`);
  });

  const d = w.doc;
  effect([tab, d.outline, d.lint, d.project, d.resolutions, selectedMedia, w.runs.selectedRun, w.runs.live], () => {
    if (tab.value === 'outline') renderOutline();
    else if (tab.value === 'media') renderMedia();
    else renderRuns();
  });

  function renderOutline() {
    const items = d.outline.value;
    if (!items.length) {
      clear(panel, emptyState('No headings yet', 'Headings and scenes in the spec appear here.'));
      return;
    }
    const warnLines = new Set(d.lint.value.filter((l) => l.severity === 'warning').map((l) => l.line));
    clear(
      panel,
      h(
        'ul',
        { class: 'outline', 'aria-label': 'Outline' },
        items.map((o) =>
          h(
            'li',
            null,
            h(
              'button',
              { class: `link-row level-${o.level} ${o.kind}`, style: { paddingLeft: `${8 + (o.level - 1) * 12}px` }, onclick: () => w.jumpToLine(o.line) },
              o.kind === 'scene' ? icon(Icons.video, 'muted') : null,
              h('span', { class: 'grow' }, o.title),
              warnLines.has(o.line) ? h('span', { class: 'warn', title: 'Spec note on this line' }, icon(Icons.warning)) : null,
            ),
          ),
        ),
      ),
    );
  }

  function glyphFor(m: MediaItem): HTMLElement {
    const g = m.kind === 'audio' ? Icons.audio : m.kind === 'font' ? Icons.font : m.kind === 'image' ? Icons.image : m.kind === 'video' ? Icons.video : Icons.document;
    return h('span', { class: 'thumb glyph' }, icon(g));
  }

  function thumbFor(m: MediaItem): HTMLElement {
    const url = m.kind === 'video' || m.kind === 'image' ? w.preview?.thumb(m.id, 0) : null;
    if (!url || d.resolution(m.id)?.status === 'missing') return glyphFor(m);
    const img = h('img', { class: 'thumb', src: url, alt: '', loading: 'lazy' }) as HTMLImageElement;
    // HEIC photos are served as JPEG copies; when no copy can be made the host says why
    if (/\.hei[cf]$/i.test(m.paths.absolute ?? m.paths.relative ?? '')) {
      img.onerror = async () => {
        const glyph = glyphFor(m);
        let why = 'no preview';
        try {
          why = (await (await fetch(url)).text()).trim() || why;
        } catch {
          /* keep the generic reason */
        }
        glyph.title = `Can't preview this HEIC photo: ${why}`;
        img.replaceWith(glyph);
      };
    }
    return img;
  }

  function renderMedia() {
    const media = d.p.media;
    const missing = d.missing();
    const list = h('div', { class: 'media-list', role: 'listbox', 'aria-label': 'Media', tabindex: '0' });
    for (const [kind, label] of MEDIA_KINDS) {
      const items = media.filter((m) => m.kind === kind);
      if (!items.length) continue;
      list.append(h('div', { class: 'group-title', role: 'presentation' }, label));
      for (const m of items) {
        const r = d.resolution(m.id);
        const status = r?.status ?? 'ok';
        const row = h(
          'div',
          {
            class: `media-row${selectedMedia.value === m.id ? ' selected' : ''}`,
            role: 'option',
            'aria-selected': String(selectedMedia.value === m.id),
            tabindex: '-1',
            draggable: 'true',
            'aria-label': `${fileName(m.paths.absolute ?? m.paths.relative ?? m.id)}, ${m.id}${status === 'missing' ? ', missing' : status === 'changed' ? ', changed' : ''}`,
            onclick: () => (selectedMedia.value = m.id),
            ondblclick: () => w.insertMediaRef(m.id),
            oncontextmenu: (e: MouseEvent) => {
              e.preventDefault();
              selectedMedia.value = m.id;
            },
            ondragstart: (e: DragEvent) => e.dataTransfer?.setData('text/plain', `![${m.id}](media:${m.id})`),
          },
          thumbFor(m),
          h('div', { class: 'grow' }, h('div', { class: 'name' }, fileName(m.paths.absolute ?? m.paths.relative ?? m.id)), h('div', { class: 'muted small' }, m.id, m.probe?.duration_s ? ` · ${duration(m.probe.duration_s)}` : '')),
          status === 'missing' ? h('span', { class: 'badge danger' }, 'Missing') : status === 'changed' ? h('span', { class: 'badge warn' }, 'Changed') : null,
        );
        list.append(row);
      }
    }
    list.addEventListener('dragover', (e) => e.preventDefault());
    list.addEventListener('drop', async (e) => {
      e.preventDefault();
      const paths = [...(e.dataTransfer?.files ?? [])].map((f) => (f as any).path as string).filter(Boolean);
      if (paths.length) await d.addMedia(paths);
    });
    list.addEventListener('keydown', (e) => {
      const ids = media.map((m) => m.id);
      const i = ids.indexOf(selectedMedia.value ?? '');
      if (e.key === 'ArrowDown') selectedMedia.value = ids[Math.min(ids.length - 1, i + 1)];
      else if (e.key === 'ArrowUp') selectedMedia.value = ids[Math.max(0, i - 1)];
      else if (e.key === 'Enter' && selectedMedia.value) w.insertMediaRef(selectedMedia.value);
      else if (e.key === 'Delete' && selectedMedia.value) d.removeMedia(selectedMedia.value);
      else return;
      e.preventDefault();
    });
    const sel = media.find((m) => m.id === selectedMedia.value);
    clear(
      panel,
      missing.length ? button(`${missing.length} missing — Relink…`, () => w.showRelink(), { appearance: 'primary', class: 'danger-button', glyph: Icons.missing }) : null,
      media.length ? list : emptyState('No media yet', 'Drop files here or on the editor. Files are referenced where they are, never copied.'),
      h('div', { class: 'sidebar-footer' }, iconButton(Icons.add, 'Add media', () => w.addMediaDialog()), iconButton(Icons.folder, 'Add a media folder', () => w.addMediaFolderDialog()), h('span', { class: 'muted small grow end' }, `${media.length} item(s)`)),
      sel ? mediaDetail(sel) : null,
    );
  }

  function mediaDetail(m: MediaItem): HTMLElement {
    const r = d.resolution(m.id);
    const path = r?.path ?? r?.stored_path ?? m.paths.absolute ?? '';
    return h(
      'section',
      { class: 'media-detail', 'aria-label': `Details of ${m.id}` },
      h('h3', null, m.id),
      field('Role', select(MEDIA_ROLES, m.role ?? '', (v) => d.update((p) => (p.media.find((x) => x.id === m.id)!.role = (v || null) as any)))),
      field('Notes for Claude', textInput(m.notes ?? '', (v) => d.update((p) => (p.media.find((x) => x.id === m.id)!.notes = v || null)))),
      field('Tags', textInput((m.tags ?? []).join(', '), (v) => d.update((p) => (p.media.find((x) => x.id === m.id)!.tags = v.split(',').map((t) => t.trim()).filter(Boolean))))),
      h('div', { class: `path small${r?.status === 'missing' ? ' danger' : ''}`, title: path }, path),
      m.probe?.error ? h('div', { class: 'danger small' }, m.probe.error) : null,
      h(
        'div',
        { class: 'row wrap' },
        button('Insert in Spec', () => w.insertMediaRef(m.id)),
        button('Copy Reference', () => copyText(`![${m.id}](media:${m.id})`)),
        r?.status === 'changed' ? button('Accept Change', () => d.acceptChanged(m.id)) : null,
        button('Relink…', () => w.locate(m.id)),
        r?.path ? button('Reveal', () => request('shell.reveal', { path: r.path })) : null,
        r?.path ? button('Open', () => request('shell.open', { path: r.path })) : null,
        button('Remove', () => d.removeMedia(m.id), { appearance: 'subtle' }),
      ),
    );
  }

  function renderRuns() {
    const runs = [...d.p.runs].reverse();
    if (!runs.length) {
      clear(panel, emptyState('No runs yet', 'Validate the spec, then Run (F5).'));
      return;
    }
    clear(
      panel,
      h(
        'ul',
        { class: 'runs', 'aria-label': 'Run history' },
        runs.map((r) => runRow(r)),
      ),
    );
  }

  function runRow(r: RunRecord): HTMLElement {
    const rc = d.p.run_configs.find((c) => c.id === r.run_config);
    const sel = w.runs.selectedRun.value === r.id;
    return h(
      'li',
      null,
      h(
        'button',
        {
          class: `link-row run-row${sel ? ' selected' : ''}`,
          'aria-current': sel ? 'true' : undefined,
          onclick: () => {
            w.runs.selectedRun.value = r.id;
            w.showTab('run');
          },
          oncontextmenu: async (e: MouseEvent) => {
            e.preventDefault();
            const c = await confirm(rc?.name ?? 'Run', r.folder, ['Reveal in File Explorer', 'Locate…', 'Copy Folder Path', 'Export Logs…', 'Copy Issue Report', 'Cancel']);
            if (c === 0) request('shell.reveal', { path: r.folder });
            else if (c === 1) relocate(r);
            else if (c === 2) copyText(r.folder);
            else if (c === 3) exportLogs(r.folder);
            else if (c === 4) copyIssueReport(r.folder);
          },
        },
        h('span', { class: `status ${r.status}`, title: STATUS_LABEL[r.status] }, icon(STATUS_ICON[r.status] ?? Icons.info)),
        h(
          'span',
          { class: 'grow' },
          h('span', { class: 'name' }, rc?.name ?? r.run_config, r.kind === 'dry_run' ? h('span', { class: 'pill' }, 'Dry run') : null, r.previous_run ? icon(Icons.revise, 'muted') : null),
          h('span', { class: 'muted small' }, when(r.started), r.cost_usd != null ? ` · ${usd(r.cost_usd)}` : ''),
        ),
      ),
    );
  }

  async function relocate(r: RunRecord) {
    const [dir] = await openFolders('Locate the run folder');
    if (!dir) return;
    d.update((p) => (p.runs.find((x) => x.id === r.id)!.folder = dir));
    toast('Run folder updated.');
  }

  return {
    el,
    tab,
    selectMedia(id: string) {
      tab.value = 'media';
      selectedMedia.value = id;
    },
  };
}

export async function pickMedia(): Promise<string[]> {
  return openFiles('Add media', Filters.media, true);
}
