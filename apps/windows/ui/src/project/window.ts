// A project window (spec §17): toolbar, sidebar, editor / preview / review, inspector, status
// bar; the Windows shortcuts (spec §23.6); menu commands; file operations; closing.

import { h, clear, button, iconButton, icon, Icons, select, spinner, usd, elapsed, fileName, parentDir, joinPath } from '../dom';
import { core, request, on, errorText } from '../bridge';
import { Signal, effect } from '../store';
import { brands, claudeAvailable, environment, detect, libraryDir } from '../app-state';
import { confirm, saveFile, openFiles, openFolders, Filters, toast, sheet } from '../dialogs';
import { clipboardText, copyButton } from '../text-tools';
import { ProjectDocument } from './document';
import { RunController } from './runs';
import { Assistant } from './assistant';
import { SpecEditor } from './editor';
import { Preview } from './preview';
import { sidebar } from './sidebar';
import { reviewView } from './review';
import { claudeTab } from './inspector/claude';
import { projectTab } from './inspector/project';
import { outputsTab } from './inspector/outputs';
import { validationTab } from './inspector/validation';
import { runTab, exportLogs } from './inspector/run';
import { showRelink, locate, showRunConfirm, showFindings } from './sheets';
import { newProject } from '../start/start';
import type { ShellInfo } from '../generated/bridge';
import type { FixLink } from '../types';
import { STATUS_LABEL } from '../types';

export type InspectorTab = 'claude' | 'project' | 'outputs' | 'validation' | 'run';

export class ProjectWindow {
  doc = new ProjectDocument();
  runs = new RunController(this.doc);
  assistant = new Assistant(this.doc);
  editor!: SpecEditor;
  preview!: Preview;
  previewOn = new Signal(false);
  tab = new Signal<InspectorTab>('claude');
  selectionLines = new Signal<[number, number] | null>(null);
  cursor = new Signal({ line: 1, col: 1, words: null as number | null });
  private sidebarApi!: ReturnType<typeof sidebar>;

  showTab = (t: string) => (this.tab.value = t as InspectorTab);

  async open(info: ShellInfo, root: HTMLElement) {
    if (!info.path) {
      clear(root, h('div', { class: 'empty' }, 'No project file was given.'));
      return;
    }
    clear(root, h('div', { class: 'center fill' }, spinner('Opening the project')));
    try {
      await this.doc.load(info.path);
    } catch (e) {
      clear(root, h('div', { class: 'empty' }, h('div', { class: 'empty-title' }, 'The project couldn’t be opened'), h('p', null, errorText(e)), button('Close', () => request('shell.close_window'))));
      return;
    }
    this.runs.showTab = this.showTab;
    this.assistant.showTab = this.showTab;
    this.build(root);
    await this.assistant.load().catch(() => {});
    this.wire();
    if (info.selftest) (await import('../selftest')).attach(this);
  }

  private build(root: HTMLElement) {
    const d = this.doc;
    // Center: editor, preview, review
    const editorHost = h('div', { class: 'editor-host' });
    const previewHost = h('div', { class: 'preview-host' });
    const review = reviewView(this.assistant);
    const banner = h('div', { class: 'banner-host' });
    const center = h('main', { class: 'center-pane', 'aria-label': 'Spec' }, banner, editorHost, previewHost, review);
    this.editor = new SpecEditor(editorHost, d.p.spec, {
      onChange: (t) => {
        d.setSpec(t);
        if (this.previewOn.value) this.preview.render();
      },
      onCursor: (line, col, words) => {
        this.cursor.value = { line, col, words };
        const s = this.editor?.selection();
        this.selectionLines.value = s ? [s.from, s.to] : null;
      },
      addMedia: (files) => d.addMedia(files),
      addPastedImage: (b64) => d.addPastedImage(b64),
    });
    this.preview = new Preview(previewHost, d, {
      onTask: (line) => this.toggleTask(line),
      onMedia: (id) => this.openMedia(id),
      onLink: (href) => {
        if (href.startsWith('media:')) this.openMedia(href.slice(6));
        else if (/^https?:/i.test(href)) request('shell.open_url', { url: href });
      },
      onScroll: () => {},
    });
    d.onExternalSpec = (spec) => {
      this.editor.setText(spec);
      this.preview.render();
    };
    d.onMissingMedia = () => showRelink(this);
    this.assistant.getSpec = () => this.editor.text;
    this.assistant.applySpec = (t) => this.editor.setText(t, true);
    this.assistant.setReadOnly = (ro) => this.editor.setReadOnly(ro);

    this.sidebarApi = sidebar(this);
    const inspector = this.inspector();
    const toolbar = this.toolbar();
    const status = this.statusBar();
    const shell = h(
      'div',
      { class: 'project-shell' },
      toolbar,
      h('div', { class: 'panes' }, this.sidebarApi.el, splitter('--sidebar-w', 1), center, splitter('--inspector-w', -1), inspector),
      status,
    );
    clear(root, shell);

    effect([this.previewOn, this.assistant.review], () => {
      const reviewing = !!this.assistant.review.value;
      editorHost.hidden = reviewing || this.previewOn.value;
      previewHost.hidden = reviewing || !this.previewOn.value;
      review.hidden = !reviewing;
      if (this.previewOn.value) this.preview.render(this.editor.cursorLine() - 1);
    });
    effect([d.banner], () => clear(banner, this.bannerEl()));
    effect([d.lint], () => this.editor.setLint(d.lint.value));
    effect([d.resolutions], () => this.previewOn.value && this.preview.render());
  }

  private wire() {
    const d = this.doc;
    this.runs.plan.subscribe((p) => p && showRunConfirm(this, p), false);
    this.runs.findings.subscribe((r) => r && showFindings(this, r), false);
    on('menu', (m) => this.command(m.command));
    on('close_requested', (m) => this.close(!!m?.quitting));
    document.addEventListener('keydown', (e) => this.shortcut(e), true);
    window.addEventListener('blur', () => {
      if (d.dirty.value) d.save().catch(() => {});
    });
    window.addEventListener('focus', () => {
      d.resolveMedia(false);
      detect().catch(() => {});
    });
  }

  // MARK: Layout pieces

  private toolbar(): HTMLElement {
    const d = this.doc;
    const el = h('header', { class: 'toolbar', role: 'toolbar', 'aria-label': 'Project' });
    effect([d.project, d.runConfig, brands, this.runs.live, this.runs.preparing, d.validating, this.previewOn, environment], () => {
      const live = this.runs.live.value;
      const update: [string, string][] = d.brandUpdateAvailable && d.p.brand ? [['__update__', `Update “${d.p.brand.snapshot.name}” to Latest`]] : [];
      const brandSel = select([['', 'No brand'], ...brands.value.map((b) => [b.id, b.name] as [string, string]), ...update, ['__edit__', 'Edit Brand Presets…']], d.p.brand?.preset_id ?? '', (v) => {
        if (v === '__update__') {
          d.updateBrandToLatest();
          return;
        }
        if (v === '__edit__') {
          brandSel.value = d.p.brand?.preset_id ?? '';
          request('shell.open_window', { kind: 'library', options: { tab: 'brands', brand: d.p.brand?.preset_id } });
          return;
        }
        d.setBrand(brands.value.find((b) => b.id === v) ?? null);
      }, { 'aria-label': 'Brand preset', title: 'Brand preset' });
      const rcSel = select([...d.p.run_configs.map((r) => [r.id, r.name] as [string, string]), ['__edit__', 'Edit Run Configs…']], d.runConfig.value, (v) => {
        if (v === '__edit__') {
          rcSel.value = d.runConfig.value;
          this.showTab('outputs');
          return;
        }
        d.runConfig.value = v;
      }, { 'aria-label': 'Run config', title: 'Run config' });
      const last = this.runs.lastRun();
      const chip = live
        ? h('button', { class: 'status-chip live', onclick: () => this.showTab('run'), title: live.cost?.claude_estimated ? 'Estimated from token counts' : '' }, spinner(live.status), ` ${live.status} · ${elapsed((Date.now() - live.started) / 1000)}${live.cost?.claude_usd != null ? ` · ${usd(live.cost.claude_usd, live.cost.claude_estimated)}` : ''}`)
        : this.runs.preparing.value || d.validating.value
          ? h('span', { class: 'status-chip' }, spinner('Validating'), ' Validating…')
          : last
            ? h('button', { class: `status-chip ${last.status}`, onclick: () => ((this.runs.selectedRun.value = last.id), this.showTab('run')) }, `${STATUS_LABEL[last.status] ?? last.status}${last.cost_usd != null ? ` · ${usd(last.cost_usd)}` : ''}`)
            : h('span', { class: 'status-chip muted' }, 'No runs yet');
      clear(
        el,
        brandSel,
        rcSel,
        h('span', { class: 'toolbar-sep' }),
        button('Validate', () => (this.showTab('validation'), d.validate()), { glyph: Icons.validate, title: 'Validate (Ctrl+Shift+B)', disabled: d.validating.value || !!live }),
        live
          ? button(live.stopping ? 'Stopping…' : 'Stop', () => this.runs.stop(), { glyph: Icons.stop, title: 'Stop (Shift+F5)', disabled: live.stopping })
          : button('Run', () => this.runs.requestRun(), { appearance: 'primary', glyph: Icons.play, title: 'Run (F5)', disabled: !claudeAvailable() || this.runs.preparing.value }),
        live ? null : button('Audio Review', () => this.runs.requestRun(undefined, { audioReview: true }), { glyph: Icons.audio, title: 'Build and play the soundtrack from the spec, without rendering video (Ctrl+F5)', disabled: !claudeAvailable() || this.runs.preparing.value }),
        h('span', { class: 'grow' }),
        chip,
        iconButton(this.previewOn.value ? Icons.code : Icons.preview, this.previewOn.value ? 'Show source (Ctrl+/)' : 'Show preview (Ctrl+/)', () => (this.previewOn.value = !this.previewOn.value), { 'aria-pressed': String(this.previewOn.value) }),
      );
    });
    return el;
  }

  private inspector(): HTMLElement {
    const tabs: [InspectorTab, string][] = [['claude', 'Claude'], ['project', 'Project'], ['outputs', 'Outputs'], ['validation', 'Validation'], ['run', 'Run']];
    const builders: Record<InspectorTab, () => HTMLElement> = { claude: () => claudeTab(this), project: () => projectTab(this), outputs: () => outputsTab(this), validation: () => validationTab(this), run: () => runTab(this) };
    const panels = new Map<InspectorTab, HTMLElement>();
    const tablist = h('div', { class: 'tabs', role: 'tablist', 'aria-label': 'Inspector' });
    const body = h('div', { class: 'inspector-body' });
    effect([this.tab, this.doc.report, this.runs.live, this.assistant.review], () => {
      const r = this.doc.report.value;
      const badge = (t: InspectorTab) =>
        t === 'validation' && r?.blockers ? h('span', { class: 'dot danger', 'aria-label': 'blockers' }) :
        t === 'validation' && r?.warnings ? h('span', { class: 'dot warn', 'aria-label': 'warnings' }) :
        t === 'run' && this.runs.live.value?.question ? h('span', { class: 'dot accent', 'aria-label': 'question waiting' }) :
        t === 'claude' && this.assistant.review.value ? h('span', { class: 'dot accent', 'aria-label': 'review waiting' }) : null;
      clear(tablist, tabs.map(([t, label]) => h('button', { role: 'tab', id: `tab-${t}`, 'aria-selected': String(this.tab.value === t), 'aria-controls': `panel-${t}`, class: this.tab.value === t ? 'selected' : '', onclick: () => (this.tab.value = t) }, label, badge(t))));
      for (const [t] of tabs) {
        if (t === this.tab.value && !panels.has(t)) {
          const p = h('div', { role: 'tabpanel', id: `panel-${t}`, 'aria-labelledby': `tab-${t}`, class: 'tab-panel' }, builders[t]());
          panels.set(t, p);
          body.append(p);
        }
        const p = panels.get(t);
        if (p) p.hidden = t !== this.tab.value;
      }
    });
    return h('aside', { class: 'inspector', 'aria-label': 'Inspector' }, tablist, body);
  }

  private statusBar(): HTMLElement {
    const d = this.doc;
    const el = h('footer', { class: 'statusbar', role: 'status' });
    effect([d.lint, this.cursor, d.dirty, d.saving, d.project], () => {
      const lint = d.lint.value;
      const warn = lint.some((l) => l.severity === 'warning');
      const words = this.cursor.value.words ?? d.p.spec.split(/\s+/).filter(Boolean).length;
      const secs = (words / d.wordsPerMinute()) * 60;
      const lintBtn = lint.length
        ? h('button', { class: `link ${warn ? 'warn' : 'muted'}`, onclick: async () => {
            const items = lint.slice(0, 8);
            const c = await confirm(`${lint.length} spec note(s)`, items.map((l) => `Line ${l.line}: ${l.message}`).join('\n'), [...items.map((l) => `Go to line ${l.line}`), 'Close']);
            if (c < items.length) this.jumpToLine(items[c].line);
          } }, icon(warn ? Icons.warning : Icons.info), ` ${lint.length} spec note(s)`)
        : h('span', { class: 'muted' }, 'No spec notes');
      clear(
        el,
        lintBtn,
        h('span', { class: 'grow' }),
        h('span', { title: this.cursor.value.words !== null ? 'Selection' : 'Whole spec' }, `${words} words · ${Math.round(secs)} s`),
        h('span', null, `Ln ${this.cursor.value.line}, Col ${this.cursor.value.col}`),
        h('span', { class: 'muted' }, d.saving.value ? 'Saving…' : d.dirty.value ? 'Edited' : 'Saved'),
      );
    });
    return el;
  }

  private bannerEl(): HTMLElement | null {
    const b = this.doc.banner.value;
    if (!b) return null;
    if (b.kind === 'external')
      return h('fluent-message-bar', { intent: 'warning', class: 'banner' }, 'This project changed on disk while you have unsaved edits.', h('div', { slot: 'actions', class: 'row' }, button('Compare', () => this.compare()), button('Reload', () => this.doc.reload()), button('Keep Mine', () => this.doc.keepMine(), { appearance: 'primary' })));
    if (b.kind === 'brand')
      return h('fluent-message-bar', { intent: 'info', class: 'banner' }, `The brand preset “${b.name}” changed in the library.`, h('div', { slot: 'actions', class: 'row' }, button('Keep Snapshot', () => (this.doc.banner.value = null)), button('Update to Latest', () => this.doc.updateBrandToLatest(), { appearance: 'primary' })));
    const message = b.message;
    return h('fluent-message-bar', { intent: 'error', class: 'banner' }, message, h('div', { slot: 'actions', class: 'row' }, copyButton('Copy the message', () => message), button('Dismiss', () => (this.doc.banner.value = null))));
  }

  // MARK: Actions

  jumpToLine(line: number) {
    this.previewOn.value = false;
    this.editor.goToLine(line);
  }

  insertMediaRef(id: string) {
    this.previewOn.value = false;
    this.editor.insertMediaRefs([id]);
    this.editor.focus();
  }

  showRelink() {
    showRelink(this);
  }

  locate(id: string) {
    return locate(this, id);
  }

  async addMediaDialog() {
    const files = await openFiles('Add media', Filters.media, true, this.doc.projectDir());
    if (files.length) await this.doc.addMedia(files);
  }

  async addMediaFolderDialog() {
    const [dir] = await openFolders('Add a media folder', false, this.doc.projectDir());
    if (!dir) return;
    const which = await confirm('Which files?', dir, ['All media', 'Video only', 'Audio only', 'Images only', 'Cancel']);
    const kinds = [null, ['video'], ['audio'], ['image']][which];
    if (which === 4) return;
    await this.doc.addMedia([dir], kinds ?? undefined);
  }

  private async openMedia(id: string) {
    const r = this.doc.resolution(id);
    if (r?.path && r.status !== 'missing') request('shell.open', { path: r.path });
    else {
      this.sidebarApi.selectMedia(id);
      if (r?.status === 'missing') this.locate(id);
    }
  }

  private toggleTask(line: number) {
    const lines = this.editor.text.split('\n');
    const l = lines[line - 1];
    if (l === undefined) return;
    lines[line - 1] = /\[ \]/.test(l) ? l.replace('[ ]', '[x]') : l.replace(/\[[xX]\]/, '[ ]');
    this.editor.setText(lines.join('\n'), true);
    this.preview.render();
  }

  revealFix(fix: FixLink) {
    switch (fix.kind) {
      case 'spec':
        if (fix.line) this.jumpToLine(fix.line);
        break;
      case 'media':
        if (fix.target) {
          this.sidebarApi.selectMedia(fix.target);
          if (this.doc.resolution(fix.target)?.status === 'missing') this.showRelink();
        } else this.sidebarApi.tab.value = 'media';
        break;
      case 'brand':
        request('shell.open_window', { kind: 'library', options: { tab: 'brands', brand: this.doc.p.brand?.preset_id } });
        break;
      case 'outputs':
      case 'run_config':
        this.showTab('outputs');
        break;
      case 'project':
        this.showTab('project');
        break;
      case 'settings':
        request('shell.open_window', { kind: 'settings', options: { pane: fix.target ?? 'general' } });
        break;
    }
  }

  private async compare() {
    const disk = await this.doc.diskSpec();
    this.assistant.review.value = null;
    await this.assistant.propose(disk);
    toast('Showing the version on disk as changes to this window’s spec. Accept what you want to keep.');
    this.doc.banner.value = null;
  }

  // MARK: Files

  async saveAs() {
    const path = await saveFile('Save As', Filters.project, fileName(this.doc.path.value ?? 'Untitled.cutproj'), this.doc.projectDir());
    if (!path) return;
    try {
      const p = await core('project.save_as', { project: this.doc.p, old_path: this.doc.path.value, path });
      this.doc.project.value = p;
      this.doc.path.value = path;
      await request('shell.add_recent', { path });
      await request('shell.watch', { path });
      this.doc.title();
      this.doc.dirty.value = false;
      request('shell.set_dirty', { dirty: false });
      this.doc.resolveMedia(false);
      toast('Saved. Pasted images were copied next to the new file.');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async duplicate() {
    const s = sheet('Duplicate project');
    let keepRuns = false;
    let keepSessions = false;
    const runsBox = h('input', { type: 'checkbox', id: 'keep-runs' }) as HTMLInputElement;
    const sessBox = h('input', { type: 'checkbox', id: 'keep-sessions' }) as HTMLInputElement;
    runsBox.addEventListener('change', () => (keepRuns = runsBox.checked));
    sessBox.addEventListener('change', () => (keepSessions = sessBox.checked));
    s.body.append(h('label', { class: 'check' }, runsBox, h('span', null, 'Keep run history')), h('label', { class: 'check' }, sessBox, h('span', null, 'Keep Claude sessions')));
    s.footer.append(button('Cancel', () => s.close()), button('Duplicate…', () => s.close(true), { appearance: 'primary' }));
    if (!(await s.done)) return;
    await this.doc.save().catch(() => {});
    const dest = await saveFile('Duplicate as', Filters.project, `${fileName(this.doc.path.value!).replace(/\.cutproj$/i, '')} copy.cutproj`, this.doc.projectDir());
    if (!dest) return;
    try {
      await core('project.duplicate', { source: this.doc.path.value, destination: dest, keep_runs: keepRuns, keep_sessions: keepSessions });
      await request('shell.open_window', { kind: 'project', path: dest });
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async saveAsTemplate() {
    const name = await (await import('../dialogs')).prompt('Save as template', 'Template name', this.doc.p.name);
    if (!name) return;
    try {
      const tpl = await core('project.as_template', { project: this.doc.p, name });
      const path = joinPath(`${libraryDir.value}\\templates`, `${name.replace(/[\\/:*?"<>|]/g, '-')}.cutproj`);
      await core('project.save', { project: tpl, path });
      toast(`Template saved: ${path}`);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async exportSpec() {
    const dest = await saveFile('Export spec', Filters.markdown, `${fileName(this.doc.path.value ?? 'spec').replace(/\.cutproj$/i, '')}.md`, this.doc.projectDir());
    if (!dest) return;
    try {
      const text = await core<string>('project.export_spec', { project: this.doc.p, path: this.doc.path.value });
      const ok = await writeText(dest, text);
      toast(ok ? 'Spec exported. media: links are written as file paths.' : 'The spec couldn’t be written.', ok ? 'info' : 'error');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async cleanupPasted() {
    const paths = await core<string[]>('project.unused_pasted_images', { project: this.doc.p, path: this.doc.path.value });
    if (!paths.length) {
      toast('No unused pasted images.');
      return;
    }
    const c = await confirm(`Move ${paths.length} unused pasted image(s) to the Recycle Bin?`, paths.slice(0, 12).map(fileName).join('\n'), ['Move to Recycle Bin', 'Cancel']);
    if (c !== 0) return;
    const r = await request<{ moved: string[] }>('shell.recycle', { paths }).catch(() => null);
    const moved = new Set((r?.moved ?? []).map((p) => p.toLowerCase()));
    this.doc.update((p) => (p.media = p.media.filter((m) => !(m.pasted && moved.has((m.paths.absolute ?? '').toLowerCase())))));
    this.doc.resolveMedia(false);
    toast(`Moved ${moved.size} image(s) to the Recycle Bin.`);
  }

  private async close(quitting: boolean) {
    if (this.runs.running) {
      const c = await confirm('A run is still going', 'Closing stops it. Its logs are kept up to that point.', [quitting ? 'Stop and Quit' : 'Stop and Close', 'Cancel'], true);
      if (c !== 0) return;
      await this.runs.stop();
    }
    if (this.assistant.running.value) this.assistant.stop();
    try {
      await this.doc.save();
    } catch {
      const c = await confirm('The project couldn’t be saved', 'Close anyway and lose the unsaved changes?', ['Close Anyway', 'Cancel'], true);
      if (c !== 0) return;
    }
    await request('shell.close_window');
  }

  // MARK: Commands and shortcuts

  command(name: string) {
    const d = this.doc;
    switch (name) {
      case 'new_project': newProject(); break;
      case 'open_project':
        openFiles('Open project', Filters.project, true).then((fs) => fs.forEach((f) => request('shell.open_window', { kind: 'project', path: f })));
        break;
      case 'save': d.save().then(() => toast('Saved.')).catch(() => {}); break;
      case 'save_as': this.saveAs(); break;
      case 'duplicate': this.duplicate(); break;
      case 'save_as_template': this.saveAsTemplate(); break;
      case 'export_spec': this.exportSpec(); break;
      case 'add_media': this.addMediaDialog(); break;
      case 'cleanup_pasted': this.cleanupPasted(); break;
      case 'settings': request('shell.open_window', { kind: 'settings' }); break;
      case 'close': this.close(false); break;
      case 'quit': request('shell.quit'); break;
      case 'undo': this.editor.undo(); break;
      case 'redo': this.editor.redo(); break;
      case 'cut': document.execCommand('cut'); break;
      case 'copy': document.execCommand('copy'); break;
      case 'paste': clipboardText().then((t) => t != null && this.editor.view.dispatch(this.editor.view.state.replaceSelection(t))); break;
      case 'select_all': document.execCommand('selectAll'); break;
      case 'find':
      case 'replace': this.previewOn.value = false; this.editor.find(); break;
      case 'toggle_preview': this.previewOn.value = !this.previewOn.value; break;
      case 'sidebar_outline': this.sidebarApi.tab.value = 'outline'; break;
      case 'sidebar_media': this.sidebarApi.tab.value = 'media'; break;
      case 'sidebar_runs': this.sidebarApi.tab.value = 'runs'; break;
      case 'inspector_claude': this.showTab('claude'); break;
      case 'inspector_project': this.showTab('project'); break;
      case 'inspector_outputs': this.showTab('outputs'); break;
      case 'inspector_validation': this.showTab('validation'); break;
      case 'inspector_run': this.showTab('run'); break;
      case 'validate': this.showTab('validation'); d.validate(); break;
      case 'run': this.runs.requestRun(); break;
      case 'audio_review': this.runs.requestRun(undefined, { audioReview: true }); break;
      case 'stop': if (this.runs.running) this.runs.stop(); else this.assistant.stop(); break;
      case 'relink': this.showRelink(); break;
      case 'gather_info': this.showTab('claude'); this.assistant.gatherInfo(); break;
      case 'send_to_claude': {
        this.showTab('claude');
        const box = document.querySelector<HTMLTextAreaElement>('.composer');
        if (box?.value.trim()) box.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true }));
        else box?.focus();
        break;
      }
      case 'library': request('shell.open_window', { kind: 'library' }); break;
      case 'help': request('shell.open_url', { url: 'https://github.com/danjebaraj/video-editor/blob/main/docs/video-studio/README.md' }); break;
      case 'about': toast(`Syncfusion Video Studio for Windows`); break;
      case 'export_logs': { const r = this.runs.selected(); if (r) exportLogs(r.folder); break; }
    }
  }

  /** The Windows shortcuts (spec §23.6) and the usual editing ones. */
  private shortcut(e: KeyboardEvent) {
    const k = e.key.toLowerCase();
    // Windows reports Ctrl+Alt as AltGraph on some layouts, with ctrlKey and altKey cleared.
    const ctrlAlt = (e.ctrlKey && e.altKey) || e.getModifierState('AltGraph');
    const map: [boolean, string][] = [
      [e.key === 'F5' && !e.shiftKey && !e.ctrlKey, 'run'],
      [e.key === 'F5' && e.ctrlKey && !e.shiftKey, 'audio_review'],
      [e.key === 'F5' && e.shiftKey, 'stop'],
      [e.ctrlKey && e.shiftKey && k === 'b', 'validate'],
      [e.ctrlKey && !e.shiftKey && !e.altKey && (k === '/' || e.code === 'Slash'), 'toggle_preview'],
      [e.ctrlKey && e.shiftKey && k === 'l', 'library'],
      [e.ctrlKey && !e.shiftKey && k === ',', 'settings'],
      [e.ctrlKey && !e.shiftKey && !e.altKey && k === 's', 'save'],
      [e.ctrlKey && e.shiftKey && k === 's', 'save_as'],
      [e.ctrlKey && !e.shiftKey && k === 'o', 'open_project'],
      [e.ctrlKey && !e.shiftKey && k === 'n', 'new_project'],
      [e.ctrlKey && !e.shiftKey && k === 'w', 'close'],
      [e.ctrlKey && e.shiftKey && k === 'm', 'add_media'],
      [e.ctrlKey && !e.altKey && !e.shiftKey && k === '1', 'sidebar_outline'],
      [e.ctrlKey && !e.altKey && !e.shiftKey && k === '2', 'sidebar_media'],
      [e.ctrlKey && !e.altKey && !e.shiftKey && k === '3', 'sidebar_runs'],
      // e.code names the physical key; e.key covers input without a scan code (some remote and
      // assistive tools send only virtual keys).
      [ctrlAlt && (e.code === 'Digit1' || e.key === '1'), 'inspector_claude'],
      [ctrlAlt && (e.code === 'Digit2' || e.key === '2'), 'inspector_project'],
      [ctrlAlt && (e.code === 'Digit3' || e.key === '3'), 'inspector_outputs'],
      [ctrlAlt && (e.code === 'Digit4' || e.key === '4'), 'inspector_validation'],
      [ctrlAlt && (e.code === 'Digit5' || e.key === '5'), 'inspector_run'],
    ];
    // Ctrl+Enter in the composer sends there; elsewhere it is "Send to Claude".
    if (e.ctrlKey && e.key === 'Enter' && !(e.target as HTMLElement)?.classList?.contains('composer')) {
      e.preventDefault();
      this.command('send_to_claude');
      return;
    }
    for (const [hit, cmd] of map) {
      if (hit) {
        e.preventDefault();
        e.stopPropagation();
        this.command(cmd);
        return;
      }
    }
  }
}

/** A draggable divider between panes; widths persist for the session. */
function splitter(cssVar: string, dir: 1 | -1): HTMLElement {
  const el = h('div', { class: 'splitter', role: 'separator', 'aria-orientation': 'vertical', tabindex: '0', 'aria-label': cssVar.includes('sidebar') ? 'Resize the sidebar' : 'Resize the inspector' });
  const root = document.documentElement;
  const get = () => parseInt(getComputedStyle(root).getPropertyValue(cssVar)) || 300;
  el.addEventListener('pointerdown', (e) => {
    const start = e.clientX;
    const w0 = get();
    el.setPointerCapture(e.pointerId);
    const move = (m: PointerEvent) => root.style.setProperty(cssVar, `${Math.max(200, Math.min(640, w0 + dir * (m.clientX - start)))}px`);
    const up = () => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
  });
  el.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      root.style.setProperty(cssVar, `${Math.max(200, Math.min(640, get() + dir * (e.key === 'ArrowRight' ? 16 : -16)))}px`);
      e.preventDefault();
    }
  });
  return el;
}

/** Writes a text file the user chose in a save dialog (through the shell). */
async function writeText(path: string, text: string): Promise<boolean> {
  return request<boolean>('shell.write_text', { path, text }).catch(() => false);
}

export function mount(root: HTMLElement, info: ShellInfo) {
  const w = new ProjectWindow();
  (window as any).__vs = w;
  w.open(info, root);
  void parentDir;
}
