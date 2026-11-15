// The spec editor (ui/spec.md §5, §23.3): CodeMirror 6 with Markdown, the Just Notes slash
// commands plus the spec blocks (/scene /vo /shot /music /cta) from the core, `media:` drops,
// pasted images into the `.assets` sidecar, lint markers, find and replace, and table tools.

import { EditorState, Compartment, type Extension, EditorSelection } from '@codemirror/state';
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, placeholder as cmPlaceholder } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab, undo, redo } from '@codemirror/commands';
import { markdown, markdownLanguage } from '@codemirror/lang-markdown';
import { languages } from '@codemirror/language-data';
import { syntaxHighlighting, defaultHighlightStyle, HighlightStyle, bracketMatching } from '@codemirror/language';
import { search, searchKeymap, openSearchPanel, highlightSelectionMatches } from '@codemirror/search';
import { autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap, type Completion, type CompletionContext } from '@codemirror/autocomplete';
import { setDiagnostics, lintGutter, type Diagnostic } from '@codemirror/lint';
import { tags } from '@lezer/highlight';
import { core } from '../bridge';
import { fileName } from '../dom';
import type { LintIssue } from '../types';

export interface SlashBlock {
  command: string;
  title: string;
  snippet: string;
}

/** The Just Notes slash commands that aren't spec blocks (MarkdownCore/SlashCommands.swift). */
const FORMATTING: SlashBlock[] = [
  { command: 'h1', title: 'Heading 1', snippet: '# {{Heading}}' },
  { command: 'h2', title: 'Heading 2', snippet: '## {{Heading}}' },
  { command: 'h3', title: 'Heading 3', snippet: '### {{Heading}}' },
  { command: 'h4', title: 'Heading 4', snippet: '#### {{Heading}}' },
  { command: 'h5', title: 'Heading 5', snippet: '##### {{Heading}}' },
  { command: 'h6', title: 'Heading 6', snippet: '###### {{Heading}}' },
  { command: 'bold', title: 'Bold', snippet: '**{{text}}**' },
  { command: 'italic', title: 'Italic', snippet: '*{{text}}*' },
  { command: 'strike', title: 'Strikethrough', snippet: '~~{{text}}~~' },
  { command: 'code', title: 'Inline code', snippet: '`{{code}}`' },
  { command: 'bullet', title: 'Bulleted list', snippet: '- {{item}}' },
  { command: 'numbered', title: 'Numbered list', snippet: '1. {{item}}' },
  { command: 'todo', title: 'Task list', snippet: '- [ ] {{task}}' },
  { command: 'quote', title: 'Quote', snippet: '> {{quote}}' },
  { command: 'codeblock', title: 'Code block', snippet: '```{{language}}\n\n```' },
  { command: 'table', title: 'Table', snippet: '| {{Column}} | Column |\n|---|---|\n|  |  |' },
  { command: 'url', title: 'Link', snippet: '[{{text}}](https://)' },
  { command: 'image', title: 'Image link', snippet: '![{{alt}}](media:)' },
  { command: 'hr', title: 'Horizontal rule', snippet: '---' },
  { command: 'note', title: 'Note alert', snippet: '> [!NOTE]\n> {{text}}' },
  { command: 'tip', title: 'Tip alert', snippet: '> [!TIP]\n> {{text}}' },
  { command: 'important', title: 'Important alert', snippet: '> [!IMPORTANT]\n> {{text}}' },
  { command: 'warning', title: 'Warning alert', snippet: '> [!WARNING]\n> {{text}}' },
  { command: 'caution', title: 'Caution alert', snippet: '> [!CAUTION]\n> {{text}}' },
  { command: 'mermaid', title: 'Mermaid diagram', snippet: '```mermaid\n{{graph LR\n  A --> B}}\n```' },
  { command: 'math', title: 'Math block', snippet: '$$\n{{E = mc^2}}\n$$' },
  { command: 'footnote', title: 'Footnote', snippet: '[^{{1}}]' },
  { command: 'details', title: 'Collapsible section', snippet: '<details>\n<summary>{{Summary}}</summary>\n\n</details>' },
  { command: 'island', title: 'Island', snippet: '<!-- island: {{name}} -->\n\n<!-- /island: name -->' },
  { command: 'date', title: 'Today’s date', snippet: '' },
  { command: 'time', title: 'Current time', snippet: '' },
];

const highlight = HighlightStyle.define([
  { tag: tags.heading1, fontWeight: '700', fontSize: '1.25em' },
  { tag: tags.heading2, fontWeight: '700', fontSize: '1.12em' },
  { tag: [tags.heading3, tags.heading4, tags.heading5, tags.heading6], fontWeight: '600' },
  { tag: tags.strong, fontWeight: '700' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: tags.strikethrough, textDecoration: 'line-through' },
  { tag: [tags.link, tags.url], color: 'var(--accent)' },
  { tag: tags.monospace, fontFamily: 'var(--font-mono)' },
  { tag: [tags.processingInstruction, tags.meta, tags.contentSeparator], color: 'var(--text-muted)' },
  { tag: tags.quote, color: 'var(--text-muted)' },
]);

export interface EditorHost {
  onChange(text: string): void;
  onCursor(line: number, col: number, selectionWords: number | null): void;
  /** Files dropped on the editor: add them as media and return their ids. */
  addMedia(files: string[]): Promise<string[]>;
  /** A pasted image (base64): store it in the sidecar, return its media id. */
  addPastedImage(base64: string): Promise<string | null>;
}

export class SpecEditor {
  view: EditorView;
  private readOnly = new Compartment();
  private wrap = new Compartment();
  private suppress = false;
  private blocks: SlashBlock[] = [];

  constructor(parent: HTMLElement, text: string, private host: EditorHost) {
    core<SlashBlock[]>('spec.slash_blocks').then((b) => (this.blocks = b)).catch(() => {});
    const extensions: Extension[] = [
      lineNumbers(),
      highlightActiveLineGutter(),
      history(),
      drawSelection(),
      highlightActiveLine(),
      bracketMatching(),
      closeBrackets(),
      highlightSelectionMatches(),
      markdown({ base: markdownLanguage, codeLanguages: languages }),
      syntaxHighlighting(highlight),
      syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
      search({ top: true }),
      lintGutter(),
      autocompletion({ override: [(c) => this.slash(c)], icons: false }),
      this.readOnly.of(EditorState.readOnly.of(false)),
      this.wrap.of(EditorView.lineWrapping),
      cmPlaceholder('Describe the video: scenes, voice-over, shots, music and the call to action. Type / for blocks.'),
      keymap.of([
        { key: 'Tab', run: (v) => this.tableTab(v, 1) },
        { key: 'Shift-Tab', run: (v) => this.tableTab(v, -1) },
        { key: 'Mod-Alt-t', run: (v) => this.formatTable(v) },
        { key: 'Mod-b', run: (v) => this.wrapSelection(v, '**') },
        { key: 'Mod-i', run: (v) => this.wrapSelection(v, '*') },
        { key: 'Mod-k', run: (v) => this.link(v) },
        { key: 'Mod-h', run: (v) => openSearchPanel(v) },
        ...closeBracketsKeymap,
        ...completionKeymap,
        ...searchKeymap,
        ...historyKeymap,
        ...defaultKeymap,
        indentWithTab,
      ]),
      EditorView.updateListener.of((u) => {
        if (u.docChanged && !this.suppress) this.host.onChange(u.state.doc.toString());
        if (u.selectionSet || u.docChanged) {
          const sel = u.state.selection.main;
          const line = u.state.doc.lineAt(sel.head);
          const words = sel.empty ? null : u.state.sliceDoc(sel.from, sel.to).split(/\s+/).filter(Boolean).length;
          this.host.onCursor(line.number, sel.head - line.from + 1, words);
        }
      }),
      EditorView.domEventHandlers({
        drop: (e, view) => this.drop(e, view),
        paste: (e, view) => this.paste(e, view),
      }),
      EditorView.contentAttributes.of({ 'aria-label': 'Spec editor', spellcheck: 'true' }),
    ];
    this.view = new EditorView({ state: EditorState.create({ doc: text, extensions }), parent });
  }

  get text(): string {
    return this.view.state.doc.toString();
  }

  /** Replaces the whole text. `undoable` keeps it as one undo step (accepted hunks). */
  setText(text: string, undoable = false) {
    if (text === this.text) return;
    this.suppress = !undoable;
    this.view.dispatch({
      changes: { from: 0, to: this.view.state.doc.length, insert: text },
      annotations: undoable ? [] : [],
    });
    this.suppress = false;
    if (undoable) this.host.onChange(text);
  }

  setReadOnly(ro: boolean) {
    this.view.dispatch({ effects: this.readOnly.reconfigure(EditorState.readOnly.of(ro)) });
  }

  setWrap(on: boolean) {
    this.view.dispatch({ effects: this.wrap.reconfigure(on ? EditorView.lineWrapping : []) });
  }

  focus() {
    this.view.focus();
  }

  goToLine(line: number) {
    const doc = this.view.state.doc;
    const l = doc.line(Math.max(1, Math.min(line, doc.lines)));
    this.view.dispatch({ selection: { anchor: l.from }, scrollIntoView: true, effects: EditorView.scrollIntoView(l.from, { y: 'center' }) });
    this.view.focus();
  }

  cursorLine(): number {
    return this.view.state.doc.lineAt(this.view.state.selection.main.head).number;
  }

  selection(): { text: string; from: number; to: number } | null {
    const s = this.view.state.selection.main;
    if (s.empty) return null;
    const doc = this.view.state.doc;
    return { text: this.view.state.sliceDoc(s.from, s.to), from: doc.lineAt(s.from).number, to: doc.lineAt(s.to).number };
  }

  undo() {
    undo(this.view);
  }
  redo() {
    redo(this.view);
  }
  find() {
    openSearchPanel(this.view);
  }

  setLint(issues: LintIssue[]) {
    const doc = this.view.state.doc;
    const diags: Diagnostic[] = issues
      .filter((i) => i.line >= 1 && i.line <= doc.lines)
      .map((i) => {
        const l = doc.line(i.line);
        return { from: l.from, to: Math.max(l.from, l.to), severity: i.severity === 'warning' ? 'warning' : 'info', message: i.message, source: i.rule };
      });
    this.view.dispatch(setDiagnostics(this.view.state, diags));
  }

  /** Inserts a structured block (`scene`, `vo`, …) on its own line, selecting its first placeholder. */
  insertBlock(command: string) {
    const b = this.blocks.find((x) => x.command === command) ?? FORMATTING.find((x) => x.command === command);
    if (!b) return;
    const v = this.view;
    const sel = v.state.selection.main;
    const line = v.state.doc.lineAt(sel.head);
    const atStart = line.text.trim() === '';
    const prefix = atStart ? '' : '\n\n';
    this.insertSnippet(v, atStart ? line.from : line.to, atStart ? line.to : line.to, prefix + b.snippet + '\n');
  }

  private insertSnippet(v: EditorView, from: number, to: number, snippet: string) {
    let text = snippet;
    if (snippet === '' ) text = '';
    const m = /\{\{([^}]*)\}\}/.exec(text);
    let selFrom = from + text.length;
    let selTo = selFrom;
    if (m) {
      selFrom = from + m.index;
      selTo = selFrom + m[1].length;
      text = text.slice(0, m.index) + m[1] + text.slice(m.index + m[0].length);
      text = text.replace(/\{\{([^}]*)\}\}/g, '$1');
    }
    v.dispatch({ changes: { from, to, insert: text }, selection: EditorSelection.single(selFrom, selTo), scrollIntoView: true, userEvent: 'input.snippet' });
    v.focus();
  }

  private slash(ctx: CompletionContext) {
    const word = ctx.matchBefore(/(^|\s)\/[\w-]*$/);
    if (!word) return null;
    const start = word.text.startsWith('/') ? word.from : word.from + 1;
    const all = [...this.blocks, ...FORMATTING];
    const options: Completion[] = all.map((b) => ({
      label: `/${b.command}`,
      detail: b.title,
      apply: (view: EditorView, _c: Completion, from: number, to: number) => {
        if (b.command === 'date' || b.command === 'time') {
          const d = new Date();
          const s = b.command === 'date' ? d.toISOString().slice(0, 10) : d.toTimeString().slice(0, 5);
          view.dispatch({ changes: { from, to, insert: s }, selection: { anchor: from + s.length } });
          return;
        }
        this.insertSnippet(view, from, to, b.snippet);
      },
    }));
    return { from: start, options, validFor: /^\/[\w-]*$/ };
  }

  private wrapSelection(v: EditorView, mark: string): boolean {
    v.dispatch(v.state.changeByRange((r) => ({
      changes: [{ from: r.from, insert: mark }, { from: r.to, insert: mark }],
      range: EditorSelection.range(r.from + mark.length, r.to + mark.length),
    })));
    return true;
  }

  private link(v: EditorView): boolean {
    const r = v.state.selection.main;
    const t = v.state.sliceDoc(r.from, r.to) || 'text';
    this.insertSnippet(v, r.from, r.to, `[${t}]({{https://}})`);
    return true;
  }

  // MARK: Tables

  private tableRange(v: EditorView): { from: number; to: number; lines: string[] } | null {
    const doc = v.state.doc;
    const cur = doc.lineAt(v.state.selection.main.head);
    if (!cur.text.trim().startsWith('|')) return null;
    let a = cur.number;
    let b = cur.number;
    while (a > 1 && doc.line(a - 1).text.trim().startsWith('|')) a--;
    while (b < doc.lines && doc.line(b + 1).text.trim().startsWith('|')) b++;
    const lines: string[] = [];
    for (let i = a; i <= b; i++) lines.push(doc.line(i).text);
    return { from: doc.line(a).from, to: doc.line(b).to, lines };
  }

  /** Aligns the pipes of the table under the cursor. */
  formatTable(v: EditorView = this.view): boolean {
    const t = this.tableRange(v);
    if (!t) return false;
    const rows = t.lines.map((l) => l.trim().replace(/^\||\|$/g, '').split('|').map((c) => c.trim()));
    const cols = Math.max(...rows.map((r) => r.length));
    const width = Array.from({ length: cols }, (_, i) => Math.max(3, ...rows.map((r) => (/^:?-+:?$/.test(r[i] ?? '') ? 3 : (r[i] ?? '').length))));
    const out = rows
      .map((r) => '| ' + width.map((w, i) => {
        const c = r[i] ?? '';
        return /^:?-+:?$/.test(c) ? '-'.repeat(w) : c.padEnd(w);
      }).join(' | ') + ' |')
      .join('\n');
    v.dispatch({ changes: { from: t.from, to: t.to, insert: out } });
    return true;
  }

  /** Tab / Shift-Tab move between table cells (and format the table). */
  private tableTab(v: EditorView, dir: 1 | -1): boolean {
    if (!this.tableRange(v)) return false;
    this.formatTable(v);
    const doc = v.state.doc;
    const pos = v.state.selection.main.head;
    const text = doc.toString();
    let i = pos;
    if (dir > 0) {
      i = text.indexOf('|', pos);
      if (i < 0) return true;
      i += 1;
      if (text[i] === '\n' || i >= text.length) {
        const next = text.indexOf('|', i);
        if (next < 0 || !doc.lineAt(next).text.trim().startsWith('|')) return true;
        i = next + 1;
      }
    } else {
      i = text.lastIndexOf('|', pos - 2);
      if (i < 0) return true;
      i = text.lastIndexOf('|', i - 1) + 1;
    }
    while (text[i] === ' ') i++;
    v.dispatch({ selection: { anchor: i } });
    return true;
  }

  // MARK: Drop and paste

  private drop(e: DragEvent, view: EditorView): boolean {
    const files = [...(e.dataTransfer?.files ?? [])];
    const paths = files.map((f) => (f as any).path as string | undefined).filter((p): p is string => !!p);
    const uri = e.dataTransfer?.getData('text/uri-list');
    if (!paths.length && uri) {
      for (const u of uri.split(/\r?\n/)) if (u.startsWith('file:///')) paths.push(decodeURIComponent(u.slice(8)).replace(/\//g, '\\'));
    }
    if (!paths.length) return false;
    e.preventDefault();
    const pos = view.posAtCoords({ x: e.clientX, y: e.clientY }) ?? view.state.selection.main.head;
    this.host.addMedia(paths).then((ids) => this.insertMediaRefs(ids, paths, pos));
    return true;
  }

  insertMediaRefs(ids: string[], paths: string[] = [], pos?: number) {
    if (!ids.length) return;
    const v = this.view;
    const at = pos ?? v.state.selection.main.head;
    const line = v.state.doc.lineAt(at);
    const text = ids.map((id, i) => `![${(paths[i] && fileName(paths[i]).replace(/\.[^.]+$/, '')) || id}](media:${id})`).join('\n');
    const insert = (line.text.trim() ? '\n' : '') + text + '\n';
    v.dispatch({ changes: { from: line.to, insert }, selection: { anchor: line.to + insert.length }, userEvent: 'input.drop' });
  }

  private paste(e: ClipboardEvent, _view: EditorView): boolean {
    const items = [...(e.clipboardData?.items ?? [])];
    const image = items.find((i) => i.kind === 'file' && i.type.startsWith('image/'));
    if (!image) return false;
    const file = image.getAsFile();
    if (!file) return false;
    e.preventDefault();
    const reader = new FileReader();
    reader.onload = async () => {
      const b64 = String(reader.result).replace(/^data:[^,]*,/, '');
      const id = await this.host.addPastedImage(b64);
      if (id) this.insertMediaRefs([id], ['pasted image']);
    };
    reader.readAsDataURL(file);
    return true;
  }
}
