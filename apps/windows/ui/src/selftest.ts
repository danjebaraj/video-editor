// The in-app self test, UI half (ui/spec.md §23.9 item 3; the Windows counterpart of the macOS
// SelfTest.swift). The shell prepares a scratch library, the fake Claude, the real cutline and
// `cutline selftest --quick` footage, opens the project and sends `start` on the `selftest`
// topic; this drives the real window through every step, reports each with `selftest.report`,
// saves a snapshot after each, and reports `done`.

import { core, request, on, errorText } from './bridge';
import { scriptedAnswers } from './dialogs';
import { environment, keyStatus, detect, brands, reloadLibrary } from './app-state';
import type { ProjectWindow } from './project/window';

interface Fixture {
  out: string;
  library: string;
  project: string;
  footage: string;
  shared: string;
  output_folder: string;
  shots: string;
  trace: string;
  assistant_script: string;
  run_script: string;
  hang_script: string;
  spec: string;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until(what: () => boolean | Promise<boolean>, ms: number, label: string): Promise<void> {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await what()) return;
    await sleep(100);
  }
  throw new Error(`timed out after ${ms / 1000} s waiting for ${label}`);
}

function check(ok: unknown, what: string) {
  if (!ok) throw new Error(what);
}

export function attach(w: ProjectWindow) {
  on('self_test', (m) => {
    if (m.step === 'start') run(w, m.args as Fixture);
  });
}

async function run(w: ProjectWindow, fx: Fixture) {
  let n = 0;
  const shot = async (name: string) => {
    n++;
    await sleep(400);
    await request('shell.snapshot', { path: `${fx.shots}\\${String(n).padStart(2, '0')}-${name}.png` }).catch(() => {});
  };
  const step = async (name: string, body: () => Promise<string | void>) => {
    try {
      const detail = await body();
      await request('selftest.report', { step: name, ok: true, detail: detail ?? '' });
    } catch (e) {
      await request('selftest.report', { step: name, ok: false, detail: errorText(e) });
    }
    await shot(name);
  };
  const d = w.doc;
  const trace = async (): Promise<any[]> => {
    const url = await request<string>('shell.allow_media', { files: [fx.trace], add: true }).then(() => request<string>('shell.media_url', { path: fx.trace }));
    const text = await fetch(url).then((r) => (r.ok ? r.text() : ''));
    return text.split('\n').filter(Boolean).map((l) => JSON.parse(l));
  };

  await step('environment', async () => {
    await detect(true);
    const env = environment.value!;
    check(env.claude?.supported, 'the fake Claude is detected and supported');
    check(env.claude?.effort_levels.join(',') === 'low,medium,high', `effort levels: ${env.claude?.effort_levels}`);
    check(env.cutline && env.doctor?.ok, `cutline found and doctor passes (${JSON.stringify(env.doctor?.missing_required)})`);
    check(keyStatus.value?.masked === '••••-key', `the key is masked: ${keyStatus.value?.masked}`);
    const lib = await core<string>('library.dir');
    check(lib.toLowerCase().startsWith(fx.out.toLowerCase()), `the library is in the scratch folder: ${lib}`);
    return `${env.claude!.version_text}; cutline ${env.cutline!.version}`;
  });

  await step('new project', async () => {
    check(d.p.spec.startsWith('# Launch teaser: marketing video'), 'the marketing template');
    check(d.p.output_configs.length === 3 && d.p.run_configs.length === 1, `3 outputs and 1 run config (${d.p.output_configs.length}, ${d.p.run_configs.length})`);
    check(d.outline.value.some((o) => o.kind === 'scene'), 'the outline has a scene');
    check(d.lint.value.some((l) => l.rule === 'placeholder'), 'lint finds the template placeholders');
  });

  await step('editing', async () => {
    w.editor.setText(fx.spec, true);
    check(d.dirty.value, 'the project is dirty');
    await until(() => !d.dirty.value, 4000, 'autosave');
    const disk = await core('project.load', { path: d.path.value });
    check(disk.spec.includes('## Scene 2: Product'), 'autosave wrote the spec');
    await until(() => d.lint.value.filter((l) => l.rule === 'unresolved_media').length === 4, 3000, '4 unresolved media references');
    w.editor.goToLine(5);
    check(w.editor.cursorLine() === 5, 'go to line');
    const before = w.editor.text;
    w.editor.goToLine(w.editor.view.state.doc.lines);
    w.editor.insertBlock('scene');
    check(w.editor.text.includes('## Scene'), 'the scene block is inserted');
    const sel = w.editor.selection();
    check(sel && sel.text.length > 0, 'its first placeholder is selected');
    w.editor.undo();
    check(w.editor.text === before, 'one undo step removes it');
  });

  await step('media', async () => {
    const ids = await d.addMedia([fx.footage]);
    for (const id of ['m-a', 'm-b', 'm-logo', 'm-music']) check(ids.includes(id), `${id} added (${ids.join(', ')})`);
    check(d.p.media.find((m) => m.id === 'm-a')?.paths.file_id_windows, 'a Windows file ID is stored');
    await until(() => d.resolution('m-a')?.status === 'ok', 5000, 'm-a resolves');
    await until(() => (d.p.media.find((m) => m.id === 'm-a')?.probe?.duration_s ?? 0) > 1, 30000, 'm-a is probed');
    check(d.p.media.find((m) => m.id === 'm-music')?.role === 'music', 'music.wav gets the music role');
    check(d.p.media.find((m) => m.id === 'm-logo')?.role === 'logo', 'logo.png gets the logo role');
    await until(() => d.lint.value.filter((l) => l.rule === 'unresolved_media').length === 1, 3000, 'one unresolved reference left');
    // a pasted image lands in the sidecar, named by its hash, stored once
    const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
    const id1 = await d.addPastedImage(png);
    const id2 = await d.addPastedImage(png);
    check(id1 && id1 === id2, `the same image is stored once (${id1}, ${id2})`);
    const item = d.p.media.find((m) => m.id === id1)!;
    const name = (item.paths.absolute ?? '').split('\\').pop()!;
    check(item.pasted && name.length === 68 && name.endsWith('.png'), `sidecar name ${name}`);
    check((item.paths.absolute ?? '').toLowerCase().includes('.cutproj.assets'), 'in the .assets sidecar');
    return `${d.p.media.length} media items`;
  });

  await step('preview', async () => {
    w.previewOn.value = true;
    const r0 = w.preview.rendered;
    await until(() => w.preview.rendered > r0, 15000, 'the preview renders');
    await until(() => !!w.preview.document()?.querySelector('pre.mermaid svg'), 15000, 'Mermaid renders offline');
    const doc = w.preview.document()!;
    const cards = doc.querySelectorAll('.media-card');
    check(cards.length === 4, `4 media cards (${cards.length})`);
    check(doc.querySelectorAll('.media-card.status-unknown').length === 1, 'one unknown media reference');
    check(doc.querySelector('.spec-vo .spec-block-body')?.textContent?.includes('fastest way to ship'), 'the voice-over block');
    check(doc.querySelectorAll('table tr').length === 2, 'the table');
    const img = doc.querySelector<HTMLImageElement>('.media-card img');
    await until(() => !!img && img.complete && img.naturalWidth > 0, 20000, 'a video thumbnail from the media host');
    w.previewOn.value = false;
  });

  await step('assistant', async () => {
    d.update((p) => (p.assistant_folders = [fx.shared]), { outputs: false });
    w.assistant.extraEnv = { FAKE_CLAUDE_SCRIPT: fx.assistant_script };
    const projectBefore = await core('project.load', { path: d.path.value });
    await w.assistant.send('Tighten the hook for developers.');
    await until(() => w.assistant.session().messages.some((m) => m.question?.state === 'pending'), 20000, 'Claude’s question');
    const q = w.assistant.session().messages.find((m) => m.question?.state === 'pending')!;
    await w.assistant.answer(q, { 'Who is the audience?': 'Developers' });
    await until(() => !!w.assistant.review.value, 20000, 'the review of Claude’s edits');
    await until(() => !w.assistant.running.value, 20000, 'the turn ends');
    const r = w.assistant.review.value!;
    check(r.hunks.length === 2, `two hunks (${r.hunks.length})`);
    check(w.editor.view.state.readOnly, 'the editor is read-only during review');
    check(w.editor.text === r.base, 'the editor still has the old text');
    w.assistant.decide(0, true);
    w.assistant.decide(1, false);
    await until(() => !w.assistant.review.value, 5000, 'the review finishes');
    check(w.editor.text.includes('for developers to ship') && !w.editor.text.includes('free trial today'), 'hunk 1 accepted, hunk 2 rejected');
    check(!w.editor.view.state.readOnly, 'the editor is editable again');
    const t = await trace();
    const denied = t.filter((x) => x.tool && x.decision === 'deny');
    check(denied.length === 2, `two denials: the project file and the shell (${denied.map((x) => x.tool).join(', ')})`);
    check(t.some((x) => x.tool === 'Read' && x.decision === 'allow' && String(x.input?.file_path).includes('brand-notes')), 'the shared folder is readable');
    check(t.some((x) => JSON.stringify(x).includes('Developers')), 'the answer reached Claude');
    const projectAfter = await core('project.load', { path: d.path.value });
    check(projectAfter.id === projectBefore.id, 'the project file wasn’t overwritten by Claude');
    await until(() => (d.p.assistant_sessions ?? []).length === 1, 5000, 'the session is saved in the project');
    const summary = await core<{ summary: string }>('project.summary', { project: d.p, path: d.path.value });
    check(summary.summary.includes('App recording: on'), 'the project summary tells Claude about app recording');
    const perms = await core<{ supported: boolean }>('capture.permissions', { request: false });
    check(perms.supported, 'app recording is supported on this machine');
  });

  await step('relink', async () => {
    await d.save();
    // renamed on the same volume: found again by its file ID, no relink sheet
    const moved = `${fx.footage}-renamed`;
    await request('selftest.report', { step: 'relink: rename footage', ok: await renameDir(fx.footage, moved), detail: moved });
    await d.resolveMedia(false);
    check(d.missing().length === 0, `nothing is missing after a rename (${d.missing().map((m) => m.id)})`);
    check(d.resolution('m-a')?.found_by === 'bookmark', `found by its file ID (${d.resolution('m-a')?.found_by})`);
    check(d.resolution('m-a')?.path?.includes('-renamed'), 'the path is updated');
    // copied elsewhere and the originals gone: missing, then Locate + prefix fix
    const work = `${fx.out}\\Volumes\\Work\\footage`;
    check(await copyDir(moved, work), 'copied the footage');
    check(await removeDir(moved), 'removed the originals');
    await d.resolveMedia(false);
    check(d.missing().length >= 4, `media is missing (${d.missing().length})`);
    check(d.resolution(d.p.media.find((m) => m.pasted)!.id)?.status !== 'missing', 'the pasted image still resolves');
    scriptedAnswers.push(0); // Apply Prefix Fix
    await d.relink('m-a', `${work}\\a.mp4`);
    check(d.missing().length === 0, `the prefix fix relinked the rest (${d.missing().map((m) => m.id)})`);
  });

  await step('brands', async () => {
    let acme = await core('brand.new', { name: 'Acme' });
    acme.colors.primary = '#ff6600';
    acme = await core('brand.save', { brand: acme });
    await core('brand.save', { brand: await core('brand.new', { name: 'Globex' }) });
    await reloadLibrary();
    const names = brands.value.map((b) => b.name).sort();
    check(names.includes('Acme') && names.includes('Globex'), `brands: ${names}`);
    d.setBrand(brands.value.find((b) => b.name === 'Acme')!);
    check(d.p.brand?.snapshot.colors.primary === '#ff6600', 'the snapshot is stored');
    await sleep(1100);
    acme.colors.primary = '#0066ff';
    await core('brand.save', { brand: acme });
    check(await core<boolean>('brand.is_newer', { brand: d.p.brand }), 'the library copy is newer');
    await reloadLibrary();
    d.updateBrandToLatest();
    check(d.p.brand?.snapshot.colors.primary === '#0066ff', 'Update to Latest');
    await core('brand.export', { brand: acme, destination: `${fx.out}\\acme.cutbrand` });
    await request('shell.open_window', { kind: 'library', options: { tab: 'brands' } });
    await sleep(1500);
  });

  await step('validation', async () => {
    const rc = d.currentRunConfig()!;
    d.update((p) => {
      p.output_configs = [{ id: 'out_shorts', preset: 'youtube_shorts', overrides: {} } as any];
      const r = p.run_configs.find((x) => x.id === rc.id)!;
      r.outputs = ['out_shorts'];
      r.quality = 'draft';
      r.dry_run_first = false;
      r.output_folder = fx.output_folder;
      p.intent.target_length_s = 6;
    });
    const report = await d.validate();
    const issue = report?.issues.find((i) => i.rule === 'spec_unresolved_media');
    check(issue && issue.severity === 'blocker', 'an unresolved reference is a blocker');
    w.revealFix(issue!.fix!);
    check(w.editor.cursorLine() === issue!.fix!.line, `the fix link goes to line ${issue!.fix!.line}`);
    w.editor.setText(w.editor.text.replace(' ![later](media:m-not-yet)', ''), true);
    const again = await d.validate();
    check(again && again.blockers === 0, `no blockers (${again?.issues.filter((i) => i.severity === 'blocker').map((i) => i.title)})`);
    w.showTab('validation');
  });

  await step('run', async () => {
    w.runs.extraOptions = { extra_env: { FAKE_CLAUDE_SCRIPT: fx.run_script } };
    await w.runs.requestRun();
    const plan = w.runs.plan.value;
    check(plan, 'the confirmation sheet is shown');
    check(plan!.outputs.length === 1 && plan!.outputFolder.toLowerCase().startsWith(fx.output_folder.toLowerCase()), `the plan: ${plan!.outputs.length} output(s) to ${plan!.outputFolder}`);
    check(plan!.report.estimates.claude_usd_high > 0, 'a cost estimate');
    document.querySelector<HTMLDialogElement>('dialog[data-sheet="run-confirm"]')?.close();
    await w.runs.confirm(plan!);
    await until(() => !!w.runs.live.value?.question, 60000, 'the run’s question');
    const live = w.runs.live.value!;
    check(live.stages.find((s) => s.id === 'authoring')?.state === 'done', 'authoring is done');
    await shot('run-question');
    await w.runs.answer({ 'Render the draft now?': 'Yes' });
    await until(() => !w.runs.live.value, 240000, 'the run finishes');
    const rec = w.runs.lastRun()!;
    check(rec.status === 'succeeded' || rec.status === 'succeeded_with_warnings', `status ${rec.status}`);
    const results = await core('run.results', { folder: rec.folder });
    const out = results.outputs[0];
    check(out?.exists && out.bytes > 10000, `the Shorts output exists (${out?.path}, ${out?.bytes} bytes)`);
    check(out.qa_verdict, 'a QA verdict');
    check(rec.cost_usd === 0.42, `the cost (${rec.cost_usd})`);
    const t = await trace();
    check(t.some((x) => x.tool === 'Bash' && x.input?.command === 'ls' && x.decision === 'deny'), '`ls` is denied');
    const names = results.logs.map((l: any) => l.name);
    for (const f of ['run.json', 'events.ndjson', 'claude.jsonl', 'validation.json', 'summary.md', 'permissions.ndjson', 'cutline/001-validate.log', 'cutline/002-render.log'])
      check(names.includes(f), `logs/${f} (${names.join(', ')})`);
    const report = await core<string>('logs.issue_report', { folder: rec.folder });
    check(report.includes('# cutline issue report') && report.includes('Blocked tool calls'), 'the issue report');
    const n = await core<number>('logs.export', { folder: rec.folder, destination: `${fx.out}\\logs.zip`, redact: true });
    check(n > 8, `logs exported (${n} files)`);
    w.showTab('run');
    return `${rec.status}, ${out.bytes} bytes`;
  });

  await step('stop', async () => {
    w.runs.extraOptions = { extra_env: { FAKE_CLAUDE_SCRIPT: fx.hang_script } };
    const report = (await d.validate())!;
    await w.runs.launch('production', d.currentRunConfig()!.production_model, report);
    await until(() => (w.runs.live.value?.transcript.length ?? 0) > 0, 30000, 'the run starts');
    const t0 = Date.now();
    await w.runs.stop();
    await until(() => !w.runs.live.value, 15000, 'the run stops');
    const secs = (Date.now() - t0) / 1000;
    const rec = w.runs.lastRun()!;
    check(rec.status === 'cancelled', `cancelled (${rec.status})`);
    check(secs < 8, `stopped in ${secs.toFixed(1)} s`);
    const results = await core('run.results', { folder: rec.folder });
    check(results.logs.some((l: any) => l.name === 'summary.md'), 'logs/summary.md exists');
    return `${secs.toFixed(1)} s`;
  });

  await step('windows', async () => {
    w.showTab('outputs');
    await shot('outputs-tab');
    w.showTab('project');
    await shot('project-tab');
    await request('shell.open_window', { kind: 'settings', options: { pane: 'claude' } });
    await sleep(1500);
  });

  await request('selftest.report', { step: 'done', ok: true, detail: '' });
}

// Moving the scratch footage around (self test only; the shell refuses outside the scratch folder).
async function renameDir(from: string, to: string): Promise<boolean> {
  return request<boolean>('selftest.fs', { op: 'rename', from, to }).catch(() => false);
}
async function copyDir(from: string, to: string): Promise<boolean> {
  return request<boolean>('selftest.fs', { op: 'copy', from, to }).catch(() => false);
}
async function removeDir(path: string): Promise<boolean> {
  return request<boolean>('selftest.fs', { op: 'remove', from: path }).catch(() => false);
}
