// Runs of a project (the macOS RunController): validation first, the confirmation sheet, an
// optional dry run, the production run or an Audio Review with live stages, questions, cost and
// transcript, Stop, and the end (notification, taskbar, results).

import { core, request, on, errorText } from '../bridge';
import { Signal } from '../store';
import { settings, assistantAvailable, assistantSetupProblem, provider, effectiveModelLabel } from '../app-state';
import { toast } from '../dialogs';
import type { ProjectDocument } from './document';
import type { Readiness, Report, ResolvedOutput, ClaudeQuestion, RunRecord } from '../types';
import { STATUS_LABEL } from '../types';

export const STAGES: [string, string][] = [
  ['prepare', 'Prepare'],
  ['audio', 'Audio'],
  ['capture', 'Recording'],
  ['authoring', 'Authoring'],
  ['validate', 'Validate'],
  ['review', 'Review'],
  ['mix', 'Mix'],
  ['render', 'Render'],
  ['qa', 'QA'],
  ['done', 'Done'],
];

export type RunKindName = 'dry_run' | 'production' | 'audio_review';

/** The stages a run of this kind goes through: a dry run renders nothing and makes no audio; an
 *  Audio Review makes audio and mixes it, but records, reviews and renders no picture. */
export function showsStage(stage: string, kind: RunKindName): boolean {
  if (kind === 'dry_run') return !['audio', 'capture', 'mix', 'render', 'qa'].includes(stage);
  if (kind === 'audio_review') return !['capture', 'review', 'render', 'qa'].includes(stage);
  return stage !== 'mix';
}

export function kindLabel(kind: string): string {
  return kind === 'dry_run' ? 'Dry run' : kind === 'audio_review' ? 'Audio Review' : 'Production run';
}

export interface StageState {
  id: string;
  label: string;
  state: 'pending' | 'active' | 'done' | 'failed';
  seconds: number;
}

export interface TranscriptItem {
  role: string;
  text: string;
  tool?: string;
  detail?: string;
  /** A tool call that came back as an error; the run goes on. */
  failed?: boolean;
}

export interface RunPlan {
  runConfig: string;
  outputs: ResolvedOutput[];
  report: Report;
  outputFolder: string;
  model: string;
  dryRunFirst: boolean;
  revise?: { previous_run: string; feedback: string };
  /** `production`, or `audio_review` for an Audio Review (FEAT-neat-osprey-DKK7). */
  kind: 'production' | 'audio_review';
  /** Audio Review: the output (ID) whose timing and audio settings are auditioned. */
  audioOutput?: string;
  /** This run's Codex model when Codex is selected (BUG-fond-crane-62ZP): starts as the run config's
   *  Codex choice; empty follows Settings › Codex, then Codex's default. `model` stays Claude's. */
  codexModel?: string;
}

export interface Live {
  token: number;
  kind: RunKindName;
  runId: string;
  folder: string;
  model: string;
  /** The provider this run started with (its cost is attributed to it); a later switch doesn't change it. */
  provider: 'claude' | 'codex';
  started: number;
  stages: StageState[];
  fraction: number | null;
  detail: string;
  status: string;
  stopping: boolean;
  question: { id: string; questions: ClaudeQuestion[]; timeout_s?: number } | null;
  cost: { claude_usd?: number; claude_estimated?: boolean; eleven_credits?: number; eleven_cap?: number } | null;
  warnings: string[];
  transcript: TranscriptItem[];
  streaming: string;
}

export class RunController {
  live = new Signal<Live | null>(null);
  plan = new Signal<RunPlan | null>(null);
  findings = new Signal<Readiness | null>(null);
  selectedRun = new Signal<string | null>(null);
  preparing = new Signal(false);
  /** Set by the window. */
  showTab: (tab: string) => void = () => {};
  private continuation: RunPlan | null = null;

  constructor(private doc: ProjectDocument) {
    on('run', (d) => this.onEvent(d));
  }

  get running(): boolean {
    return !!this.live.value;
  }

  /** Run (F5): validate, then the confirmation sheet. With `audioReview` (Ctrl+F5) the sheet
   *  starts an Audio Review: the soundtrack of one output, with no dry run and no picture. */
  async requestRun(revise?: { previous_run: string; feedback: string }, opts: { audioReview?: boolean; audioOutput?: string } = {}) {
    if (this.running || this.preparing.value) return;
    const rc = this.doc.currentRunConfig();
    if (!rc) {
      toast('Add a run config in the Outputs tab first.', 'error');
      this.showTab('outputs');
      return;
    }
    // The provider selected now (Settings › Assistant); the core reads it again as the run starts.
    if (!assistantAvailable()) {
      const p = assistantSetupProblem();
      toast(`${p.title} ${p.detail}`, 'error');
      return;
    }
    this.preparing.value = true;
    try {
      const report = await this.doc.validate(opts.audioReview ? 'audio_review' : 'production');
      if (!report) return;
      if (report.blockers > 0) {
        this.showTab('validation');
        toast(`${report.blockers} blocker(s) must be fixed before this run.`, 'error');
        return;
      }
      const outputs = this.doc.outputs.value.filter((o) => rc.outputs.includes(o.id));
      if (opts.audioReview) {
        // Allowed as a project's first run: no dry run, and the review goes in the library.
        const audioOutput = outputs.find((o) => o.id === opts.audioOutput)?.id ?? outputs[0]?.id;
        this.plan.value = { runConfig: rc.id, outputs, report, outputFolder: '', model: rc.production_model, codexModel: rc.codex_production_model ?? '', dryRunFirst: false, revise, kind: 'audio_review', audioOutput };
        return;
      }
      const outputFolder = await core<string>('outputs.root', { project: this.doc.p, run_config: rc.id }).catch(() => '');
      this.plan.value = { runConfig: rc.id, outputs, report, outputFolder, model: rc.production_model, codexModel: rc.codex_production_model ?? '', dryRunFirst: rc.dry_run_first && !revise, revise, kind: 'production' };
    } finally {
      this.preparing.value = false;
    }
  }

  /** Validate › Dry Run: a dry run on its own. */
  async requestDryRun() {
    if (this.running) return;
    const report = await this.doc.validate('dry_run');
    if (!report) return;
    if (report.blockers > 0) {
      this.showTab('validation');
      return;
    }
    this.continuation = null;
    await this.launch('dry_run', this.doc.dryRunModel, report);
  }

  async confirm(plan: RunPlan) {
    this.plan.value = null;
    for (const i of plan.report.issues) if (i.severity === 'warning') this.doc.acknowledged.add(i.id);
    if (plan.kind === 'audio_review') {
      this.continuation = null;
      await this.launch('audio_review', plan.model, plan.report, plan.revise, {}, plan.audioOutput, plan.codexModel);
    } else if (plan.dryRunFirst) {
      this.continuation = plan;
      await this.launch('dry_run', this.doc.dryRunModel, plan.report);
    } else {
      this.continuation = null;
      await this.launch('production', plan.model, plan.report, plan.revise, {}, undefined, plan.codexModel);
    }
  }

  async launch(kind: RunKindName, model: string, report: Report, revise?: { previous_run: string; feedback: string }, testOptions: Record<string, unknown> = {}, audioOutput?: string, codexModel?: string) {
    const rc = this.doc.currentRunConfig()!;
    await this.doc.save().catch(() => {});
    const request_: Record<string, unknown> = {
      project_path: this.doc.path.value,
      run_config: rc.id,
      kind,
      assume_answers: false,
      validation: report,
    };
    // A Claude model isn't sent to Codex, and a Codex model isn't sent to Claude
    // (BUG-fond-crane-62ZP). Dry runs use the project's dry run model for the provider.
    const runProvider = provider();
    if (kind !== 'dry_run' && runProvider === 'claude' && model && model !== rc.production_model) request_.model = model;
    const codexOverride = (codexModel ?? '').trim();
    if (kind !== 'dry_run' && runProvider === 'codex' && codexOverride && codexOverride !== (rc.codex_production_model ?? '')) request_.codex_model = codexOverride;
    if (kind !== 'dry_run' && revise) request_.revise = revise;
    if (kind === 'audio_review' && audioOutput) request_.audio_output = audioOutput;
    if (rc.question_timeout_s) request_.question_timeout_s = rc.question_timeout_s;
    try {
      const started = await request<{ token: number; run_id: string; folder: string }>('run.start', { request: request_, options: { ...this.extraOptions, ...testOptions } });
      this.live.value = {
        token: started.token,
        kind,
        runId: started.run_id,
        folder: started.folder,
        model: runProvider === 'codex' ? effectiveModelLabel(this.doc.p.models, rc, kind === 'dry_run' ? 'dry_run' : 'production', kind === 'dry_run' ? null : codexOverride) : kind === 'dry_run' ? model : model || rc.production_model,
        provider: runProvider,
        started: Date.now(),
        stages: STAGES.filter(([id]) => showsStage(id, kind)).map(([id, label]) => ({ id, label, state: 'pending', seconds: 0 })),
        fraction: null,
        detail: '',
        status: kind === 'dry_run' ? 'Dry run' : kind === 'audio_review' ? 'Audio Review' : 'Running',
        stopping: false,
        question: null,
        cost: null,
        warnings: [],
        transcript: [],
        streaming: '',
      };
      this.selectedRun.value = started.run_id;
      this.showTab('run');
      request('shell.progress', { state: 'indeterminate' });
      this.refreshHistory();
    } catch (e) {
      toast(`The run couldn’t start: ${errorText(e)}`, 'error');
    }
  }

  /** Extra start options (the self test passes the fake Claude's script). */
  extraOptions: Record<string, unknown> = {};

  private onEvent(d: { token: number; event?: any; finished?: any }) {
    const live = this.live.value;
    if (!live || d.token !== live.token) return;
    if (d.finished) {
      this.finished(d.finished);
      return;
    }
    const e = d.event;
    if (!e) return;
    const stage = (id: string) => live.stages.find((s) => s.id === id);
    switch (e.event) {
      case 'run_started':
        live.folder = e.folder;
        live.model = e.model;
        break;
      case 'stage_started': {
        for (const s of live.stages) if (s.state === 'active') s.state = 'done';
        const s = stage(e.stage);
        if (s) {
          s.state = e.stage === 'done' ? 'done' : 'active';
          if (e.stage === 'render' && e.label) s.label = e.label;
        }
        live.fraction = null;
        live.detail = '';
        live.status = e.label;
        break;
      }
      case 'stage_finished': {
        const s = stage(e.stage);
        if (s) {
          s.state = e.ok === false ? 'failed' : 'done';
          s.seconds += e.seconds ?? 0;
        }
        break;
      }
      case 'progress':
        live.fraction = e.fraction ?? null;
        live.detail = e.detail ?? '';
        if (e.fraction !== undefined && e.fraction !== null) request('shell.progress', { state: 'normal', value: e.fraction });
        break;
      case 'text_delta':
        live.streaming += e.text;
        break;
      case 'message':
        live.streaming = '';
        live.transcript.push({ role: e.role, text: e.text, tool: e.tool, detail: e.detail, failed: !!e.failed });
        if (live.transcript.length > 600) live.transcript.splice(0, live.transcript.length - 600);
        break;
      case 'question':
        live.question = { id: e.id, questions: e.questions, timeout_s: e.timeout_s };
        live.status = 'Waiting for your answer';
        this.showTab('run');
        request('shell.progress', { state: 'paused', value: live.fraction ?? 0.5 });
        if (!document.hasFocus() && settings.value?.notifications !== false) request('shell.notify', { title: 'The Assistant has a question', body: e.questions?.[0]?.question ?? '' });
        break;
      case 'question_answered':
        live.question = null;
        request('shell.progress', { state: 'indeterminate' });
        break;
      case 'warning':
        live.warnings.push(e.message);
        break;
      case 'cost':
        live.cost = e;
        break;
      case 'run_finished':
        live.status = STATUS_LABEL[e.status] ?? e.status;
        if (e.cost_usd !== undefined) live.cost = { ...(live.cost ?? {}), claude_usd: e.cost_usd, claude_estimated: false };
        break;
    }
    this.live.emit();
  }

  async answer(answers: Record<string, string>) {
    const live = this.live.value;
    if (!live?.question) return;
    await request('run.answer', { token: live.token, question_id: live.question.id, answers });
  }

  async stop() {
    const live = this.live.value;
    if (!live || live.stopping) return;
    live.stopping = true;
    live.status = 'Stopping…';
    this.continuation = null;
    this.live.emit();
    await request('run.cancel', { token: live.token }).catch(() => {});
  }

  private async finished(outcome: any) {
    const live = this.live.value!;
    this.live.value = null;
    const status: string = outcome?.status ?? 'failed';
    request('shell.progress', { state: status === 'failed' ? 'error' : 'none', value: 1 });
    if (status !== 'failed') setTimeout(() => request('shell.progress', { state: 'none' }), 50);
    await this.refreshHistory();
    this.selectedRun.value = outcome?.run_id ?? live.runId;
    const rcName = this.doc.currentRunConfig()?.name ?? 'Run';
    const title = live.kind === 'production' ? `${rcName}: ${STATUS_LABEL[status] ?? status}` : `${kindLabel(live.kind)}: ${STATUS_LABEL[status] ?? status}`;
    const body = outcome?.message ?? `${outcome?.outputs?.length ?? 0} output(s)`;
    if (settings.value?.notifications !== false && !document.hasFocus()) request('shell.notify', { title, body });
    if (live.kind === 'dry_run') {
      const readiness: Readiness | null = outcome?.readiness ?? null;
      if (readiness) await this.doc.validate('production', readiness);
      const plan = this.continuation;
      this.continuation = null;
      if (status === 'failed' || status === 'cancelled') {
        this.showTab('run');
        return;
      }
      if (!plan) {
        this.showTab('validation');
        return;
      }
      const clean = readiness && readiness.ready && !readiness.ambiguities.length && !readiness.missing_media.length && !readiness.risks.length;
      if (clean) {
        await this.launch('production', plan.model, this.doc.report.value ?? plan.report, plan.revise, {}, undefined, plan.codexModel);
      } else {
        this.findings.value = readiness;
        this.pendingPlan = plan;
      }
    } else {
      this.showTab('run');
    }
  }

  /** The plan waiting behind the dry run findings sheet. */
  pendingPlan: RunPlan | null = null;

  async continueAfterDryRun() {
    const plan = this.pendingPlan;
    this.pendingPlan = null;
    this.findings.value = null;
    if (plan) await this.launch('production', plan.model, this.doc.report.value ?? plan.report, plan.revise, {}, undefined, plan.codexModel);
  }

  /** Reloads run history from disk (the orchestrator appends it). */
  async refreshHistory() {
    if (!this.doc.path.value) return;
    try {
      const disk = await core('project.load', { path: this.doc.path.value });
      this.doc.mergeRuns(disk);
    } catch {
      /* next time */
    }
  }

  lastRun(): RunRecord | undefined {
    const runs = this.doc.p.runs;
    return runs[runs.length - 1];
  }

  selected(): RunRecord | undefined {
    const id = this.selectedRun.value;
    return this.doc.p.runs.find((r) => r.id === id) ?? this.lastRun();
  }

  /** Re-run: the same run config again (an Audio Review auditions the same output again). */
  rerun(record: RunRecord, audioOutput?: string) {
    this.doc.runConfig.value = record.run_config;
    this.requestRun(undefined, { audioReview: record.kind === 'audio_review', audioOutput });
  }
}
