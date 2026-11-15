// The spec assistant (the macOS AssistantModel and SpecReview): sessions with transcripts, one
// Claude turn at a time, questions, and Claude's edits reviewed per hunk before they reach the spec.

import { core, request, on, errorText } from '../bridge';
import { Signal } from '../store';
import { claudeAvailable } from '../app-state';
import { toast } from '../dialogs';
import type { ProjectDocument } from './document';
import type { ClaudeQuestion, Hunk } from '../types';

export interface ChatMessage {
  role: 'user' | 'assistant' | 'tool' | 'note' | 'usage' | 'error' | 'question';
  text: string;
  toolId?: string;
  detail?: string;
  failed?: boolean;
  attachment?: string;
  question?: { id: string; questions: ClaudeQuestion[]; state: 'pending' | 'answered' | 'cancelled'; answer?: string };
}

export interface ChatSession {
  id: string;
  name: string;
  claudeSession?: string | null;
  messages: ChatMessage[];
  created: string;
  autoNamed?: boolean;
}

export interface Review {
  base: string;
  proposed: string;
  hunks: Hunk[];
  decisions: (boolean | null)[];
}

function newId(): string {
  return `s${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

export class Assistant {
  sessions = new Signal<ChatSession[]>([]);
  current = new Signal<string>('');
  running = new Signal(false);
  review = new Signal<Review | null>(null);
  /** The composer's draft (Ask Claude to fix fills it). */
  draft = new Signal('');
  private token: number | null = null;
  private stopRequested = false;
  private sessionsDir = '';
  /** Set by the window: the editor's current text and the text it should show. */
  getSpec: () => string = () => this.doc.p.spec;
  applySpec: (text: string) => void = () => {};
  setReadOnly: (ro: boolean) => void = () => {};
  showTab: (tab: string) => void = () => {};
  /** Extra environment for the CLI (the self test passes the fake Claude's script). */
  extraEnv: Record<string, string> = {};

  constructor(private doc: ProjectDocument) {
    on('assistant', (d) => this.onEvent(d));
  }

  async load() {
    this.sessionsDir = await core<string>('assistant.sessions_dir', { project_id: this.doc.p.id });
    let list: ChatSession[] = await request<ChatSession[]>('assistant.load_sessions', { project_id: this.doc.p.id }).catch(() => []);
    for (const s of list) for (const m of s.messages) if (m.question?.state === 'pending') m.question.state = 'cancelled';
    for (const ref of this.doc.p.assistant_sessions ?? []) {
      if (!list.some((s) => s.id === ref.id)) list.push({ id: ref.id, name: ref.name, claudeSession: ref.claude_session, messages: [], created: new Date().toISOString() });
    }
    if (!list.length) list = [{ id: newId(), name: 'New session', messages: [], created: new Date().toISOString(), autoNamed: true }];
    this.sessions.value = list;
    this.current.value = list[list.length - 1].id;
  }

  session(): ChatSession {
    return this.sessions.value.find((s) => s.id === this.current.value) ?? this.sessions.value[0];
  }

  private push(m: ChatMessage) {
    this.session().messages.push(m);
    this.sessions.emit();
  }

  async persist() {
    const list = this.sessions.value;
    await request('assistant.save_sessions', { project_id: this.doc.p.id, sessions: list }).catch(() => {});
    const refs = list.filter((s) => s.messages.length && s.claudeSession).map((s) => ({ id: s.id, name: s.name, claude_session: s.claudeSession! }));
    if (JSON.stringify(refs) !== JSON.stringify(this.doc.p.assistant_sessions ?? [])) this.doc.update((p) => (p.assistant_sessions = refs), { outputs: false });
  }

  newSession() {
    const empty = this.sessions.value.find((s) => !s.messages.length);
    if (empty) {
      this.current.value = empty.id;
      return;
    }
    const s: ChatSession = { id: newId(), name: 'New session', messages: [], created: new Date().toISOString(), autoNamed: true };
    this.sessions.update((l) => l.push(s));
    this.current.value = s.id;
  }

  rename(id: string, name: string) {
    const s = this.sessions.value.find((x) => x.id === id);
    if (!s) return;
    s.name = name;
    s.autoNamed = false;
    this.sessions.emit();
    this.persist();
  }

  async remove(id: string) {
    await request('assistant.delete_session', { project_id: this.doc.p.id, session_id: id }).catch(() => {});
    this.sessions.update((l) => l.splice(l.findIndex((s) => s.id === id), 1));
    if (!this.sessions.value.length) this.newSession();
    if (this.current.value === id) this.current.value = this.sessions.value[this.sessions.value.length - 1].id;
    this.persist();
  }

  /** Sends one turn. `display` is what the transcript shows for it. */
  async send(prompt: string, opts: { display?: string; selection?: { text: string; from: number; to: number } } = {}) {
    if (this.running.value) return;
    if (!claudeAvailable()) {
      toast('Claude Code isn’t available. See Settings › Claude.', 'error');
      return;
    }
    if (this.review.value) {
      toast('Review Claude’s last change first.', 'error');
      return;
    }
    try {
      await this.doc.save();
    } catch {
      return;
    }
    let full = prompt;
    let attachment: string | undefined;
    if (opts.selection) {
      full += `\n\n---\nContext from the editor:\n- Selected text in spec.md (lines ${opts.selection.from}–${opts.selection.to}):\n\n\`\`\`markdown\n${opts.selection.text}\n\`\`\``;
      attachment = `Selection ${opts.selection.from}–${opts.selection.to}`;
    }
    const s = this.session();
    this.push({ role: 'user', text: opts.display ?? prompt, attachment });
    if (s.autoNamed && s.messages.filter((m) => m.role === 'user').length === 1) {
      s.name = (opts.display ?? prompt).slice(0, 48);
    }
    this.running.value = true;
    this.stopRequested = false;
    try {
      const summary = await core<{ summary: string; read_paths: string[] }>('project.summary', { project: this.doc.p, path: this.doc.path.value, run_config: this.doc.runConfig.value || null });
      const req = {
        session_dir: `${this.sessionsDir}\\${s.id}`,
        spec: this.getSpec(),
        prompt: full,
        model: this.doc.p.models.assistant,
        effort: this.doc.p.models.effort ?? null,
        web_access: !!this.doc.p.models.web_access,
        resume: s.claudeSession ?? null,
        read_paths: summary.read_paths,
        reference_paths: this.doc.p.assistant_folders ?? [],
        project_summary: summary.summary,
        helper: (await request<{ helper: string }>('shell.info')).helper,
        extra_env: this.extraEnv,
      };
      const r = await request<{ token: number }>('assistant.start', { request: req });
      this.token = r.token;
    } catch (e) {
      this.running.value = false;
      this.push({ role: 'error', text: errorText(e) });
    }
  }

  async gatherInfo() {
    const prompt = await core<string>('assistant.gather_info_prompt');
    this.send(prompt, { display: 'Gather missing info' });
  }

  stop() {
    if (this.token === null) return;
    this.stopRequested = true;
    request('assistant.stop', { token: this.token }).catch(() => {});
    for (const m of this.session().messages) if (m.question?.state === 'pending') m.question.state = 'cancelled';
    this.sessions.emit();
  }

  async answer(m: ChatMessage, answers: Record<string, string>) {
    if (!m.question || this.token === null) return;
    m.question.state = 'answered';
    m.question.answer = Object.values(answers).join('; ');
    this.sessions.emit();
    await request('assistant.answer', { token: this.token, question_id: m.question.id, answers });
  }

  async skip(m: ChatMessage) {
    if (!m.question || this.token === null) return;
    m.question.state = 'cancelled';
    this.sessions.emit();
    await request('assistant.dismiss', { token: this.token, question_id: m.question.id });
  }

  private onEvent(d: { token: number; event?: any; finished?: boolean }) {
    if (d.token !== this.token) return;
    if (d.finished) return;
    const e = d.event;
    const msgs = this.session().messages;
    const last = msgs[msgs.length - 1];
    switch (e?.event) {
      case 'started':
        this.session().claudeSession = e.session_id;
        break;
      case 'text_delta':
        if (last?.role === 'assistant' && (last as any).streaming) last.text += e.text;
        else msgs.push({ role: 'assistant', text: e.text, ...({ streaming: true } as any) });
        break;
      case 'text':
        if (last?.role === 'assistant' && (last as any).streaming) {
          last.text = e.text;
          delete (last as any).streaming;
        } else msgs.push({ role: 'assistant', text: e.text });
        break;
      case 'tool':
        msgs.push({ role: 'tool', text: e.summary || e.name, toolId: e.id, detail: e.detail ?? undefined });
        break;
      case 'tool_result':
        if (e.is_error) {
          const t = msgs.find((m) => m.toolId === e.id);
          if (t) {
            t.failed = true;
            if (e.detail) t.detail = e.detail;
          }
        }
        break;
      case 'blocked':
        msgs.push({ role: 'note', text: `Blocked: ${e.reason}` });
        break;
      case 'question':
        msgs.push({ role: 'question', text: '', question: { id: e.id, questions: e.questions, state: 'pending' } });
        this.showTab('claude');
        break;
      case 'spec_changed':
        this.propose(e.spec);
        break;
      case 'usage': {
        const k = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));
        msgs.push({ role: 'usage', text: `${k(e.input_tokens)} in · ${k(e.output_tokens)} out${e.cost_usd != null ? ` · $${e.cost_usd.toFixed(4)}` : ''}${e.duration_ms ? ` · ${(e.duration_ms / 1000).toFixed(1)}s` : ''}` });
        break;
      }
      case 'error':
        if (!this.stopRequested) msgs.push({ role: 'error', text: e.message });
        break;
      case 'finished':
        if (e.session_id) this.session().claudeSession = e.session_id;
        for (const m of msgs) if (m.question?.state === 'pending') m.question.state = 'cancelled';
        if (e.stopped) msgs.push({ role: 'note', text: 'Stopped.' });
        this.running.value = false;
        this.token = null;
        this.persist();
        break;
    }
    this.sessions.emit();
  }

  // MARK: Review (per hunk)

  async propose(spec: string) {
    const base = this.review.value?.base ?? this.getSpec();
    const hunks = await core<Hunk[]>('spec.diff', { old: base, new: spec });
    if (!hunks.length) {
      if (this.review.value) this.finish();
      return;
    }
    const prev = this.review.value;
    const decisions = hunks.map((hk) => {
      const i = prev?.hunks.findIndex((p) => JSON.stringify(p) === JSON.stringify(hk)) ?? -1;
      return i >= 0 ? prev!.decisions[i] : null;
    });
    this.review.value = { base, proposed: spec, hunks, decisions };
    this.setReadOnly(true);
    this.showTab('claude');
  }

  decide(i: number, accept: boolean) {
    const r = this.review.value;
    if (!r) return;
    r.decisions[i] = accept;
    this.review.emit();
    if (r.decisions.every((d) => d !== null)) this.finish();
  }

  acceptAll() {
    const r = this.review.value;
    if (!r) return;
    r.decisions = r.decisions.map(() => true);
    this.finish();
  }

  rejectAll() {
    const r = this.review.value;
    if (!r) return;
    r.decisions = r.decisions.map(() => false);
    this.finish();
  }

  /** Applies the accepted hunks (undecided ones count as rejected) as one edit. */
  async finish() {
    const r = this.review.value;
    if (!r) return;
    const accepted = r.decisions.map((d) => d === true);
    const text = accepted.some(Boolean) ? await core<string>('spec.apply_hunks', { old: r.base, hunks: r.hunks, accepted }) : r.base;
    this.review.value = null;
    this.setReadOnly(false);
    if (text !== this.getSpec()) this.applySpec(text);
  }

  /** Validation › Ask Claude to fix the spec: a new session with the findings drafted. */
  async askToFix(readiness: unknown) {
    this.newSession();
    this.draft.value = await core<string>('validation.fix_prompt', { readiness });
    this.showTab('claude');
  }
}
