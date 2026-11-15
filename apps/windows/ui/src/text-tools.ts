// One-click copy and Markdown preview, used wherever the app shows text it didn't ask the user to
// type (the Windows counterpart of the Mac app's TextTools.swift). Markdown is turned into DOM
// nodes directly, never through innerHTML, so text from Claude or a log can't inject markup.

import { h, icon, Icons } from './dom';
import type { Child } from './dom';
import { request } from './bridge';

/** Puts text on the clipboard through the shell: `navigator.clipboard` fails while the page
 *  doesn't have focus, as right after a dialog closes. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await request('shell.copy_text', { text });
    return true;
  } catch {
    return navigator.clipboard.writeText(text).then(() => true, () => false);
  }
}

/** The clipboard's text, through the shell (see [`copyText`]). */
export function clipboardText(): Promise<string | null> {
  return request<string | null>('shell.clipboard_text').catch(() => navigator.clipboard.readText().catch(() => null));
}

/** A small button that copies text and shows a checkmark for a moment. With `title`, a labelled
 *  button ("Copy All") instead of an icon. */
export function copyButton(label: string, text: () => string, opts: { title?: string } = {}): HTMLElement {
  const content = () => (opts.title ? [icon(Icons.copy), ` ${opts.title}`] : [icon(Icons.copy)]);
  const b = h('button', { type: 'button', class: opts.title ? 'copy-btn labelled' : 'copy-btn', title: label, 'aria-label': label }, ...content());
  let timer: number | undefined;
  b.addEventListener('click', async (e) => {
    e.stopPropagation();
    if (!(await copyText(text()))) return;
    b.classList.add('copied');
    b.replaceChildren(icon(Icons.check), ...(opts.title ? [' Copied'] : []));
    b.title = 'Copied';
    clearTimeout(timer);
    timer = window.setTimeout(() => {
      b.classList.remove('copied');
      b.replaceChildren(...content());
      b.title = label;
    }, 1200);
  });
  return b;
}

/** Puts a copy button beside an element (top trailing), not over it. */
export function copyable(el: HTMLElement, label: string, text: () => string): HTMLElement {
  return h('div', { class: 'copyable' }, h('div', { class: 'grow' }, el), copyButton(label, text));
}

/** A button that switches a Markdown text between its preview and its source. */
export function previewToggle(preview: boolean, onToggle: () => void, opts: { disabled?: boolean } = {}): HTMLElement {
  const label = preview ? 'Show Markdown source' : 'Show rendered preview';
  return h('button', { type: 'button', class: 'copy-btn', title: label, 'aria-label': label, 'aria-pressed': String(!preview), disabled: opts.disabled, onclick: onToggle }, icon(preview ? Icons.code : Icons.preview));
}

/** Markdown with a copy button and a preview toggle beside it. Starts as a preview. */
export function markdownText(text: string, cls = ''): HTMLElement {
  let preview = true;
  const root = h('div', { class: `markdown-text ${cls}`.trim() });
  const render = () =>
    root.replaceChildren(
      preview ? markdownBlocks(text) : h('pre', { class: 'md-source' }, text),
      h('div', { class: 'md-tools' }, copyButton('Copy the Markdown', () => text), previewToggle(preview, () => ((preview = !preview), render()))),
    );
  render();
  return root;
}

export type Block =
  | { kind: 'heading'; level: number; text: string }
  | { kind: 'paragraph'; text: string }
  | { kind: 'item'; indent: number; marker: string; text: string }
  | { kind: 'quote'; text: string }
  | { kind: 'code'; text: string }
  | { kind: 'table'; rows: string[][] }
  | { kind: 'rule' };

/** Block structure of Markdown: headings, paragraphs, list items, quotes, code, tables, rules.
 *  The same rules as MarkdownBlocks.parse in the Mac app. */
export function parseMarkdown(text: string): Block[] {
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  let code: string[] | null = null;
  let table: string[][] = [];
  const flush = () => {
    if (paragraph.length) blocks.push({ kind: 'paragraph', text: paragraph.join(' ') });
    paragraph = [];
    if (table.length) blocks.push({ kind: 'table', rows: table });
    table = [];
  };
  for (const raw of text.replace(/\r\n/g, '\n').split('\n')) {
    const line = raw.trim();
    if (code) {
      if (line.startsWith('```') || line.startsWith('~~~')) {
        blocks.push({ kind: 'code', text: code.join('\n') });
        code = null;
      } else code.push(raw);
      continue;
    }
    if (line.startsWith('```') || line.startsWith('~~~')) {
      flush();
      code = [];
      continue;
    }
    if (!line) {
      flush();
      continue;
    }
    if (line.startsWith('|') && line.endsWith('|') && line.length > 1) {
      if (paragraph.length) flush();
      const cells = line.slice(1, -1).split('|').map((c) => c.trim());
      const separator = cells.every((c) => c.length > 0 && /^[-: ]+$/.test(c));
      if (!separator) table.push(cells);
      continue;
    }
    if (table.length) flush();
    const level = headingLevel(line);
    const item = listItem(line);
    if (level) {
      flush();
      blocks.push({ kind: 'heading', level, text: line.slice(level).trim() });
    } else if (line.length >= 3 && /^[-*_ ]+$/.test(line) && new Set(line.replace(/ /g, '')).size === 1) {
      flush();
      blocks.push({ kind: 'rule' });
    } else if (line.startsWith('>')) {
      flush();
      blocks.push({ kind: 'quote', text: line.slice(1).trim() });
    } else if (item) {
      flush();
      const lead = raw.match(/^[ \t]*/)![0];
      const indent = Math.floor([...lead].reduce((n, c) => n + (c === '\t' ? 2 : 1), 0) / 2);
      blocks.push({ kind: 'item', indent: Math.min(indent, 6), marker: item[0], text: item[1] });
    } else paragraph.push(line);
  }
  if (code) blocks.push({ kind: 'code', text: code.join('\n') });
  flush();
  return blocks;
}

function headingLevel(line: string): number | null {
  const hashes = line.match(/^#*/)![0].length;
  return hashes >= 1 && hashes <= 6 && line[hashes] === ' ' ? hashes : null;
}

/** "- text", "* text", "+ text", "1. text" or "1) text", with task boxes shown as such. */
function listItem(line: string): [string, string] | null {
  if ('-*+'.includes(line[0]) && line[1] === ' ') {
    const rest = line.slice(2).trim();
    if (rest.startsWith('[ ] ')) return ['☐', rest.slice(4)];
    if (rest.toLowerCase().startsWith('[x] ')) return ['☑', rest.slice(4)];
    return ['•', rest];
  }
  const m = line.match(/^(\d{1,3})[.)] (.*)$/);
  return m ? [`${m[1]}.`, m[2].trim()] : null;
}

/** Inline Markdown: `code`, **strong**, *emphasis* / _emphasis_ and [links](https://…). */
export function inline(s: string): Child[] {
  const out: Child[] = [];
  const re = /`([^`]+)`|\*\*([^*]+)\*\*|__([^_]+)__|\*([^*\s][^*]*)\*|(?<![\w])_([^_\s][^_]*)_(?![\w])|\[([^\]]+)\]\(([^)\s]+)\)/g;
  let last = 0;
  for (const m of s.matchAll(re)) {
    if (m.index! > last) out.push(s.slice(last, m.index));
    if (m[1] != null) out.push(h('code', null, m[1]));
    else if (m[2] != null || m[3] != null) out.push(h('strong', null, ...inline(m[2] ?? m[3])));
    else if (m[4] != null || m[5] != null) out.push(h('em', null, ...inline(m[4] ?? m[5])));
    else if (/^https?:\/\//i.test(m[7])) out.push(h('a', { href: m[7], target: '_blank', rel: 'noreferrer' }, ...inline(m[6])));
    else out.push(...inline(m[6]));
    last = m.index! + m[0].length;
  }
  if (last < s.length) out.push(s.slice(last));
  return out;
}

/** Renders Markdown as DOM: headings, paragraphs, lists, quotes, code blocks, tables and rules. */
export function markdownBlocks(text: string): HTMLElement {
  return h('div', { class: 'md' }, parseMarkdown(text).map((b) => block(b)));
}

function block(b: Block): HTMLElement {
  switch (b.kind) {
    case 'heading':
      return h(`h${Math.min(b.level + 2, 6)}` as 'h3', { class: `md-h${b.level}` }, ...inline(b.text));
    case 'paragraph':
      return h('p', null, ...inline(b.text));
    case 'item':
      return h('div', { class: 'md-item', style: { paddingLeft: `${b.indent * 14}px` } }, h('span', { class: 'md-marker', 'aria-hidden': 'true' }, b.marker), h('span', null, ...inline(b.text)));
    case 'quote':
      return h('blockquote', null, ...inline(b.text));
    case 'code':
      return h('div', { class: 'md-code' }, h('pre', null, b.text), copyButton('Copy the code', () => b.text));
    case 'table':
      return h('table', { class: 'md-table' },
        h('thead', null, h('tr', null, (b.rows[0] ?? []).map((c) => h('th', null, ...inline(c))))),
        h('tbody', null, b.rows.slice(1).map((r) => h('tr', null, r.map((c) => h('td', null, ...inline(c)))))));
    case 'rule':
      return h('hr');
  }
}
