// Inspector › Assistant (spec §4.3): sessions, the transcript with tool calls and questions, the
// per-hunk review of the Assistant's edits, and the composer (Ctrl+Enter sends).

import { h, clear, button, iconButton, icon, Icons, checkbox, emptyState, select } from '../../dom';
import { request } from '../../bridge';
import { effect } from '../../store';
import { environment, assistantAvailable, assistantChecked, assistantSetupProblem, readiness, modelName, settings } from '../../app-state';
import { prompt, confirm, openFolders } from '../../dialogs';
import { questionCard } from '../../widgets';
import { copyButton, copyable, inline, markdownText } from '../../text-tools';
import type { ProjectWindow } from '../window';
import type { ChatMessage } from '../assistant';

const STARTERS: [string, string][] = [
  ['Tighten the script to the target length', 'Tighten the voice-over and scenes so the video fits the target length. Keep the key message and the call to action.'],
  ['Check the spec against the attached media', 'Compare the spec with the attached media. Point out scenes with no suitable media, media that isn’t used, and anything the footage can’t support. Then fix what you can in the spec.'],
];

export function claudeTab(w: ProjectWindow): HTMLElement {
  const a = w.assistant;
  const header = h('div', { class: 'tab-header' });
  const transcript = h('div', { class: 'transcript', role: 'log', 'aria-live': 'polite', 'aria-label': 'Conversation with the Assistant' });
  const composerNote = h('div', { class: 'note warn small' });
  const box = h('textarea', { class: 'composer', rows: 4, 'aria-label': 'Message to the Assistant', placeholder: 'Ask the Assistant to improve the spec…  Ctrl+Enter to send' }) as HTMLTextAreaElement;
  let includeSelection = true;
  const selectionToggle = h('div', { class: 'small' });
  const sendBtn = button('Send', () => send(), { appearance: 'primary', glyph: Icons.send });
  const stopBtn = button('Stop', () => a.stop(), { glyph: Icons.stop });
  const actions = h('div', { class: 'row wrap small' });
  const root = h('div', { class: 'claude-tab' }, header, transcript, h('div', { class: 'composer-area' }, composerNote, selectionToggle, box, h('div', { class: 'row end' }, stopBtn, sendBtn), actions));

  const send = () => {
    const text = box.value.trim();
    if (!text) return;
    const sel = includeSelection && !w.previewOn.value ? w.editor.selection() : null;
    box.value = '';
    a.draft.value = '';
    a.send(text, { selection: sel ?? undefined });
  };
  box.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && e.ctrlKey) {
      e.preventDefault();
      send();
    }
  });
  box.addEventListener('input', () => (a.draft.value = box.value));
  a.draft.subscribe((v) => {
    if (box.value !== v) box.value = v;
  });

  const modelLabel = h('span', { class: 'muted small', title: 'The spec assistant model. Change it in the Project inspector.' });
  effect([w.doc.project, settings], () => {
    const name = modelName(w.doc.assistantModel);
    if (modelLabel.textContent !== name) modelLabel.textContent = name;
  });

  effect([a.sessions, a.current, a.running, environment, settings, readiness], () => {
    const sessions = [...a.sessions.value].reverse();
    clear(
      header,
      select(sessions.map((s) => [s.id, s.name || 'Untitled session']), a.current.value, (v) => (a.current.value = v), { 'aria-label': 'Session', class: 'grow' }),
      iconButton(Icons.add, 'New session', () => {
        a.newSession();
        box.focus();
      }),
      iconButton(Icons.edit, 'Rename session', async () => {
        const s = a.session();
        const name = await prompt('Rename session', 'Name', s.name);
        if (name) a.rename(s.id, name);
      }),
      iconButton(Icons.delete, 'Delete session', async () => {
        const c = await confirm('Delete this session?', 'Its transcript is removed. The spec isn’t changed.', ['Delete', 'Cancel'], true);
        if (c === 0) a.remove(a.current.value);
      }, { disabled: a.running.value }),
      modelLabel,
    );
    renderTranscript();
    sendBtn.toggleAttribute('disabled', a.running.value || !assistantAvailable());
    stopBtn.hidden = !a.running.value;
  });

  effect([a.review], () => {
    composerNote.textContent = a.review.value ? 'Review the Assistant’s changes in the editor to continue.' : '';
    composerNote.hidden = !a.review.value;
  });

  effect([w.doc.project, w.selectionLines, w.previewOn, w.doc.report, w.doc.lint, a.includeValidation], () => {
    const sel = w.selectionLines.value;
    const n = a.validationFindingCount;
    clear(
      selectionToggle,
      sel && !w.previewOn.value ? checkbox(`Selection, lines ${sel[0]}–${sel[1]}`, includeSelection, (v) => (includeSelection = v)) : null,
      n > 0
        ? checkbox(`Validation findings (${n})`, a.includeValidation.value, (v) => (a.includeValidation.value = v), { title: 'Send what validation found with your message, so the Assistant can fix it. Findings outside the spec are marked as such.' })
        : null,
    );
    const folders = w.doc.p.assistant_folders ?? [];
    clear(
      actions,
      button('Gather missing info', () => a.gatherInfo(), { appearance: 'subtle', glyph: Icons.question }),
      button(`Folders (${folders.length})…`, async () => {
        const c = await confirm('Folders the Assistant may read', folders.length ? folders.join('\n') : 'No folders yet.', ['Add Folder…', 'Remove All', 'Close']);
        if (c === 0) {
          const dirs = await openFolders('Folders the Assistant may read', true);
          if (dirs.length) w.doc.update((p) => (p.assistant_folders = [...new Set([...(p.assistant_folders ?? []), ...dirs])]), { outputs: false });
        } else if (c === 1) w.doc.update((p) => (p.assistant_folders = []), { outputs: false });
      }, { appearance: 'subtle', glyph: Icons.folder }),
      checkbox('Web', !!w.doc.p.models.web_access, (v) => w.doc.update((p) => (p.models.web_access = v), { outputs: false })),
    );
  });

  function renderTranscript() {
    if (assistantChecked() && !assistantAvailable()) {
      // The provider selected in Settings › Assistant, named with the executable looked for.
      const p = assistantSetupProblem();
      clear(transcript, emptyState(p.title, p.detail, button('Open Settings…', () => request('shell.open_window', { kind: 'settings', options: { pane: p.pane } }))));
      return;
    }
    const msgs = a.session()?.messages ?? [];
    if (!msgs.length) {
      clear(
        transcript,
        h('div', { class: 'starters' }, h('p', { class: 'muted' }, 'Ask the Assistant to improve the spec. Its edits come back as changes you accept or reject.'), button('Gather missing info', () => a.gatherInfo()), STARTERS.map(([label, text]) => button(label, () => a.send(text)))),
      );
      return;
    }
    clear(transcript, msgs.map((m) => message(m)));
    transcript.scrollTop = transcript.scrollHeight;
  }

  function message(m: ChatMessage): HTMLElement {
    switch (m.role) {
      case 'user':
        return h('div', { class: 'msg user' }, h('div', { class: 'row top' }, copyButton('Copy your message', () => m.text), h('div', { class: 'bubble' }, m.text)), m.attachment ? h('div', { class: 'muted small' }, m.attachment) : null);
      case 'assistant':
        return h('div', { class: 'msg assistant', 'aria-label': `Assistant: ${m.text}` }, markdownText(m.text));
      case 'tool': {
        const detail = m.detail;
        const det = h(
          'details',
          { class: `msg tool${m.failed ? ' failed' : ''}`, title: m.failed ? 'This step didn’t work. The Assistant sees the same message and usually tries again.' : '' },
          h('summary', { class: 'md-inline' }, icon(m.failed ? Icons.warning : Icons.wrench), ' ', ...inline(m.text)),
          detail ? copyable(h('pre', null, detail), 'Copy the details', () => detail) : null,
        );
        return det;
      }
      case 'question':
        if (m.question?.state === 'pending') return questionCard(m.question.questions, (ans) => a.answer(m, ans), () => a.skip(m));
        return h('div', { class: 'msg note' }, m.question?.state === 'answered' ? `Answered: ${m.question.answer}` : 'Not answered');
      case 'error':
        return h('div', { class: 'msg error', role: 'alert' }, copyable(h('span', null, m.text), 'Copy the error', () => m.text));
      case 'usage':
        return h('div', { class: 'msg usage muted small' }, m.text);
      default:
        return h('div', { class: 'msg note muted small' }, m.text);
    }
  }

  return root;
}
