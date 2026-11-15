// A small DOM toolkit: element builder, formatting helpers and Segoe Fluent Icons.

type Child = Node | string | number | false | null | undefined | Child[];
type Props = Record<string, any> & { class?: string; style?: string | Record<string, string> };

/** `h('button', {onclick, 'aria-label': 'Run'}, 'Run')` */
export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props?: Props | null, ...children: Child[]): HTMLElementTagNameMap[K];
export function h(tag: string, props?: Props | null, ...children: Child[]): HTMLElement;
export function h(tag: string, props?: Props | null, ...children: Child[]): HTMLElement {
  const el = document.createElement(tag);
  if (props) {
    for (const [k, v] of Object.entries(props)) {
      if (v === undefined || v === null || v === false) continue;
      if (k.startsWith('on') && typeof v === 'function') {
        el.addEventListener(k.slice(2).toLowerCase(), v);
      } else if (k === 'style' && typeof v === 'object') {
        Object.assign(el.style, v);
      } else if (k === 'class') {
        el.className = v;
      } else if (k === 'value' && 'value' in el) {
        (el as any).value = v;
      } else if (k === 'checked' && 'checked' in el) {
        (el as any).checked = !!v;
      } else if (k === 'dataset') {
        Object.assign(el.dataset, v);
      } else if (v === true) {
        el.setAttribute(k, '');
      } else {
        el.setAttribute(k, String(v));
      }
    }
  }
  append(el, children);
  return el;
}

function append(el: Node, children: Child[]) {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else el.appendChild(typeof c === 'object' ? c : document.createTextNode(String(c)));
  }
}

export function clear(el: Element, ...children: Child[]) {
  el.replaceChildren();
  append(el, children);
}

/** A Segoe Fluent Icons glyph (decorative; the control carries the accessible name). */
export function icon(glyph: string, cls = ''): HTMLElement {
  return h('span', { class: `icon ${cls}`, 'aria-hidden': 'true' }, glyph);
}

export const Icons = {
  play: '',
  stop: '',
  check: '',
  checkCircle: '',
  warning: '',
  error: '',
  info: '',
  add: '',
  remove: '',
  delete: '',
  folder: '',
  openFolder: '',
  document: '',
  video: '',
  audio: '',
  image: '',
  font: '',
  link: '',
  settings: '',
  search: '',
  edit: '',
  chat: '',
  send: '',
  refresh: '',
  more: '',
  chevronDown: '',
  chevronRight: '',
  question: '',
  validate: '',
  outline: '',
  history: '',
  library: '',
  preview: '',
  code: '',
  brand: '',
  globe: '',
  copy: '',
  dismiss: '',
  spinner: '',
  missing: '',
  changed: '',
  key: '',
  wrench: '',
  revise: '',
  rerun: '',
  window: '',
};

export function debounce<T extends (...a: any[]) => void>(fn: T, ms: number): T & { flush(): void; cancel(): void } {
  let timer: number | undefined;
  let lastArgs: any[] = [];
  const f = ((...args: any[]) => {
    lastArgs = args;
    if (timer !== undefined) clearTimeout(timer);
    timer = window.setTimeout(() => {
      timer = undefined;
      fn(...lastArgs);
    }, ms);
  }) as T & { flush(): void; cancel(): void };
  f.flush = () => {
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
      fn(...lastArgs);
    }
  };
  f.cancel = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  };
  return f;
}

export function bytes(n: number | undefined | null): string {
  if (n === undefined || n === null) return '—';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = n;
  let i = 0;
  while (v >= 1000 && i < u.length - 1) {
    v /= 1000;
    i++;
  }
  return i === 0 ? `${n} B` : `${v.toFixed(1)} ${u[i]}`;
}

export function duration(s: number | undefined | null): string {
  if (s === undefined || s === null || !isFinite(s)) return '—';
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)} s`;
  const m = Math.floor(s / 60);
  const r = Math.round(s % 60);
  return `${m}:${String(r).padStart(2, '0')}`;
}

export function usd(v: number | undefined | null, estimate = false): string {
  if (v === undefined || v === null) return '—';
  return `${estimate ? 'about ' : ''}$${v < 1 ? v.toFixed(3) : v.toFixed(2)}`;
}

export function when(ts: string | undefined | null): string {
  if (!ts) return '';
  const d = new Date(ts);
  if (isNaN(d.getTime())) return ts;
  return d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

export function elapsed(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, '0')}`;
}

export function fileName(p: string | undefined | null): string {
  if (!p) return '';
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

export function fileStem(p: string): string {
  const n = fileName(p);
  const i = n.lastIndexOf('.');
  return i > 0 ? n.slice(0, i) : n;
}

export function parentDir(p: string): string {
  const i = Math.max(p.lastIndexOf('\\'), p.lastIndexOf('/'));
  return i > 0 ? p.slice(0, i) : p;
}

export function joinPath(dir: string, name: string): string {
  return dir.endsWith('\\') || dir.endsWith('/') ? dir + name : `${dir}\\${name}`;
}

/** A field: a label with its control, so every control has an accessible name. */
export function field(label: string, control: HTMLElement, hint?: string): HTMLElement {
  const id = control.id || `f-${Math.random().toString(36).slice(2, 9)}`;
  control.id = id;
  return h('div', { class: 'field' }, h('label', { for: id }, label), control, hint ? h('div', { class: 'hint' }, hint) : null);
}

/** A text input bound to a getter and setter. */
export function textInput(value: string | undefined | null, onChange: (v: string) => void, props: Props = {}): HTMLInputElement {
  const el = h('input', { type: 'text', value: value ?? '', ...props }) as HTMLInputElement;
  el.addEventListener('change', () => onChange(el.value));
  return el;
}

export function numberInput(value: number | undefined | null, onChange: (v: number | null) => void, props: Props = {}): HTMLInputElement {
  const el = h('input', { type: 'number', value: value ?? '', ...props }) as HTMLInputElement;
  el.addEventListener('change', () => {
    const v = el.value.trim() === '' ? null : Number(el.value);
    onChange(v === null || isNaN(v) ? null : v);
  });
  return el;
}

export function select(options: [string, string][], value: string | undefined | null, onChange: (v: string) => void, props: Props = {}): HTMLSelectElement {
  const el = h('select', props, options.map(([v, label]) => h('option', { value: v, selected: v === (value ?? '') }, label))) as HTMLSelectElement;
  el.value = value ?? '';
  el.addEventListener('change', () => onChange(el.value));
  return el;
}

export function checkbox(label: string, checked: boolean, onChange: (v: boolean) => void, props: Props = {}): HTMLElement {
  const input = h('input', { type: 'checkbox', checked, ...props }) as HTMLInputElement;
  input.addEventListener('change', () => onChange(input.checked));
  return h('label', { class: 'check' }, input, h('span', null, label));
}

export function button(label: string, onClick: (e: MouseEvent) => void, props: Props & { appearance?: string; glyph?: string } = {}): HTMLElement {
  const { appearance, glyph, ...rest } = props;
  const b = h(
    'fluent-button',
    { appearance: appearance ?? 'secondary', size: 'medium', ...rest },
    glyph ? icon(glyph) : null,
    label,
  ) as HTMLElement;
  b.addEventListener('click', (e) => {
    if ((b as any).disabled || b.hasAttribute('disabled')) return;
    onClick(e as MouseEvent);
  });
  return b;
}

export function iconButton(glyph: string, label: string, onClick: (e: MouseEvent) => void, props: Props = {}): HTMLElement {
  const b = h('fluent-button', { appearance: 'subtle', 'icon-only': true, 'aria-label': label, title: label, ...props }, icon(glyph)) as HTMLElement;
  b.addEventListener('click', (e) => {
    if (b.hasAttribute('disabled')) return;
    onClick(e as MouseEvent);
  });
  return b;
}

export function setDisabled(el: HTMLElement, disabled: boolean) {
  if (disabled) el.setAttribute('disabled', '');
  else el.removeAttribute('disabled');
}

export function spinner(label = 'Working'): HTMLElement {
  return h('fluent-spinner', { size: 'tiny', 'aria-label': label });
}

export function emptyState(title: string, body?: string, ...actions: HTMLElement[]): HTMLElement {
  return h('div', { class: 'empty', role: 'status' }, h('div', { class: 'empty-title' }, title), body ? h('p', null, body) : null, actions.length ? h('div', { class: 'row' }, actions) : null);
}

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
}
