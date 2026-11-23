// Inspector › Run (spec §12.3, §12.5, §15): the live run (stages, progress, cost, question,
// transcript, Stop) or a finished run's results, summary and logs.

import { h, clear, button, iconButton, icon, Icons, usd, bytes, duration, elapsed, when, spinner, emptyState, select, fileName } from '../../dom';
import { core, request, errorText } from '../../bridge';
import { effect, Signal } from '../../store';
import { assistantAvailable, modelName, readiness, settings } from '../../app-state';
import { saveFile, Filters, toast, sheet } from '../../dialogs';
import { questionCard } from '../../widgets';
import { copyButton, copyable, copyText, inline, markdownBlocks, markdownText, previewToggle } from '../../text-tools';
import { STATUS_LABEL } from '../../types';
import type { ProjectWindow } from '../window';
import type { RunRecord, RunResults, LogFile, OutputFile, BlockingInput } from '../../types';
import { kindLabel } from '../runs';

export function runTab(w: ProjectWindow): HTMLElement {
  const root = h('div', { class: 'run-tab' });
  const sub = new Signal<'results' | 'summary' | 'logs'>('results');
  let results: RunResults | null = null;
  let resultsFor = '';
  let timer: number | undefined;
  // The live inspector is rebuilt on every update; the Transcript's open state and reading position
  // live here so a rebuild keeps them (FEAT-humble-parrot-M68S).
  let transcriptOpen = true;
  const itemKeys = new WeakMap<object, number>();
  let nextKey = 1;
  const keyOf = (t: object) => {
    let k = itemKeys.get(t);
    if (k === undefined) itemKeys.set(t, (k = nextKey++));
    return k;
  };

  effect([w.runs.live, w.runs.selectedRun, w.doc.project, sub, settings, readiness], () => {
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
      clear(root, emptyState('No runs yet', 'Validate the spec, then run it. Audio Review builds only the soundtrack, to listen to before any video is rendered.', h('div', { class: 'row' },
        button('Run', () => w.runs.requestRun(), { appearance: 'primary', glyph: Icons.play, disabled: !assistantAvailable() }),
        button('Audio Review', () => w.runs.requestRun(undefined, { audioReview: true }), { glyph: Icons.audio, disabled: !assistantAvailable() }))));
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

  /** The first transcript entry visible at the top of the scroll area, and how far above or below
   *  the top it sits, so the same entry can be put back in the same place after a rebuild. */
  function readingPosition(): { key: string; offset: number; scrollTop: number } | null {
    const body = root.querySelector<HTMLElement>('.live-transcript-body');
    if (!body) return null;
    const top = body.scrollTop;
    for (const el of Array.from(body.querySelectorAll<HTMLElement>('[data-key]'))) {
      if (el.offsetTop + el.offsetHeight > top) return { key: el.dataset.key!, offset: el.offsetTop - top, scrollTop: top };
    }
    return { key: '', offset: 0, scrollTop: top };
  }

  function restoreReadingPosition(pos: { key: string; offset: number; scrollTop: number } | null) {
    const body = root.querySelector<HTMLElement>('.live-transcript-body');
    if (!body || !pos) return;
    const el = pos.key ? body.querySelector<HTMLElement>(`[data-key="${pos.key}"]`) : null;
    body.scrollTop = el ? el.offsetTop - pos.offset : pos.scrollTop;
  }

  function renderLive() {
    const l = w.runs.live.value!;
    const secs = (Date.now() - l.started) / 1000;
    const pos = readingPosition();
    const stages = h('ol', { class: 'stages', 'aria-label': 'Stages' }, l.stages.map((s) => h('li', { class: `stage ${s.state}` },
      h('span', { class: 'stage-icon' }, s.state === 'active' ? spinner(s.label) : icon(s.state === 'done' ? Icons.checkCircle : s.state === 'failed' ? Icons.warning : '')),
      h('div', { class: 'grow' }, h('div', null, s.label), s.state === 'active' && l.fraction !== null ? h('fluent-progress-bar', { value: String(Math.round(l.fraction * 100)), max: '100', 'aria-label': `${s.label} progress` }) : null, s.state === 'active' && l.detail ? h('div', { class: 'muted small' }, l.detail) : null),
      s.seconds ? h('span', { class: 'muted small' }, `${Math.round(s.seconds)} s`) : null)));
    const cost = l.cost;
    const toggle = h('button', {
      class: 'live-transcript-toggle',
      'aria-expanded': String(transcriptOpen),
      'aria-controls': 'live-transcript-body',
      onclick: () => {
        transcriptOpen = !transcriptOpen;
        renderLive();
      },
    }, h('span', { class: 'chevron', 'aria-hidden': 'true' }, transcriptOpen ? '▾' : '▸'), ' Transcript');
    const body = transcriptOpen
      ? h('div', { class: 'live-transcript-body transcript small', id: 'live-transcript-body', role: 'log', 'aria-label': 'Transcript entries', tabindex: '0' },
          l.transcript.map((t) => {
            const el = transcriptItem(t);
            el.dataset.key = String(keyOf(t));
            return el;
          }),
          l.streaming ? h('div', { class: 'msg assistant' }, l.streaming) : null)
      : null;
    clear(
      root,
      h('div', { class: `live-details${transcriptOpen ? ' capped' : ''}` },
        h('div', { class: 'tab-header' }, h('strong', null, kindLabel(l.kind)), h('span', { class: 'muted' }, ` · ${modelName(l.model)} · ${elapsed(secs)}`), h('span', { class: 'grow' }), h('span', { class: 'muted small' }, l.status)),
        l.question
          ? h('div', { class: 'live-pad' }, questionCard(l.question.questions, (a) => w.runs.answer(a)), l.question.timeout_s ? h('p', { class: 'hint' }, `With no answer in ${l.question.timeout_s} s, the Assistant proceeds on its stated assumption.`) : null)
          : null,
        stages,
        h('dl', { class: 'estimates' },
          h('dt', null, l.provider === 'codex' ? 'Codex' : 'Claude Code'), h('dd', null, usd(cost?.claude_usd, cost?.claude_estimated)),
          l.kind !== 'dry_run' ? [h('dt', null, 'ElevenLabs'), h('dd', null, `${cost?.eleven_credits ?? 0} of ${cost?.eleven_cap ?? 0} credits`)] : null),
        l.warnings.length ? h('ul', { class: 'warnings small live-pad' }, l.warnings.map((x) => h('li', { class: 'warn' }, x))) : null),
      h('div', { class: 'live-transcript-head' }, toggle),
      body ?? h('div', { class: 'live-spacer' }),
      h('div', { class: 'row end live-actions' }, button('Reveal Folder', () => request('shell.reveal', { path: l.folder }), { glyph: Icons.openFolder }), button(l.stopping ? 'Stopping…' : 'Stop', () => w.runs.stop(), { glyph: Icons.stop, disabled: l.stopping })),
    );
    root.classList.add('live');
    restoreReadingPosition(pos);
  }

  function transcriptItem(t: { role: string; text: string; detail?: string; failed?: boolean }): HTMLElement {
    const failed = t.role === 'tool' && !!t.failed;
    const cls = `msg ${t.role}${failed ? ' failed' : ''}`;
    if (t.role === 'assistant') return h('div', { class: cls }, markdownText(t.text));
    if (t.role === 'error') return h('div', { class: cls }, copyable(h('span', null, t.text), 'Copy the error', () => t.text));
    if (failed) {
      // Not the run failing: a step that didn't work, which the Assistant reads and usually retries.
      const all = `${t.text}\n\n${t.detail ?? ''}`.trim();
      return h('div', { class: cls, title: `This step didn’t work. The Assistant sees the same message and usually tries again; the run continues.${t.detail ? `\n\n${t.detail}` : ''}` },
        copyable(h('span', { class: 'md-inline' }, icon(Icons.warning), ' ', ...inline(t.text)), 'Copy the details', () => all));
    }
    if (t.role === 'tool') return h('div', { class: `${cls} md-inline`, title: t.detail ?? '' }, icon(Icons.wrench), ' ', ...inline(t.text));
    return h('div', { class: cls }, t.text);
  }

  function renderResult(rec: RunRecord) {
    root.classList.remove('live');
    const rc = w.doc.p.run_configs.find((c) => c.id === rec.run_config);
    const failed = rec.status === 'failed';
    const head = h('div', { class: 'result-head' },
      h('div', { class: 'row' }, h('strong', { class: `status ${rec.status}` }, STATUS_LABEL[rec.status] ?? rec.status), rec.kind === 'dry_run' ? h('span', { class: 'pill' }, 'Dry run') : rec.kind === 'audio_review' ? h('span', { class: 'pill' }, 'Audio Review') : null),
      h('div', { class: 'muted small' }, [rc?.name ?? rec.run_config, when(rec.started), rec.provider === 'codex' ? 'Codex' : 'Claude Code', rec.model ? modelName(rec.model) : null, rec.cost_usd != null ? usd(rec.cost_usd) : null, rec.eleven_credits ? `${rec.eleven_credits} credits` : null].filter(Boolean).join(' · ')),
      results?.run?.message ? copyable(h('div', { class: failed ? 'danger small' : 'small' }, results.run.message), 'Copy the message', () => results?.run?.message ?? '') : null,
      results?.blocking?.length ? blockingList(results.blocking) : null,
      // An Audio Review's warnings (a silent timeline among them) come before the player.
      rec.kind === 'audio_review' && Array.isArray(results?.run?.warnings) && results.run.warnings.length ? h('ul', { class: 'warnings small' }, (results.run.warnings as string[]).map((x) => h('li', { class: 'warn' }, x))) : null,
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
    else if (sub.value === 'results') body = h('div', { class: 'outputs' }, results.outputs.length ? results.outputs.map((o) => (rec.kind === 'audio_review' ? audioReviewCard(o, rec.status === 'failed' || rec.status === 'cancelled') : outputCard(o))) : h('p', { class: 'muted' }, rec.kind === 'dry_run' ? 'A dry run doesn’t render outputs. See the summary.' : 'No outputs were produced.'), results.project_json5 ? button('View project.json5', () => viewText('project.json5', results!.project_json5!), { appearance: 'subtle' }) : null);
    else if (sub.value === 'summary') body = summary();
    else body = logs(rec);
    clear(
      root,
      head,
      tabs,
      body,
      h('div', { class: 'row end wrap' },
        button('Reveal Run Folder', () => request('shell.reveal', { path: rec.folder }), { glyph: Icons.openFolder }),
        rec.kind !== 'dry_run' ? button('Revise…', () => revise(rec), { glyph: Icons.revise, disabled: !rec.claude_session || !assistantAvailable() || w.runs.running }) : null,
        rec.kind !== 'dry_run' ? button('Re-run', () => w.runs.rerun(rec, auditioned()), { glyph: Icons.rerun, disabled: !assistantAvailable() || w.runs.running }) : null),
    );
  }

  /** The output an Audio Review auditioned, so a revision or a re-run auditions the same one. */
  function auditioned(): string | undefined {
    return results?.outputs[0]?.output_id;
  }

  /** What stopped a failed run: the missing audio source or the credit cap, named from its logs. */
  function blockingList(items: BlockingInput[]): HTMLElement {
    const title = (b: BlockingInput) => (b.kind === 'credit_cap' ? 'Over the credit cap' : b.kind === 'missing_source' ? 'Missing audio source' : b.kind);
    return h('div', { class: 'note danger blocking', role: 'group', 'aria-label': 'Blocked by' },
      h('strong', { class: 'small' }, 'Blocked by'),
      h('ul', { class: 'small' }, items.map((b) => h('li', null, copyable(h('span', null, h('strong', null, title(b)), ': ', b.detail), 'Copy this', () => `${title(b)}: ${b.detail}`)))));
  }

  /** An Audio Review's result (FEAT-neat-osprey-DKK7): the auditioned output, play/pause, a seek
   *  bar with the position and the duration. It plays the saved WAV; nothing is built again. */
  function audioReviewCard(o: OutputFile, failedRun: boolean): HTMLElement {
    const playable = o.exists && !o.incomplete && !failedRun;
    const head = h('div', { class: 'row' }, h('strong', null, o.output_name), h('span', { class: 'muted small' }, ` Soundtrack of this output${o.platform ? ` · ${o.platform}` : ''}`), o.exists && !playable ? h('span', { class: 'badge warn' }, 'Incomplete') : null);
    if (!playable) {
      return h('article', { class: 'output-card audio-review', 'aria-label': `Audio Review of ${o.output_name}` }, head,
        h('p', { class: 'danger' }, failedRun ? 'The soundtrack wasn’t completed, so there is nothing to play.' : 'The soundtrack wasn’t produced.'));
    }
    const audio = h('audio', { preload: 'metadata' }) as HTMLAudioElement;
    request<string>('shell.allow_media', { files: [o.path], add: true }).catch(() => {});
    request<string>('shell.media_url', { path: o.path }).then((u) => (audio.src = u)).catch(() => {});
    const clock = (t: number) => {
      const s = Math.max(0, Math.floor(t || 0));
      return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
    };
    let total = o.duration_s ?? 0;
    const play = iconButton(Icons.play, 'Play', () => (audio.paused ? audio.play().catch((e) => toast(errorText(e), 'error')) : audio.pause()));
    const pos = h('span', { class: 'muted small mono', 'aria-label': 'Position' }, clock(0));
    const dur = h('span', { class: 'muted small mono', 'aria-label': 'Duration' }, clock(total));
    const seek = h('input', { type: 'range', min: '0', max: String(total || 1), step: '0.05', value: '0', class: 'grow', 'aria-label': 'Seek' }) as HTMLInputElement;
    const sync = () => {
      const playing = !audio.paused;
      clear(play, icon(playing ? Icons.pause : Icons.play));
      play.setAttribute('aria-label', playing ? 'Pause' : 'Play');
      play.title = playing ? 'Pause' : 'Play';
      pos.textContent = clock(audio.currentTime);
      if (document.activeElement !== seek) seek.value = String(audio.currentTime);
      seek.setAttribute('aria-valuetext', `${clock(audio.currentTime)} of ${clock(total)}`);
    };
    seek.addEventListener('input', () => {
      audio.currentTime = Number(seek.value);
      pos.textContent = clock(audio.currentTime);
    });
    audio.addEventListener('loadedmetadata', () => {
      if (Number.isFinite(audio.duration) && audio.duration > 0) total = audio.duration;
      seek.max = String(total || 1);
      dur.textContent = clock(total);
    });
    for (const ev of ['play', 'pause', 'timeupdate', 'seeked']) audio.addEventListener(ev, sync);
    audio.addEventListener('ended', () => ((audio.currentTime = 0), sync()));
    return h('article', { class: 'output-card audio-review', 'aria-label': `Audio Review of ${o.output_name}` },
      head,
      audio,
      h('div', { class: 'row audio-controls' }, play, pos, seek, dur),
      h('div', { class: 'muted small' }, `${duration(total)} · ${bytes(o.bytes)} · ${fileName(o.path)}`),
      h('div', { class: 'row wrap' },
        button('Reveal in File Explorer', () => request('shell.reveal', { path: o.path })),
        button('Open', () => request('shell.open', { path: o.path })),
        button('Copy Path', () => copyText(o.path))));
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
        button('Export Logs…', () => exportLogs(rec.folder), { title: 'A zip of everything needed to study this run: logs, the instructions the Assistant was given, the spec and the edit. Media paths can be redacted.' }),
        button('Copy Issue Report', () => copyIssueReport(rec.folder))));
  }

  async function revise(rec: RunRecord) {
    const review = rec.kind === 'audio_review';
    const s = sheet(review ? 'Revise this Audio Review' : 'Revise this run');
    const box = h('textarea', { rows: 6, 'aria-label': 'What should change?', placeholder: 'For example: make the hook faster, use the logo at the end.' }) as HTMLTextAreaElement;
    s.body.append(h('p', { class: 'hint' }, review ? 'Say what to change in the soundtrack. The Assistant mixes it again, reusing audio that hasn’t changed, into a new review; this take stays playable to compare.' : 'The Assistant continues from this run and writes a new run folder linked to it.'), box);
    s.footer.append(button('Cancel', () => s.close()), button('Continue…', () => s.close(box.value.trim()), { appearance: 'primary' }));
    const feedback = await s.done;
    if (!feedback) return;
    w.doc.runConfig.value = rec.run_config;
    w.runs.requestRun({ previous_run: rec.id, feedback }, { audioReview: review, audioOutput: auditioned() });
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
      "The zip holds this run's logs, the instructions the Assistant was given, the spec and the edit. No media is included. Replace media paths, media file names and your home folder with placeholders?",
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
