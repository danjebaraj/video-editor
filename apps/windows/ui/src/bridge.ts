// The UI side of the bridge (ui/spec.md §23.3): requests with ids, their responses, and events
// from the shell, over WebView2's postMessage. Types come from src/generated (Rust is the source
// of truth). Outside WebView2 (a plain browser during development) every request fails clearly.

import type { Method, Topic, ShellInfo, HostError } from './generated/bridge';

type Pending = { resolve: (v: any) => void; reject: (e: BridgeError) => void };

export class BridgeError extends Error {
  code: string;
  constructor(e: HostError) {
    super(e.message);
    this.code = e.code;
  }
}

interface WebView {
  postMessage(msg: unknown): void;
  addEventListener(type: 'message', fn: (e: MessageEvent) => void): void;
}

const webview: WebView | undefined = (window as any).chrome?.webview;
const pending = new Map<number, Pending>();
const listeners = new Map<Topic, Set<(data: any) => void>>();
const backlog = new Map<Topic, any[]>();
let nextId = 1;

if (webview) {
  webview.addEventListener('message', (e: MessageEvent) => {
    const m = e.data;
    if (!m || typeof m !== 'object') return;
    if (m.kind === 'response') {
      const p = pending.get(m.id);
      if (!p) return;
      pending.delete(m.id);
      if (m.ok) p.resolve(m.result);
      else p.reject(new BridgeError(m.error ?? { code: 'shell', message: 'failed' }));
    } else if (m.kind === 'event') {
      const subs = listeners.get(m.topic);
      if (!subs || subs.size === 0) {
        // Nobody listens yet (the page is still opening the project): keep it for the first one.
        const b = backlog.get(m.topic) ?? [];
        if (b.length < 50) b.push(m.data);
        backlog.set(m.topic, b);
        return;
      }
      for (const fn of subs) {
        try {
          fn(m.data);
        } catch (err) {
          console.error(err);
        }
      }
    }
  });
}

export const inApp = !!webview;

/** Tells the shell the page is ready for events. */
export function ready() {
  webview?.postMessage({ kind: 'ready' });
}

export function request<T = any>(method: Method, args: unknown = {}): Promise<T> {
  if (!webview) return Promise.reject(new BridgeError({ code: 'shell', message: 'not running in the app' }));
  const id = nextId++;
  return new Promise<T>((resolve, reject) => {
    pending.set(id, { resolve, reject });
    webview.postMessage({ id, method, args });
  });
}

/** One core operation, as the macOS shell calls `call(method, args)`. */
export function core<T = any>(method: string, args: Record<string, unknown> = {}): Promise<T> {
  return request<T>('core', { method, args });
}

export function on(topic: Topic, fn: (data: any) => void): () => void {
  let set = listeners.get(topic);
  if (!set) listeners.set(topic, (set = new Set()));
  set.add(fn);
  const waiting = backlog.get(topic);
  if (waiting?.length) {
    backlog.delete(topic);
    queueMicrotask(() => waiting.forEach((d) => fn(d)));
  }
  return () => set!.delete(fn);
}

let info: ShellInfo | undefined;
export async function shellInfo(): Promise<ShellInfo> {
  if (!info) info = await request<ShellInfo>('shell.info');
  return info;
}

export function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
