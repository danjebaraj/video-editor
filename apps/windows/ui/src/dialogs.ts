// In-window dialogs (sheets) built on <dialog>, which gives focus trapping, Esc to close and a
// modal role for Narrator and UI Automation. Native file pickers and multi-button questions go
// through the shell (dialog.open / dialog.save / dialog.confirm).

import { h, button } from './dom';
import { request } from './bridge';
import type { FileFilter } from './generated/bridge';

export interface SheetHandle {
  el: HTMLDialogElement;
  body: HTMLElement;
  footer: HTMLElement;
  close(result?: any): void;
  done: Promise<any>;
}

export function sheet(title: string, opts: { wide?: boolean; label?: string } = {}): SheetHandle {
  const body = h('div', { class: 'sheet-body' });
  const footer = h('div', { class: 'sheet-footer' });
  const titleId = `t-${Math.random().toString(36).slice(2)}`;
  const el = h('dialog', { class: `sheet${opts.wide ? ' wide' : ''}`, 'aria-labelledby': titleId }, h('h2', { id: titleId }, title), body, footer) as HTMLDialogElement;
  let resolve!: (v: any) => void;
  const done = new Promise<any>((r) => (resolve = r));
  let result: any = undefined;
  const close = (r?: any) => {
    result = r;
    el.close();
  };
  el.addEventListener('close', () => {
    el.remove();
    resolve(result);
  });
  document.body.appendChild(el);
  queueMicrotask(() => el.showModal());
  return { el, body, footer, close, done };
}

/** A one-line text prompt. Resolves to the text, or null when cancelled. */
export async function prompt(title: string, label: string, value = '', okLabel = 'OK'): Promise<string | null> {
  const s = sheet(title);
  const input = h('input', { type: 'text', value, 'aria-label': label, autofocus: true }) as HTMLInputElement;
  s.body.append(h('label', { class: 'field' }, h('span', null, label), input));
  const ok = () => s.close(input.value.trim() || null);
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      // Keep the Enter from reaching the button that opened the prompt (focus returns to it).
      e.preventDefault();
      e.stopPropagation();
      ok();
    }
  });
  s.footer.append(button('Cancel', () => s.close(null)), button(okLabel, ok, { appearance: 'primary' }));
  setTimeout(() => {
    input.focus();
    input.select();
  }, 30);
  return s.done.then((v) => v ?? null);
}

/** Answers the self test queues for the next native questions (a task dialog would block it). */
export const scriptedAnswers: number[] = [];

/** A question with named buttons, as a native task dialog. Returns the index chosen. */
export function confirm(title: string, message: string, buttons: string[], warning = false): Promise<number> {
  if (scriptedAnswers.length) return Promise.resolve(scriptedAnswers.shift()!);
  return request<number>('dialog.confirm', { title, message, buttons, warning });
}

export async function openFiles(title: string, filters: FileFilter[] = [], multiple = false, defaultPath?: string): Promise<string[]> {
  return request<string[]>('dialog.open', { title, filters, multiple, folders: false, default_path: defaultPath ?? null });
}

export async function openFolders(title: string, multiple = false, defaultPath?: string): Promise<string[]> {
  return request<string[]>('dialog.open', { title, filters: [], multiple, folders: true, default_path: defaultPath ?? null });
}

export async function saveFile(title: string, filters: FileFilter[], defaultName?: string, defaultPath?: string): Promise<string | null> {
  return request<string | null>('dialog.save', { title, filters, default_name: defaultName ?? null, default_path: defaultPath ?? null });
}

export const Filters = {
  project: [{ name: 'Video Studio projects (*.cutproj)', spec: '*.cutproj' }],
  brand: [{ name: 'Brand presets (*.cutbrand)', spec: '*.cutbrand' }],
  markdown: [{ name: 'Markdown (*.md)', spec: '*.md' }],
  zip: [{ name: 'Zip archives (*.zip)', spec: '*.zip' }],
  media: [
    { name: 'Media files', spec: '*.mp4;*.mov;*.m4v;*.webm;*.mkv;*.mp3;*.wav;*.m4a;*.aac;*.flac;*.ogg;*.png;*.jpg;*.jpeg;*.gif;*.webp;*.svg;*.ttf;*.otf;*.json;*.tape;*.json5' },
    { name: 'All files', spec: '*.*' },
  ],
  images: [{ name: 'Images', spec: '*.png;*.jpg;*.jpeg;*.svg;*.webp;*.gif' }],
  movieOrImage: [{ name: 'Video or image', spec: '*.mp4;*.mov;*.webm;*.png;*.jpg;*.jpeg' }],
  fonts: [{ name: 'Fonts', spec: '*.ttf;*.otf' }],
  any: [{ name: 'All files', spec: '*.*' }],
};

/** A transient message at the bottom of the window (also announced by Narrator). */
export function toast(text: string, kind: 'info' | 'error' = 'info') {
  let region = document.getElementById('toasts');
  if (!region) {
    region = h('div', { id: 'toasts', role: 'status', 'aria-live': 'polite' });
    document.body.append(region);
  }
  const t = h('div', { class: `toast ${kind}` }, text);
  region.append(t);
  setTimeout(() => t.remove(), kind === 'error' ? 8000 : 4000);
}
