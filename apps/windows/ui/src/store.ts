// Minimal observable state: views subscribe and re-render when a value changes.

export class Signal<T> {
  private v: T;
  private subs = new Set<(v: T) => void>();
  constructor(v: T) {
    this.v = v;
  }
  get value(): T {
    return this.v;
  }
  set value(v: T) {
    this.v = v;
    this.emit();
  }
  /** Mutates in place, then notifies. */
  update(fn: (v: T) => void) {
    fn(this.v);
    this.emit();
  }
  emit() {
    for (const s of [...this.subs]) s(this.v);
  }
  subscribe(fn: (v: T) => void, now = true): () => void {
    this.subs.add(fn);
    if (now) fn(this.v);
    return () => this.subs.delete(fn);
  }
}

/** Re-runs `fn` (coalesced to one animation frame) whenever any signal changes. */
export function effect(signals: Signal<any>[], fn: () => void): () => void {
  let queued = false;
  const run = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      fn();
    });
  };
  const offs = signals.map((s) => s.subscribe(run, false));
  fn();
  return () => offs.forEach((o) => o());
}
