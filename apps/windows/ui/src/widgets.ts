// Widgets shared by several views: the model picker, the effort picker and Claude's question card.

import { h, button, select } from './dom';
import { models, environment, modelName } from './app-state';
import { prompt } from './dialogs';
import type { ClaudeQuestion } from './types';

/** Family › Latest (the CLI alias) or a version (its full ID), plus a custom ID (spec §4.2). */
export function modelPicker(value: string | undefined | null, onChange: (v: string) => void, opts: { label: string; allowDefault?: boolean; settingsDefault?: string } = { label: 'Model' }): HTMLElement {
  const options: [string, string][] = [];
  if (opts.settingsDefault !== undefined) {
    // an empty model follows this one, including later changes to it
    options.push(['', `Settings default (${modelName(opts.settingsDefault)})`]);
    value = (value ?? '').trim();
  } else if (opts.allowDefault) options.push(['', 'Default']);
  let known = false;
  for (const f of models.value?.families ?? []) {
    options.push([f.alias, `${f.name} — latest`]);
    if (f.alias === value) known = true;
    for (const v of f.versions) {
      options.push([v.id, `${f.name} — ${v.name}`]);
      if (v.id === value) known = true;
    }
  }
  if (value && !known) options.push([value, `Custom: ${value}`]);
  options.push(['__custom__', 'Custom ID…']);
  const el = select(options, value ?? '', async (v) => {
    if (v === '__custom__') {
      const id = await prompt('Custom model ID', 'Model ID or alias', value ?? '');
      el.value = value ?? '';
      if (id) onChange(id);
      return;
    }
    onChange(v);
  }, { 'aria-label': opts.label });
  return el;
}

/** Shown only when the installed CLI lists `--effort` levels. */
export function effortPicker(value: string | undefined | null, onChange: (v: string | null) => void, label = 'Effort'): HTMLElement | null {
  const levels = environment.value?.claude?.effort_levels ?? [];
  if (!levels.length) return null;
  return select([['', 'Default'], ...levels.map((l) => [l, l] as [string, string])], value ?? '', (v) => onChange(v || null), { 'aria-label': label });
}

/**
 * Claude's questions (AskUserQuestion). Answers are keyed by question text; each is the chosen
 * labels plus any free text, joined with ", ".
 */
export function questionCard(questions: ClaudeQuestion[], onAnswer: (answers: Record<string, string>) => void, onSkip?: () => void): HTMLElement {
  const answers: Record<string, { picked: Set<string>; text: string }> = {};
  const answerBtn = button('Answer', () => {
    const out: Record<string, string> = {};
    for (const q of questions) {
      const a = answers[q.question];
      out[q.question] = [...a.picked, a.text.trim()].filter(Boolean).join(', ');
    }
    onAnswer(out);
  }, { appearance: 'primary' });
  const refresh = () => {
    const complete = questions.every((q) => {
      const a = answers[q.question];
      return a.picked.size > 0 || a.text.trim() !== '';
    });
    if (complete) answerBtn.removeAttribute('disabled');
    else answerBtn.setAttribute('disabled', '');
  };
  const card = h('section', { class: 'question-card', role: 'group', 'aria-label': 'Claude has a question' });
  questions.forEach((q, qi) => {
    answers[q.question] = { picked: new Set(), text: '' };
    const multi = !!(q.multiSelect ?? q.multi_select);
    const name = `q${qi}-${Math.random().toString(36).slice(2, 7)}`;
    const opts = q.options.map((o) => {
      const input = h('input', { type: multi ? 'checkbox' : 'radio', name, value: o.label }) as HTMLInputElement;
      input.addEventListener('change', () => {
        const a = answers[q.question];
        if (!multi) a.picked.clear();
        if (input.checked) a.picked.add(o.label);
        else a.picked.delete(o.label);
        refresh();
      });
      return h('label', { class: 'option' }, input, h('span', null, h('strong', null, o.label), o.description ? h('span', { class: 'muted' }, ` — ${o.description}`) : null));
    });
    const free = h('input', { type: 'text', placeholder: q.options.length ? 'Something else…' : 'Your answer', 'aria-label': `Answer: ${q.question}` }) as HTMLInputElement;
    free.addEventListener('input', () => {
      answers[q.question].text = free.value;
      refresh();
    });
    card.append(
      h('div', { class: 'question' }, q.header ? h('div', { class: 'eyebrow' }, q.header) : null, h('p', { class: 'question-text' }, q.question), h('div', { class: 'options', role: multi ? 'group' : 'radiogroup', 'aria-label': q.question }, opts), free),
    );
  });
  refresh();
  card.append(h('div', { class: 'row end' }, onSkip ? button('Skip', onSkip) : null, answerBtn));
  return card;
}
