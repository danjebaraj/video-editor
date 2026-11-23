// The Assistant's edits, reviewed per hunk (spec §4.3, §23.3): a card per change from the core's diff
// with Accept and Reject, and the whole change in a unified merge view (@codemirror/merge).

import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { unifiedMergeView } from '@codemirror/merge';
import { markdown } from '@codemirror/lang-markdown';
import { h, clear, button, Icons } from '../dom';
import { effect } from '../store';
import type { Assistant } from './assistant';

export function reviewView(a: Assistant): HTMLElement {
  const root = h('section', { class: 'review', 'aria-label': 'Review the Assistant’s changes' });
  let merge: EditorView | null = null;
  effect([a.review], () => {
    const r = a.review.value;
    merge?.destroy();
    merge = null;
    if (!r) {
      clear(root);
      return;
    }
    const accepted = r.decisions.filter((d) => d === true).length;
    const undecided = r.decisions.filter((d) => d === null).length;
    const cards = r.hunks.map((hk, i) => {
      const before = r.base.split('\n');
      const ctxStart = Math.max(0, hk.old_start - 2);
      const ctxBefore = before.slice(ctxStart, hk.old_start);
      const ctxAfter = before.slice(hk.old_start + hk.old_lines.length, hk.old_start + hk.old_lines.length + 2);
      const d = r.decisions[i];
      return h(
        'article',
        { class: `hunk${d === false ? ' rejected' : ''}${d === true ? ' accepted' : ''}`, 'aria-label': `Change ${i + 1}, line ${hk.old_start + 1}` },
        h('div', { class: 'row' },
          h('strong', null, `Change ${i + 1}`), h('span', { class: 'muted small' }, ` · line ${hk.old_start + 1}`), h('span', { class: 'grow' }),
          d === true ? h('span', { class: 'ok small' }, 'Accepted') : d === false ? h('span', { class: 'muted small' }, 'Rejected') : null,
          button('Reject', () => a.decide(i, false), { appearance: 'subtle', glyph: Icons.dismiss }),
          button('Accept', () => a.decide(i, true), { appearance: 'primary', glyph: Icons.check }),
        ),
        h('pre', { class: 'diff' },
          ctxBefore.map((l) => h('div', { class: 'ctx' }, `  ${l}`)),
          hk.old_lines.map((l) => h('div', { class: 'del' }, `− ${l}`)),
          hk.new_lines.map((l) => h('div', { class: 'add' }, `+ ${l}`)),
          ctxAfter.map((l) => h('div', { class: 'ctx' }, `  ${l}`)),
        ),
      );
    });
    const full = h('div', { class: 'merge-host' });
    clear(
      root,
      h('header', { class: 'row' },
        h('div', { class: 'grow' }, h('h2', null, 'The Assistant changed the spec'), h('div', { class: 'muted small' }, `${r.hunks.length} changes · ${accepted} accepted · ${undecided} to review`)),
        button('Reject All', () => a.rejectAll()),
        button('Accept All', () => a.acceptAll(), { appearance: 'primary' }),
        button('Done', () => a.finish()),
      ),
      h('div', { class: 'hunks' }, cards),
      h('details', { class: 'full-diff' }, h('summary', null, 'The whole change'), full),
    );
    merge = new EditorView({
      parent: full,
      state: EditorState.create({
        doc: r.proposed,
        extensions: [EditorView.editable.of(false), EditorState.readOnly.of(true), EditorView.lineWrapping, markdown(), unifiedMergeView({ original: r.base, mergeControls: false, collapseUnchanged: { margin: 2, minSize: 4 } })],
      }),
    });
  });
  return root;
}
