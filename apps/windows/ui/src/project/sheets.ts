// Project sheets: Relink (spec §6.3), the run confirmation (§12.1) and the dry run findings (§11.2).

import { h, clear, button, icon, Icons, usd } from '../dom';
import { effect } from '../store';
import { sheet, openFiles, openFolders, Filters, toast } from '../dialogs';
import { modelName } from '../app-state';
import { modelPicker } from '../widgets';
import { copyButton, copyable } from '../text-tools';
import type { ProjectWindow } from './window';
import type { RunPlan } from './runs';
import type { Finding, Readiness } from '../types';

let relinkOpen = false;

export function showRelink(w: ProjectWindow) {
  if (relinkOpen) return;
  relinkOpen = true;
  const s = sheet('Relink media', { wide: true });
  s.el.dataset.sheet = 'relink';
  const status = h('p', { class: 'hint', role: 'status' });
  const list = h('ul', { class: 'relink-list', 'aria-label': 'Missing media' });
  const off = effect([w.doc.resolutions], () => {
    const missing = w.doc.missing();
    clear(
      list,
      missing.length
        ? missing.map((r) => h('li', { class: 'row' }, icon(Icons.missing, 'danger'), h('div', { class: 'grow' }, h('strong', null, r.id), h('div', { class: 'path small muted', title: r.stored_path ?? '' }, r.stored_path ?? '')), button('Locate…', () => w.locate(r.id))))
        : h('li', { class: 'ok' }, icon(Icons.checkCircle), ' Every media file is found.'),
    );
    done.textContent = missing.length ? 'Later' : 'Done';
  });
  const done = button('Later', () => s.close(), { appearance: 'primary' });
  s.body.append(h('p', null, 'These files aren’t where the project last saw them. Locate one and the app offers to fix the others with the same folder change, or search a folder for all of them by name and content.'), list, status);
  s.footer.append(
    button('Search Folder…', async () => {
      const [dir] = await openFolders('Search a folder for the missing media');
      if (!dir) return;
      const relinked = await w.doc.searchFolder(dir);
      status.textContent = relinked.length ? `Relinked ${relinked.length} item(s) from ${dir}.` : `Nothing in ${dir} matched.`;
    }, { glyph: Icons.search }),
    done,
  );
  s.done.then(() => {
    off();
    relinkOpen = false;
  });
}

/** Locate… for one missing item: the file picker, then the prefix fix offer. */
export async function locate(w: ProjectWindow, id: string) {
  const [file] = await openFiles(`Locate ${id}`, Filters.media, false, w.doc.projectDir());
  if (!file) return;
  try {
    await w.doc.relink(id, file);
  } catch (e) {
    toast(String(e), 'error');
  }
}

/** “Run …” confirmation: outputs, model, quality, estimate, folder, dry run first. */
export function showRunConfirm(w: ProjectWindow, plan: RunPlan) {
  const rc = w.doc.p.run_configs.find((r) => r.id === plan.runConfig)!;
  const s = sheet(plan.revise ? `Revise with “${rc.name}”` : `Run “${rc.name}”`, { wide: true });
  s.el.dataset.sheet = 'run-confirm';
  const warnings = plan.report.issues.filter((i) => i.severity === 'warning');
  const est = plan.report.estimates;
  const dryToggle = h('input', { type: 'checkbox', checked: plan.dryRunFirst, id: 'dry-first' }) as HTMLInputElement;
  dryToggle.addEventListener('change', () => (plan.dryRunFirst = dryToggle.checked));
  clear(s.body,
    h('dl', { class: 'summary-grid' },
      h('dt', null, 'Outputs'), h('dd', null, plan.outputs.map((o) => `${o.name} ${o.width}×${o.height}`).join(', ') || '—'),
      h('dt', null, 'Model'), h('dd', null, modelPicker(plan.model, (v) => (plan.model = v), { label: 'Production model for this run' })),
      h('dt', null, 'Quality'), h('dd', null, rc.quality === 'draft' ? 'Draft' : 'Final'),
      h('dt', null, 'Estimated cost'), h('dd', null, `${usd(est.claude_usd_low)} – ${usd(est.claude_usd_high)} (approximate)${est.eleven_credits ? ` · ${est.eleven_credits} ElevenLabs credits` : ''}`),
      h('dt', null, 'Output folder'), h('dd', { class: 'path small' }, plan.outputFolder),
    ),
    plan.revise ? h('blockquote', null, plan.revise.feedback) : h('label', { class: 'check' }, dryToggle, h('span', null, `Do a dry run first with ${modelName(w.doc.dryRunModel)}`)),
    warnings.length ? h('div', { class: 'note warn' }, h('strong', null, `${warnings.length} warning(s) will be accepted for this run`), h('ul', { class: 'small' }, warnings.slice(0, 5).map((i) => h('li', null, i.title)))) : null,
  );
  s.footer.append(
    button('Show Validation', () => {
      s.close();
      w.showTab('validation');
    }, { appearance: 'subtle' }),
    button('Cancel', () => s.close()),
    button(plan.dryRunFirst && !plan.revise ? 'Start Dry Run' : 'Run', () => {
      s.close();
      w.runs.confirm(plan);
    }, { appearance: 'primary', glyph: Icons.play }),
  );
  dryToggle.addEventListener('change', () => {
    const b = s.footer.lastElementChild as HTMLElement;
    b.lastChild!.textContent = dryToggle.checked ? 'Start Dry Run' : 'Run';
  });
  s.done.then(() => {
    if (w.runs.plan.value === plan) w.runs.plan.value = null;
  });
}

export function findingText(f: Finding): string {
  let s = f.text;
  if (f.spec_line) s += ` (spec line ${f.spec_line})`;
  if (f.detail) s += `\n${f.detail}`;
  return s;
}

/** The findings as Markdown, section by section, ready to paste into a note or an issue. */
export function findingsMarkdown(r: Readiness): string {
  const sections: [string, Finding[]][] = [
    ['Unclear in the spec', r.ambiguities], ['Missing media', r.missing_media], ['Risks', r.risks],
    ['Assumptions the dry run made', r.assumptions], ['Made during the production run', r.planned ?? []],
  ];
  const out = sections.filter(([, items]) => items.length).map(([title, items]) => `## ${title}\n\n${items.map((f) => `- ${findingText(f).replace(/\n/g, '\n  ')}`).join('\n')}`);
  if (r.notes) out.push(`## Notes\n\n${r.notes}`);
  return `${out.join('\n\n')}\n`;
}

/** After a dry run with findings: fix the spec, stop, or run anyway. */
export function showFindings(w: ProjectWindow, r: Readiness) {
  const s = sheet('The dry run found things to check', { wide: true });
  s.el.dataset.sheet = 'findings';
  const section = (title: string, list: Finding[]) =>
    list.length
      ? h('section', null, h('h3', null, title), h('ul', null, list.map((f) => h('li', { class: 'finding' }, h('div', { class: 'grow' }, f.text, f.detail ? h('div', { class: 'muted small' }, f.detail) : null), copyButton('Copy this finding', () => findingText(f)), f.spec_line ? h('button', { class: 'link small', onclick: () => {
          s.close();
          w.jumpToLine(f.spec_line!);
        } }, `Line ${f.spec_line}`) : null))))
      : null;
  const notes = r.notes;
  clear(s.body, section('Unclear in the spec', r.ambiguities), section('Missing media', r.missing_media), section('Risks', r.risks), section('Assumptions the dry run made', r.assumptions), section('Made during the production run (nothing to do)', r.planned ?? []), notes ? copyable(h('p', { class: 'muted' }, notes), 'Copy the notes', () => notes) : null);
  s.footer.append(
    button('Ask Claude to Fix the Spec', () => {
      s.close('fix');
      w.runs.findings.value = null;
      w.runs.pendingPlan = null;
      w.assistant.askToFix(r);
    }),
    copyButton('Copy every finding as Markdown', () => findingsMarkdown(r), { title: 'Copy All' }),
    button('Stop Here', () => {
      s.close('stop');
      w.runs.findings.value = null;
      w.runs.pendingPlan = null;
    }),
    button('Run Anyway', () => {
      s.close('run');
      w.runs.continueAfterDryRun();
    }, { appearance: 'primary' }),
  );
}
