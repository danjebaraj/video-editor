// Inspector › Validation (spec §11): the report with blockers, warnings and notes, fix links,
// acknowledgements, estimates, and the dry run.

import { h, clear, button, icon, Icons, usd, bytes, duration, spinner, emptyState, checkbox } from '../../dom';
import { effect } from '../../store';
import type { ProjectWindow } from '../window';
import type { Issue } from '../../types';

const SEV: Record<string, [string, string, string]> = {
  blocker: [Icons.error, 'danger', 'Blocker'],
  warning: [Icons.warning, 'warn', 'Warning'],
  info: [Icons.info, 'muted', 'Note'],
};

const FIX_LABEL: Record<string, string> = {
  spec: 'Line',
  media: 'Media',
  brand: 'Brand',
  outputs: 'Outputs',
  run_config: 'Outputs',
  project: 'Project',
  settings: 'Settings',
};

export function validationTab(w: ProjectWindow): HTMLElement {
  const root = h('div', { class: 'validation-tab' });
  const d = w.doc;
  effect([d.report, d.validating, w.runs.live], () => {
    const r = d.report.value;
    const head = h(
      'div',
      { class: 'tab-header' },
      r ? h('span', { class: 'counts' }, h('span', { class: 'danger' }, `${r.blockers} blocker(s)`), ' · ', h('span', { class: 'warn' }, `${r.warnings} warning(s)`), ' · ', h('span', { class: 'muted' }, `${r.infos} note(s)`)) : h('span', { class: 'muted' }, 'Not validated yet'),
      h('span', { class: 'grow' }),
      d.validating.value ? spinner('Validating') : null,
      button('Validate', () => d.validate(), { glyph: Icons.validate, disabled: d.validating.value || w.runs.running }),
      button('Dry Run…', () => w.runs.requestDryRun(), { appearance: 'subtle', disabled: d.validating.value || w.runs.running }),
    );
    if (!r) {
      clear(root, head, emptyState('Validate before you run', 'Checks the environment, media, spec, brand, outputs and budget. Nothing is sent to Claude.', button('Validate', () => d.validate(), { appearance: 'primary' })));
      return;
    }
    const est = r.estimates;
    const estimates = h(
      'dl',
      { class: 'estimates' },
      h('dt', null, 'Claude (approximate)'), h('dd', null, `${usd(est.claude_usd_low)} – ${usd(est.claude_usd_high)}`),
      h('dt', null, 'ElevenLabs credits'), h('dd', null, `${est.eleven_credits} of ${est.eleven_cap}${est.eleven_remaining != null ? ` (${est.eleven_remaining} left)` : ''}`),
      h('dt', null, 'Disk'), h('dd', null, `${bytes(est.disk_bytes)} needed${est.free_bytes != null ? `, ${bytes(est.free_bytes)} free` : ''}`),
      h('dt', null, 'Planned length'), h('dd', null, `${duration(est.duration_s)} · ${est.voice_over_words} voice-over words`),
    );
    const groups = (['blocker', 'warning', 'info'] as const).map((sev) => r.issues.filter((i) => i.severity === sev));
    clear(
      root,
      head,
      r.issues.length === 0 ? h('div', { class: 'ready', role: 'status' }, icon(Icons.checkCircle, 'ok'), ' Ready to run') : null,
      r.dry_run && (r.dry_run.ambiguities.length || r.dry_run.missing_media.length || r.dry_run.risks.length)
        ? button('Ask Claude to fix the spec', () => w.assistant.askToFix(r.dry_run), { appearance: 'primary', glyph: Icons.chat })
        : null,
      h('ul', { class: 'issues', 'aria-label': 'Validation issues' }, groups.flat().map((i) => issueRow(i))),
      estimates,
    );
  });

  function issueRow(i: Issue): HTMLElement {
    const [glyph, cls, label] = SEV[i.severity] ?? SEV.info;
    const fix = i.fix;
    return h(
      'li',
      { class: `issue ${i.severity}` },
      h('span', { class: cls, title: label, 'aria-label': label, role: 'img' }, icon(glyph)),
      h(
        'div',
        { class: 'grow' },
        h('div', { class: 'title' }, i.title),
        i.detail ? h('div', { class: 'muted small' }, i.detail) : null,
        h(
          'div',
          { class: 'row small' },
          fix ? h('button', { class: 'link', onclick: () => w.revealFix(fix) }, `${FIX_LABEL[fix.kind] ?? 'Fix'}${fix.kind === 'spec' && fix.line ? ` ${fix.line}` : ''} →`) : null,
          i.severity === 'warning' ? checkbox('Acknowledge', d.acknowledged.has(i.id), (v) => (v ? d.acknowledged.add(i.id) : d.acknowledged.delete(i.id))) : null,
        ),
      ),
    );
  }
  return root;
}
