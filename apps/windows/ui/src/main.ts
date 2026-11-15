// Window content entry point: one page for every window kind, chosen by the shell.

import { ready, shellInfo, inApp, on, request } from './bridge';
import { startTheme } from './theme';
import { reloadLibrary, detect } from './app-state';
import { h, clear } from './dom';

async function boot() {
  const app = document.getElementById('app')!;
  if (!inApp) {
    clear(app, h('div', { class: 'empty' }, h('div', { class: 'empty-title' }, 'Syncfusion Video Studio'), h('p', null, 'This page runs inside the Windows app.')));
    return;
  }
  ready();
  const info = await shellInfo();
  startTheme(info.theme);
  try {
    await reloadLibrary();
  } catch (e) {
    clear(app, h('div', { class: 'empty' }, h('div', { class: 'empty-title' }, 'The app library couldn’t be read'), h('p', null, String(e))));
    return;
  }
  detect(true).catch(() => {});
  // Windows that aren't projects close as soon as they're asked to.
  if (info.window_kind !== 'project') {
    on('close_requested', () => request('shell.close_window'));
  }
  switch (info.window_kind) {
    case 'start':
      (await import('./start/start')).mount(app);
      break;
    case 'project':
      (await import('./project/window')).mount(app, info);
      break;
    case 'library':
      (await import('./library/library')).mount(app);
      break;
    case 'settings':
      (await import('./settings/settings')).mount(app);
      break;
  }
}

window.addEventListener('error', (e) => console.error(e.error ?? e.message));
boot();
