// Fluent design (ui/spec.md §23.6): Fluent UI web components with the Fluent 2 web themes,
// the Windows accent color, light/dark following the system, and high-contrast support.

import '@fluentui/web-components/button/define.js';
import '@fluentui/web-components/spinner/define.js';
import '@fluentui/web-components/progress-bar/define.js';
import '@fluentui/web-components/badge/define.js';
import '@fluentui/web-components/message-bar/define.js';
import '@fluentui/web-components/switch/define.js';
import '@fluentui/web-components/divider/define.js';
import '@fluentui/web-components/tooltip/define.js';
import { setTheme } from '@fluentui/web-components/theme/set-theme.js';
import { webDarkTheme, webLightTheme } from '@fluentui/tokens';
import { on } from './bridge';
import type { ThemeInfo } from './generated/bridge';
import { Signal } from './store';

export const theme = new Signal<ThemeInfo>({ dark: false, high_contrast: false, accent: '#0067c0', scale: 1, mica: false });

function hexToRgb(hex: string): [number, number, number] {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  return m ? [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)] : [0, 103, 192];
}

function shade(hex: string, f: number): string {
  const [r, g, b] = hexToRgb(hex);
  const mix = (c: number) => Math.round(f < 0 ? c * (1 + f) : c + (255 - c) * f);
  return `rgb(${mix(r)}, ${mix(g)}, ${mix(b)})`;
}

export function apply(t: ThemeInfo) {
  const root = document.documentElement;
  root.dataset.theme = t.dark ? 'dark' : 'light';
  root.dataset.mica = String(!!t.mica);
  root.classList.toggle('high-contrast', t.high_contrast);
  const base = { ...(t.dark ? webDarkTheme : webLightTheme) } as unknown as Record<string, string>;
  // The Windows accent color drives the brand ramp the components use.
  if (!t.high_contrast) {
    base.colorBrandBackground = t.dark ? shade(t.accent, 0.25) : t.accent;
    base.colorBrandBackgroundHover = t.dark ? shade(t.accent, 0.35) : shade(t.accent, -0.12);
    base.colorBrandBackgroundPressed = t.dark ? shade(t.accent, 0.15) : shade(t.accent, -0.24);
    base.colorBrandForeground1 = t.dark ? shade(t.accent, 0.45) : shade(t.accent, -0.1);
    base.colorBrandForegroundLink = base.colorBrandForeground1;
    base.colorBrandStroke1 = t.accent;
    base.colorCompoundBrandStroke = t.accent;
    base.colorCompoundBrandBackground = t.accent;
  }
  setTheme(base as any);
  root.style.setProperty('--accent', t.dark ? shade(t.accent, 0.35) : t.accent);
  root.style.setProperty('--accent-rgb', hexToRgb(t.accent).join(', '));
  theme.value = t;
}

export function startTheme(initial: ThemeInfo) {
  apply(initial);
  on('theme', (t: ThemeInfo) => apply(t));
  // Windows high contrast also reaches the page as forced colors.
  matchMedia('(forced-colors: active)').addEventListener('change', (e) => apply({ ...theme.value, high_contrast: e.matches }));
}
