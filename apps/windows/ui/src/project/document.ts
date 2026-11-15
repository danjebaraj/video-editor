// One open project (the macOS ProjectDocument): the `.cutproj` model, autosave, external changes,
// media resolution, derived outline and lint, output resolution and validation.

import { core, request, on, errorText } from '../bridge';
import { Signal } from '../store';
import { debounce, fileStem, parentDir } from '../dom';
import { settings, keyStatus, environment, brands } from '../app-state';
import { confirm } from '../dialogs';
import type {
  Project,
  Resolution,
  OutlineItem,
  LintIssue,
  ResolvedOutput,
  Report,
  Readiness,
  BrandPreset,
  RunConfig,
} from '../types';

export type Banner =
  | { kind: 'external' }
  | { kind: 'brand'; name: string }
  | { kind: 'error'; message: string }
  | null;

export class ProjectDocument {
  path = new Signal<string | null>(null);
  project = new Signal<Project>(null as unknown as Project);
  /** The copy last read from or written to disk (external change detection). */
  private base: Project | null = null;
  dirty = new Signal(false);
  saving = new Signal(false);
  resolutions = new Signal<Resolution[]>([]);
  outline = new Signal<OutlineItem[]>([]);
  lint = new Signal<LintIssue[]>([]);
  outputs = new Signal<ResolvedOutput[]>([]);
  report = new Signal<Report | null>(null);
  validating = new Signal(false);
  acknowledged = new Set<string>();
  banner = new Signal<Banner>(null);
  runConfig = new Signal<string>('');
  /** Set by the editor: replaces the editor's text without an undo step. */
  onExternalSpec: (spec: string) => void = () => {};
  /** Asks the window to show the relink sheet. */
  onMissingMedia: () => void = () => {};
  private lastWrite = 0;

  private autosave = debounce(() => this.save().catch(() => {}), 1000);
  private derive = debounce(() => this.refreshDerived(), 300);

  async load(path: string) {
    const p = await core<Project>('project.load', { path });
    this.path.value = path;
    this.setLoaded(p);
    await request('shell.watch', { path });
    await request('shell.add_recent', { path });
    on('file_changed', (d) => {
      if (d.path === this.path.value) this.externalChange();
    });
    this.resolveMedia(true);
    this.checkBrand();
  }

  private setLoaded(p: Project) {
    this.base = structuredClone(p);
    this.project.value = p;
    if (!p.run_configs.some((r) => r.id === this.runConfig.value)) this.runConfig.value = p.run_configs[0]?.id ?? '';
    this.dirty.value = false;
    this.title();
    this.refreshDerived();
    this.refreshOutputs();
  }

  get p(): Project {
    return this.project.value;
  }

  currentRunConfig(): RunConfig | undefined {
    return this.p.run_configs.find((r) => r.id === this.runConfig.value) ?? this.p.run_configs[0];
  }

  title() {
    const name = this.p?.name || (this.path.value ? fileStem(this.path.value) : 'Untitled');
    request('shell.set_title', { title: `${name} — Syncfusion Video Studio` });
  }

  /** Changes the project; autosaves a second later. */
  update(change: (p: Project) => void, opts: { outputs?: boolean } = {}) {
    change(this.p);
    this.project.emit();
    this.markDirty();
    if (opts.outputs !== false) this.refreshOutputs();
    this.derive();
  }

  /** The editor's text changed. */
  setSpec(text: string) {
    if (text === this.p.spec) return;
    this.p.spec = text;
    this.markDirty();
    this.derive();
  }

  private markDirty() {
    if (!this.dirty.value) {
      this.dirty.value = true;
      request('shell.set_dirty', { dirty: true });
    }
    this.autosave();
  }

  async refreshDerived() {
    try {
      const [outline, lint] = await Promise.all([
        core<OutlineItem[]>('spec.outline', { spec: this.p.spec }),
        core<LintIssue[]>('spec.lint', { project: this.p }),
      ]);
      this.outline.value = outline;
      this.lint.value = lint;
    } catch {
      /* derived data is best effort */
    }
  }

  async refreshOutputs() {
    try {
      this.outputs.value = await core<ResolvedOutput[]>('outputs.resolve', { project: this.p });
    } catch {
      /* shown as errors per output */
    }
  }

  /** Saves now (merging run history the CLI may have appended first). */
  async save(): Promise<void> {
    this.autosave.cancel();
    const path = this.path.value;
    if (!path) return;
    this.saving.value = true;
    try {
      const merged = await core<{ project: Project; changed: boolean }>('project.merge_runs', { project: this.p, path }).catch(() => null);
      if (merged?.changed) this.p.runs = merged.project.runs;
      this.lastWrite = Date.now();
      const saved = await core<Project>('project.save', { project: this.p, path });
      this.p.modified = saved.modified;
      this.base = structuredClone(saved);
      this.dirty.value = false;
      request('shell.set_dirty', { dirty: false });
      if (this.banner.value?.kind === 'error') this.banner.value = null;
    } catch (e) {
      this.banner.value = { kind: 'error', message: `Couldn’t save: ${errorText(e)}` };
      throw e;
    } finally {
      this.saving.value = false;
    }
  }

  /** The file changed on disk (often the CLI appending a run). */
  async externalChange() {
    if (Date.now() - this.lastWrite < 1500 || !this.base || !this.path.value) return;
    try {
      const r = await core<{ change: 'none' | 'runs_only' | 'other'; disk: Project }>('project.external_change', { base: this.base, path: this.path.value });
      if (r.change === 'none') return;
      if (r.change === 'runs_only') {
        this.mergeRuns(r.disk);
        this.base = structuredClone(r.disk);
        return;
      }
      if (this.dirty.value) this.banner.value = { kind: 'external' };
      else this.reload();
    } catch {
      /* a half-written file: the next change event tries again */
    }
  }

  mergeRuns(disk: Project) {
    const byId = new Map(this.p.runs.map((r) => [r.id, r]));
    for (const d of disk.runs) {
      const ours = byId.get(d.id);
      if (!ours || ours.status === 'running' || d.finished) byId.set(d.id, d);
    }
    this.p.runs = [...byId.values()].sort((a, b) => (a.started ?? '').localeCompare(b.started ?? ''));
    this.project.emit();
  }

  async reload() {
    if (!this.path.value) return;
    const p = await core<Project>('project.load', { path: this.path.value });
    this.setLoaded(p);
    this.onExternalSpec(p.spec);
    this.banner.value = null;
    this.resolveMedia(false);
  }

  /** Keep Mine: take the disk's run history, keep everything else, and save. */
  async keepMine() {
    if (!this.path.value) return;
    const disk = await core<Project>('project.load', { path: this.path.value });
    this.mergeRuns(disk);
    this.banner.value = null;
    this.markDirty();
    await this.save();
  }

  /** Compare: both specs side by side in the review view. */
  async diskSpec(): Promise<string> {
    const disk = await core<Project>('project.load', { path: this.path.value! });
    return disk.spec;
  }

  // MARK: Media

  async resolveMedia(showRelink: boolean): Promise<void> {
    const path = this.path.value;
    const before = JSON.stringify(this.p.media.map((m) => m.id));
    try {
      const r = await core<{ project: Project; resolutions: Resolution[]; dirty: boolean }>('media.resolve', { project: this.p, path });
      if (JSON.stringify(this.p.media.map((m) => m.id)) !== before) return this.resolveMedia(showRelink);
      if (r.dirty) {
        this.p.media = r.project.media;
        this.markDirty();
      }
      this.resolutions.value = r.resolutions;
      this.allowMedia();
      if (showRelink && r.resolutions.some((x) => x.status === 'missing')) this.onMissingMedia();
      if (environment.value?.cutline || !environment.value) this.probe(r.resolutions);
    } catch (e) {
      this.banner.value = { kind: 'error', message: errorText(e) };
    }
  }

  private async probe(resolutions: Resolution[]) {
    try {
      const r = await core<{ project: Project; changed: boolean }>('media.probe', { project: this.p, resolutions });
      if (!r.changed) return;
      let changed = false;
      for (const m of r.project.media) {
        const ours = this.p.media.find((x) => x.id === m.id);
        if (ours && ours.paths.absolute === m.paths.absolute && ours.fingerprint?.quick_hash === m.fingerprint?.quick_hash && JSON.stringify(ours.probe) !== JSON.stringify(m.probe)) {
          ours.probe = m.probe;
          changed = true;
        }
      }
      if (changed) this.update(() => {}, { outputs: false });
    } catch {
      /* probing is optional */
    }
  }

  /** The files the page may load through the media host: this project's media and sidecar. */
  allowMedia() {
    const files = this.resolutions.value.map((r) => r.path).filter((p): p is string => !!p);
    const path = this.path.value;
    request('shell.allow_media', { files, sidecar: path ? `${path}.assets` : null });
  }

  resolution(id: string): Resolution | undefined {
    return this.resolutions.value.find((r) => r.id === id);
  }

  missing(): Resolution[] {
    return this.resolutions.value.filter((r) => r.status === 'missing');
  }

  async addMedia(files: string[], kinds?: string[]): Promise<string[]> {
    if (!files.length) return [];
    const r = await core<{ project: Project; ids: string[]; errors: string[] }>('media.add', { project: this.p, path: this.path.value, files, kinds: kinds ?? null });
    this.p.media = r.project.media;
    this.update(() => {});
    if (r.errors.length) this.banner.value = { kind: 'error', message: r.errors[0] };
    await this.resolveMedia(false);
    return r.ids;
  }

  /** Stores a pasted image in the sidecar and adds it as media; returns its id. */
  async addPastedImage(base64: string): Promise<string | null> {
    await this.save();
    const path = this.path.value;
    if (!path) return null;
    const file = await request<string>('project.store_pasted_image', { project_path: path, image_base64: base64 });
    const ids = await this.addMedia([file]);
    const id = ids[0];
    const item = this.p.media.find((m) => m.id === id);
    if (item && !item.pasted) this.update((p) => (p.media.find((m) => m.id === id)!.pasted = true));
    return id ?? null;
  }

  async removeMedia(id: string) {
    if (this.p.spec.includes(`media:${id}`)) {
      const c = await confirm(`Remove ${id}?`, 'The spec still refers to it. The file itself isn’t touched.', ['Remove', 'Cancel'], true);
      if (c !== 0) return;
    }
    this.update((p) => (p.media = p.media.filter((m) => m.id !== id)));
    this.resolveMedia(false);
  }

  async relink(id: string, file: string) {
    const r = await core<{ project: Project; prefix_fix: { old_prefix: string; new_prefix: string } | null; candidates: { id: string; path: string }[] }>('media.relink', { project: this.p, path: this.path.value, id, file });
    this.p.media = r.project.media;
    this.update(() => {});
    if (r.prefix_fix && r.candidates.length) {
      const c = await confirm(
        'Fix the other missing files too?',
        `${r.candidates.length} more missing file(s) are under ${r.prefix_fix.old_prefix}. Look for them under ${r.prefix_fix.new_prefix} instead?`,
        ['Apply Prefix Fix', 'Not Now'],
      );
      if (c === 0) {
        const fixed = await core<{ project: Project; relinked: string[] }>('media.apply_prefix_fix', { project: this.p, path: this.path.value, fix: r.prefix_fix });
        this.p.media = fixed.project.media;
        this.update(() => {});
      }
    }
    await this.resolveMedia(false);
  }

  async searchFolder(folder: string): Promise<string[]> {
    const r = await core<{ project: Project; relinked: string[] }>('media.search_folder', { project: this.p, path: this.path.value, folder });
    this.p.media = r.project.media;
    this.update(() => {});
    await this.resolveMedia(false);
    return r.relinked;
  }

  async acceptChanged(id: string) {
    const res = this.resolution(id);
    if (!res?.path) return;
    const p = await core<Project>('media.accept_changed', { project: this.p, path: this.path.value, id, file: res.path });
    this.p.media = p.media;
    this.update(() => {});
    await this.resolveMedia(false);
  }

  // MARK: Brand

  setBrand(b: BrandPreset | null) {
    this.update((p) => {
      p.brand = b ? { preset_id: b.id, snapshot: structuredClone(b) } : null;
    });
  }

  async checkBrand() {
    if (!this.p.brand) return;
    try {
      if (await core<boolean>('brand.is_newer', { brand: this.p.brand })) this.banner.value = { kind: 'brand', name: this.p.brand.snapshot.name };
    } catch {
      /* the preset may be gone from the library */
    }
  }

  /** True when the library holds a newer copy of the project's brand preset (the same test as
   *  the core's `brand.is_newer`, on the list the app already has). */
  get brandUpdateAvailable(): boolean {
    const b = this.p.brand;
    const latest = b && brands.value.find((x) => x.id === b.preset_id);
    if (!b || !latest) return false;
    const time = (s?: string) => (s ? Date.parse(s) || 0 : 0);
    return JSON.stringify(latest) !== JSON.stringify(b.snapshot) && time(latest.modified) > time(b.snapshot.modified);
  }

  updateBrandToLatest() {
    const b = brands.value.find((x) => x.id === this.p.brand?.preset_id);
    if (b) this.setBrand(b);
    this.banner.value = null;
  }

  // MARK: Validation

  async validate(kind: 'production' | 'dry_run' = 'production', readiness?: Readiness | null): Promise<Report | null> {
    this.validating.value = true;
    try {
      await this.save().catch(() => {});
      const r = await core<{ report: Report; project: Project; resolutions: Resolution[] }>('validate', {
        project: this.p,
        path: this.path.value,
        kind,
        run_config: this.runConfig.value || null,
        eleven_key_present: keyStatus.value?.present ?? false,
        eleven_remaining: null,
        environment: environment.value,
        readiness: readiness ?? null,
      });
      // probes are a cache worth keeping
      for (const m of r.project.media) {
        const ours = this.p.media.find((x) => x.id === m.id);
        if (ours && m.probe && JSON.stringify(ours.probe) !== JSON.stringify(m.probe)) ours.probe = m.probe;
      }
      this.resolutions.value = r.resolutions;
      this.allowMedia();
      const ids = new Set(r.report.issues.map((i) => i.id));
      for (const a of [...this.acknowledged]) if (!ids.has(a)) this.acknowledged.delete(a);
      this.report.value = r.report;
      return r.report;
    } catch (e) {
      this.banner.value = { kind: 'error', message: `Validation failed: ${errorText(e)}` };
      return null;
    } finally {
      this.validating.value = false;
    }
  }

  wordsPerMinute(): number {
    return settings.value?.words_per_minute ?? 150;
  }

  projectDir(): string {
    return this.path.value ? parentDir(this.path.value) : '';
  }
}
