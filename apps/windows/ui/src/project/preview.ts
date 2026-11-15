// The spec preview (ui/spec.md §5, §23.3): the macOS app's preview page and scripts in an iframe
// (dist/preview, see vite.config.ts), driven with the same `MD.render(payload)` calls, so a spec
// renders the same on both platforms. Media cards get their stills from the media host.

import type { ProjectDocument } from './document';
import { theme } from '../theme';
import { duration as fmtDuration } from '../dom';

const MEDIA_HOST = 'https://media.video-studio.invalid';
const SCRUB_FRAMES = 12;

export interface PreviewHost {
  onTask(line: number): void;
  onMedia(id: string): void;
  onLink(href: string): void;
  onScroll(line: number): void;
}

export class Preview {
  frame: HTMLIFrameElement;
  private ready = false;
  private queued: (() => void)[] = [];
  private renderTimer: number | undefined;
  /** Rendered (after Mermaid) — the self test waits on this. */
  rendered = 0;

  constructor(parent: HTMLElement, private doc: ProjectDocument, private host: PreviewHost) {
    this.frame = document.createElement('iframe');
    this.frame.className = 'preview-frame';
    this.frame.title = 'Spec preview';
    this.frame.setAttribute('aria-label', 'Spec preview');
    this.frame.src = './preview/index.html';
    parent.appendChild(this.frame);
    // the page asks the window for media stills
    (window as any).__vsThumb = (id: string, n: number) => this.thumb(id, n);
    window.addEventListener('message', (e) => {
      if (e.source !== this.frame.contentWindow || !e.data?.__vsPreview) return;
      this.onMessage(e.data.msg);
    });
    theme.subscribe(() => this.render(), false);
  }

  private onMessage(m: any) {
    switch (m?.type) {
      case 'ready':
        this.ready = true;
        for (const f of this.queued.splice(0)) f();
        break;
      case 'rendered':
        this.rendered++;
        break;
      case 'task':
        this.host.onTask(m.line);
        break;
      case 'media':
        this.host.onMedia(m.id);
        break;
      case 'link':
        this.host.onLink(m.href);
        break;
      case 'scroll':
        this.host.onScroll(m.line);
        break;
      case 'jserror':
        console.warn('preview:', m.message);
        break;
    }
  }

  private call(fn: (md: any) => void) {
    const run = () => {
      const md = (this.frame.contentWindow as any)?.MD;
      if (md) fn(md);
    };
    if (this.ready) run();
    else this.queued.push(run);
  }

  /** URL of still `n` of a media item, through the media host (only this project's files). */
  thumb(id: string, n: number): string | null {
    const item = this.doc.p.media.find((m) => m.id === id);
    const res = this.doc.resolution(id);
    if (!item || !res?.path) return null;
    const enc = encodeURIComponent(res.path);
    if (item.kind === 'image') return `${MEDIA_HOST}/file/${encodeURIComponent(item.id)}?path=${enc}`;
    const dur = item.probe?.duration_s ?? 0;
    const frames = dur > 1 ? SCRUB_FRAMES : 1;
    return `${MEDIA_HOST}/thumb?path=${enc}&n=${n}&frames=${frames}&dur=${dur}`;
  }

  private mediaInfo() {
    const out: Record<string, unknown> = {};
    const kindName: Record<string, string> = { video: 'Video', audio: 'Audio', image: 'Image', font: 'Font', transcript: 'Transcript', tape: 'Terminal tape', demo_script: 'Demo script', other: 'File' };
    for (const m of this.doc.p.media) {
      const r = this.doc.resolution(m.id);
      const status = r?.status ?? 'unknown';
      const dur = m.probe?.duration_s;
      const parts = [kindName[m.kind] ?? m.kind];
      if (dur) parts.push(fmtDuration(dur));
      if (m.probe?.width && m.probe?.height) parts.push(`${m.probe.width}×${m.probe.height}`);
      out[m.id] = {
        kind: m.kind,
        name: (m.paths.absolute ?? m.paths.relative ?? m.id).split(/[\\/]/).pop(),
        status,
        meta: parts.join(' · '),
        duration: dur,
        frames: m.kind === 'video' ? (dur && dur > 1 ? SCRUB_FRAMES : 1) : m.kind === 'image' ? 1 : 0,
      };
    }
    return out;
  }

  /** Renders the spec soon (coalesced while typing). */
  render(scrollLine?: number) {
    if (this.renderTimer !== undefined) clearTimeout(this.renderTimer);
    this.renderTimer = window.setTimeout(() => {
      this.renderTimer = undefined;
      const payload = {
        text: this.doc.p.spec,
        theme: theme.value.dark ? 'dark' : 'light',
        showFrontMatter: false,
        customCSS: '',
        scrollLine,
        remoteImages: false,
        islandOutlines: false,
        media: this.mediaInfo(),
      };
      this.call((md) => md.render(payload));
    }, 150);
  }

  scrollToLine(line: number) {
    this.call((md) => md.scrollToLine(line, true));
  }

  /** The preview's document (self test checks). */
  document(): Document | null {
    return this.frame.contentDocument;
  }
}
