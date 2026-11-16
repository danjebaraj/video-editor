// Inspector › Run (spec §12.3, §12.5, §15): the live run (stages, progress, cost, question,
// transcript, Stop) or a finished run's results, summary and logs.

import { h, clear, button, icon, Icons, usd, bytes, duration, elapsed, when, spinner, emptyState, select, fileName } from '../../dom';
import { core, request, errorText } from '../../bridge';
import { effect, Signal } from '../../store';
import { claudeAvailable, modelName } from '../../app-state';
import { saveFile, Filters, toast, sheet } from '../../dialogs';
import { questionCard } from '../../widgets';
import { copyButton, copyable, copyText, inline, markdownBlocks, markdownText, previewToggle } from '../../text-tools';
import { STATUS_LABEL } from '../../types';
import type { ProjectWindow } from '../window';
import type { RunRecord, RunResults, LogFile, OutputFile } from '../../types';

export function runTab(w: ProjectWindow): HTMLElement {
  const root = h('div', { class: 'run-tab' });
  const sub = new Signal<'results' | 'summary' | 'logs'>('results');
  let results: RunResults | null = null;
  let resultsFor = '';
  let timer: number | undefined;

  effect([w.runs.live, w.runs.selectedRun, w.doc.project, sub], () => {
    const live = w.runs.live.value;
    if (live) {
      renderLive();
      if (timer === undefined) timer = window.setInterval(() => w.runs.live.value && renderLive(), 1000);
      return;
    }
    if (timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
    }
    const rec = w.runs.selected();
    if (!rec) {
      clear(root, emptyState('No runs yet', 'Validate the spec, then run it.', button('Run', () => w.runs.requestRun(), { appearance: 'primary', glyph: Icons.play, disabled: !claudeAvailable() })));
      return;
    }
    if (resultsFor !== rec.id + rec.status) {
      resultsFor = rec.id + rec.status;
      results = null;
      if (rec.kind === 'dry_run') sub.value = 'summary';
      core<RunResults>('run.results', { folder: rec.folder })
        .then((r) => {
          results = r;
          renderResult(rec);
        })
        .catch(() => renderResult(rec));
    }
    renderResult(rec);
  });

  function renderLive() {
    const l = w.runs.live.value!;
    const secs = (Date.now() - l.started) / 1000;
    const stages = h('ol', { class: 'stages', 'aria-label': 'Stages' }, l.stages.map((s) => h('li', { class: `stage ${s.state}` },
      h('span', { class: 'stage-icon' }, s.state === 'active' ? spinner(s.label) : icon(s.state === 'done' ? Icons.checkCircle : s.state === 'failed' ? Icons.warning : '')),
      h('div', { class: 'grow' }, h('div', null, s.label), s.state === 'active' && l.fraction !== null ? h('fluent-progress-bar', { value: String(Math.round(l.fraction * 100)), max: '100', 'aria-label': `${s.label} progress` }) : null, s.state === 'active' && l.detail ? h('div', { class: 'muted small' }, l.detail) : null),
      s.seconds ? h('span', { class: 'muted small' }, `${Math.round(s.seconds)} s`) : null)));
    const cost = l.cost;
    clear(
      root,
      h('div', { class: 'tab-header' }, h('strong', null, l.kind === 'dry_run' ? 'Dry run' : 'Production run'), h('span', { class: 'muted' }, ` · ${modelName(l.model)} · ${elapsed(secs)}`), h('span', { class: 'grow' }), h('span', { class: 'muted small' }, l.status)),
      l.question
        ? h('div', null, questionCard(l.question.questions, (a) => w.runs.answer(a)), l.question.timeout_s ? h('p', { class: 'hint' }, `With no answer in ${l.question.timeout_s} s, Claude proceeds on its stated assumption.`) : null)
        : null,
      stages,
      h('dl', { class: 'estimates' },
        h('dt', null, 'Claude'), h('dd', null, usd(cost?.claude_usd, cost?.claude_estimated)),
        l.kind === 'production' ? [h('dt', null, 'ElevenLabs'), h('dd', null, `${cost?.eleven_credits ?? 0} of ${cost?.eleven_cap ?? 0} credits`)] : null),
      l.warnings.length ? h('ul', { class: 'warnings small' }, l.warnings.map((x) => h('li', { class: 'warn' }, x))) : null,
      h('details', { class: 'live-transcript' }, h('summary', null, 'Transcript'), h('div', { class: 'transcript small' }, l.transcript.slice(-200).map((t) => transcriptItem(t)), l.streaming ? h('div', { class: 'msg assistant' }, l.streaming) : null)),
      h('div', { class: 'row end' }, button('Reveal Folder', () => request('shell.reveal', { path: l.folder }), { glyph: Icons.openFolder }), button(l.stopping ? 'Stopping…' : 'Stop', () => w.runs.stop(), { glyph: Icons.stop, disabled: l.stopping })),
    );
  }

  function transcriptItem(t: { role: string; text: string; detail?: string; failed?: boolean }): HTMLElement {
    const failed = t.role === 'tool' && !!t.failed;
    const cls = `msg ${t.role}${failed ? ' failed' : ''}`;
    if (t.role === 'assistant') return h('div', { class: cls }, markdownText(t.text));
    if (t.role === 'error') return h('div', { class: cls }, copyable(h('span', null, t.text), 'Copy the error', () => t.text));
    if (failed) {
      // Not the run failing: a step that didn't work, which Claude reads and usually retries.
      const all = `${t.text}\n\n${t.detail ?? ''}`.trim();
      return h('div', { class: cls, title: `This step didn’t work. Claude sees the same message and usually tries again; the run continues.${t.detail ? `\n\n${t.detail}` : ''}` },
        copyable(h('span', { class: 'md-inline' }, icon(Icons.warning), ' ', ...inline(t.text)), 'Copy the details', () => all));
    }
    if (t.role === 'tool') return h('div', { class: `${cls} md-inline`, title: t.detail ?? '' }, icon(Icons.wrench), ' ', ...inline(t.text));
    return h('div', { class: cls }, t.text);
  }

  function renderResult(rec: RunRecord) {
    const rc = w.doc.p.run_configs.find((c) => c.id === rec.run_config);
    const failed = rec.status === 'failed';
    const head = h('div', { class: 'result-head' },
      h('div', { class: 'row' }, h('strong', { class: `status ${rec.status}` }, STATUS_LABEL[rec.status] ?? rec.status), rec.kind === 'dry_run' ? h('span', { class: 'pill' }, 'Dry run') : null),
      h('div', { class: 'muted small' }, [rc?.name ?? rec.run_config, when(rec.started), rec.model ? modelName(rec.model) : null, rec.cost_usd != null ? usd(rec.cost_usd) : null, rec.eleven_credits ? `${rec.eleven_credits} credits` : null].filter(Boolean).join(' · ')),
      results?.run?.message ? copyable(h('div', { class: failed ? 'danger small' : 'small' }, results.run.message), 'Copy the message', () => results?.run?.message ?? '') : null,
    );
    if (results && !results.exists) {
      clear(root, head, emptyState('The run folder has moved', rec.folder, button('Locate…', async () => {
        const [dir] = await (await import('../../dialogs')).openFolders('Locate the run folder');
        if (dir) w.doc.update((p) => (p.runs.find((r) => r.id === rec.id)!.folder = dir));
      })));
      return;
    }
    const tabs = h('div', { class: 'segmented', role: 'tablist', 'aria-label': 'Run details' }, (['results', 'summary', 'logs'] as const).map((t) => h('button', { role: 'tab', 'aria-selected': String(sub.value === t), class: sub.value === t ? 'selected' : '', onclick: () => (sub.value = t) }, t[0].toUpperCase() + t.slice(1))));
    let body: HTMLElement;
    if (!results) body = h('div', { class: 'center' }, spinner('Loading'));
    else if (sub.value === 'results') body = h('div', { class: 'outputs' }, results.outputs.length ? results.outputs.map((o) => outputCard(o)) : h('p', { class: 'muted' }, rec.kind === 'dry_run' ? 'A dry run doesn’t render outputs. See the summary.' : 'No outputs were produced.'), results.project_json5 ? button('View project.json5', () => viewText('project.json5', results!.project_json5!), { appearance: 'subtle' }) : null);
    else if (sub.value === 'summary') body = summary();
    else body = logs(rec);
    clear(
      root,
      head,
      tabs,
      body,
      h('div', { class: 'row end wrap' },
        button('Reveal Run Folder', () => request('shell.reveal', { path: rec.folder }), { glyph: Icons.openFolder }),
        rec.kind === 'production' ? button('Revise…', () => revise(rec), { glyph: Icons.revise, disabled: !rec.claude_session || !claudeAvailable() || w.runs.running }) : null,
        rec.kind === 'production' ? button('Re-run', () => w.runs.rerun(rec), { glyph: Icons.rerun, disabled: !claudeAvailable() || w.runs.running }) : null),
    );
  }

  function outputCard(o: OutputFile): HTMLElement {
    const verdictCls = o.qa_verdict === 'pass' ? 'ok' : o.qa_verdict === 'warnings' ? 'warn' : o.qa_verdict === 'fail' ? 'danger' : 'muted';
    let player: HTMLElement | null = null;
    if (o.exists) {
      const vid = h('video', { controls: true, preload: 'metadata', class: 'player', 'aria-label': `${o.output_name} preview` }) as HTMLVideoElement;
      request<string>('shell.allow_media', { files: [o.path, ...(o.poster ? [o.poster] : [])], add: true }).catch(() => {});
      request<string>('shell.media_url', { path: o.path }).then((u) => (vid.src = u)).catch(() => {});
      if (o.poster) request<string>('shell.media_url', { path: o.poster }).then((u) => (vid.poster = u)).catch(() => {});
      player = vid;
    }
    return h('article', { class: 'output-card', 'aria-label': o.output_name },
      h('div', { class: 'row' }, h('strong', null, o.output_name), h('span', { class: 'muted small' }, ` ${o.platform} · ${o.width}×${o.height}`), o.incomplete ? h('span', { class: 'badge warn' }, 'Incomplete') : null, h('span', { class: 'grow' }), h('span', { class: `badge ${verdictCls}` }, `QA ${o.qa_verdict || 'none'}`)),
      player ?? h('p', { class: 'muted' }, 'This file wasn’t produced.'),
      o.exists ? h('div', { class: 'muted small' }, `${duration(o.duration_s)} · ${bytes(o.bytes)} · ${fileName(o.path)}`) : null,
      o.qa_findings.length ? h('ul', { class: 'small qa' }, o.qa_findings.map((f) => h('li', { class: f.severity.toLowerCase().startsWith('err') ? 'danger' : 'warn' }, `${f.check}: ${f.message}`))) : null,
      o.exists ? h('div', { class: 'row wrap' },
        button('Reveal in File Explorer', () => request('shell.reveal', { path: o.path })),
        button('Open', () => request('shell.open', { path: o.path })),
        button('Open With…', () => request('shell.open', { path: o.path, application: '' })),
        button('Copy Path', () => copyText(o.path)),
        o.captions_srt ? button('Captions', () => request('shell.open', { path: o.captions_srt })) : null) : null,
    );
  }

  function summary(): HTMLElement {
    const r = results!;
    const rd = r.readiness;
    return h('div', { class: 'summary' },
      rd ? h('div', null,
        h('p', null, h('strong', { class: rd.ready ? 'ok' : 'warn' }, rd.ready ? 'Ready for production' : 'Not ready yet')),
        Object.keys(rd.estimated_durations).length ? h('ul', { class: 'small' }, Object.entries(rd.estimated_durations).map(([k, v]) => h('li', null, `${k}: ${duration(v)}`))) : null,
        button('Show Findings in Validation', async () => {
          await w.doc.validate('production', rd);
          w.showTab('validation');
        })) : null,
      r.summary ? markdownText(r.summary, 'summary-text') : h('p', { class: 'muted' }, 'No summary was written.'));
  }

  function logs(rec: RunRecord): HTMLElement {
    const r = results!;
    let source = 'all';
    let severity = 'all';
    let query = '';
    let file: LogFile | undefined = r.logs.find((l) => l.name === 'summary.md') ?? r.logs[0];
    // A Markdown log (the summary, the prompts) is read as a preview while no filter is on.
    let preview = true;
    let fullText = '';
    let shown = '';
    const list = h('ul', { class: 'log-files', 'aria-label': 'Log files' });
    const view = h('div', { class: 'log-view', tabindex: '0', 'aria-label': 'Log contents' });
    const tools = h('span', { class: 'row' });
    const isMarkdown = () => !!file?.name.toLowerCase().endsWith('.md');
    const filtered = () => !!query || severity !== 'all';
    const showsPreview = () => isMarkdown() && preview && !filtered();
    const renderView = () => {
      clear(view, showsPreview() ? markdownBlocks(fullText) : h('pre', null, shown));
      clear(tools,
        isMarkdown() ? previewToggle(preview, () => ((preview = !preview), renderView()), { disabled: filtered() }) : null,
        copyButton('Copy what is shown of this log', () => (showsPreview() ? fullText : shown)));
    };
    const refresh = async () => {
      clear(list, r.logs.filter((l) => source === 'all' || l.source === source).map((l) => h('li', null, h('button', { class: `link-row${l === file ? ' selected' : ''}`, onclick: () => ((file = l), refresh()) }, h('span', { class: 'grow' }, l.name), h('span', { class: 'muted small' }, bytes(l.bytes))))));
      fullText = shown = '';
      if (!file) return renderView();
      try {
        const url = await request<string>('shell.allow_media', { files: r.logs.map((l) => l.path), add: true }).then(() => request<string>('shell.media_url', { path: file!.path }));
        const res = await fetch(url, { headers: { Range: 'bytes=0-1500000' } });
        fullText = await res.text();
        const warnRe = /warn|W_|error|failed|E_|deny/i;
        const errRe = /error|failed|"is_error":true|E_|exit: 1|deny/i;
        let lines = fullText.split(/\r?\n/);
        if (severity === 'warning') lines = lines.filter((l) => warnRe.test(l));
        if (severity === 'error') lines = lines.filter((l) => errRe.test(l));
        if (query) lines = lines.filter((l) => l.toLowerCase().includes(query.toLowerCase()));
        shown = lines.slice(0, 4000).join('\n');
      } catch (e) {
        shown = errorText(e);
      }
      renderView();
    };
    const search = h('input', { type: 'search', placeholder: 'Search', 'aria-label': 'Search logs' }) as HTMLInputElement;
    search.addEventListener('input', () => ((query = search.value), refresh()));
    refresh();
    return h('div', { class: 'logs' },
      h('div', { class: 'row wrap' },
        select(['all', 'app', 'claude', 'cutline', 'eleven', 'helper', 'prompt', 'validation', 'qa', 'summary'].map((s) => [s, s === 'all' ? 'All sources' : s] as [string, string]), source, (v) => ((source = v), refresh()), { 'aria-label': 'Log source' }),
        select([['all', 'All lines'], ['warning', 'Warnings and errors'], ['error', 'Errors']], severity, (v) => ((severity = v), refresh()), { 'aria-label': 'Severity' }),
        search,
        tools),
      h('div', { class: 'log-split' }, list, view),
      h('div', { class: 'row wrap' },
        button('Reveal Logs Folder', () => request('shell.reveal', { path: `${rec.folder}\\logs` })),
        button('Export Logs…', () => exportLogs(rec.folder), { title: 'A zip of everything needed to study this run: logs, the instructions Claude was given, the spec and the edit. Media paths can be redacted.' }),
        button('Copy Issue Report', () => copyIssueReport(rec.folder))));
  }

  async function revise(rec: RunRecord) {
    const s = sheet('Revise this run');
    const box = h('textarea', { rows: 6, 'aria-label': 'What should change?', placeholder: 'For example: make the hook faster, use the logo at the end.' }) as HTMLTextAreaElement;
    s.body.append(h('p', { class: 'hint' }, 'Claude continues its session from this run and writes a new run folder linked to it.'), box);
    s.footer.append(button('Cancel', () => s.close()), button('Continue…', () => s.close(box.value.trim()), { appearance: 'primary' }));
    const feedback = await s.done;
    if (!feedback) return;
    w.doc.runConfig.value = rec.run_config;
    w.runs.requestRun({ previous_run: rec.id, feedback });
  }

  return root;
}

export async function exportLogs(folder: string) {
  const dest = await saveFile('Export logs', Filters.zip, `${fileName(folder)}-logs.zip`);
  if (!dest) return;
  const { confirm } = await import('../../dialogs');
  const redact =
    (await confirm(
      'Replace paths with placeholders?',
      "The zip holds this run's logs, the instructions Claude was given, the spec and the edit. No media is included. Replace media paths, media file names and your home folder with placeholders?",
      ['Keep Paths', 'Redact'],
    )) === 1;
  try {
    const n = await core<number>('logs.export', { folder, destination: dest, redact });
    toast(`Exported ${n} files${redact ? ' (paths replaced with placeholders)' : ''}.`);
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

/** A Markdown summary of cutline errors, warnings, failed QA checks and blocked tool calls. */
export async function copyIssueReport(folder: string) {
  try {
    const md = await core<string>('logs.issue_report', { folder });
    if (!(await copyText(md))) throw new Error('The clipboard is in use by another app. Try again.');
    toast('Issue report copied.');
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

function viewText(title: string, text: string) {
  const s = sheet(title, { wide: true });
  s.body.append(h('pre', { class: 'log-view', tabindex: '0' }, text));
  s.footer.append(button('Close', () => s.close(), { appearance: 'primary' }));
}
