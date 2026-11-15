// Inspector › Project (spec §7, §4.2, §23.7): name, intent, models, App recording and the
// project's media search folders.

import { h, clear, field, textInput, numberInput, select, checkbox, button, iconButton, Icons } from '../../dom';
import { core } from '../../bridge';
import { effect } from '../../store';
import { platforms } from '../../app-state';
import { openFolders } from '../../dialogs';
import { modelPicker, effortPicker } from '../../widgets';
import { INTENTS } from '../../types';
import type { ProjectWindow } from '../window';

interface CapturePermissions {
  supported: boolean;
  screen_recording: boolean;
  accessibility: boolean;
}

export function projectTab(w: ProjectWindow): HTMLElement {
  const root = h('div', { class: 'form-tab' });
  const d = w.doc;
  let perms: CapturePermissions | null = null;
  const refreshPerms = () => core<CapturePermissions>('capture.permissions', { request: false }).then((p) => ((perms = p), d.project.emit())).catch(() => {});
  refreshPerms();

  effect([d.project, d.path], () => {
    const p = d.p;
    const defaults = platforms.value?.intents[p.intent.kind];
    const capture = p.app_capture ?? { enabled: true, apps: [] };
    clear(
      root,
      h('section', null,
        h('h3', null, 'Project'),
        field('Name', textInput(p.name, (v) => {
          d.update((x) => (x.name = v));
          d.title();
        })),
        h('div', { class: 'path small muted', title: d.path.value ?? '' }, d.path.value ?? 'Not saved yet'),
      ),
      h('section', null,
        h('h3', null, 'Intent'),
        field('Intent', select(p.intent.kind === 'unset' ? [['unset', 'Choose…'], ...INTENTS] : INTENTS, p.intent.kind, (v) => d.update((x) => (x.intent.kind = v as any))), p.intent.kind === 'unset' ? 'Required: the intent sets defaults and tells Claude what the video is for.' : undefined),
        p.intent.kind === 'other' ? field('Describe the intent', textInput(p.intent.other ?? '', (v) => d.update((x) => (x.intent.other = v || null)))) : null,
        field('Audience', textInput(p.intent.audience ?? '', (v) => d.update((x) => (x.intent.audience = v || null)))),
        field('Tone', textInput(p.intent.tone ?? '', (v) => d.update((x) => (x.intent.tone = v || null)), { placeholder: 'confident, friendly, technical…' })),
        field('Target length (seconds)', numberInput(p.intent.target_length_s ?? null, (v) => d.update((x) => (x.intent.target_length_s = v ? v : null)), { min: 0 })),
        field('Call to action', textInput(p.intent.cta ?? '', (v) => d.update((x) => (x.intent.cta = v || null)))),
        field('Languages', textInput((p.intent.locales ?? []).join(', '), (v) => d.update((x) => (x.intent.locales = v.split(',').map((s) => s.trim()).filter(Boolean)))), 'Comma-separated, for example en, de'),
        defaults ? h('div', { class: 'hint' }, `Defaults for this intent: ${defaults.pacing} pacing, ${defaults.captions} captions, ${defaults.music_level} music.`) : null,
      ),
      h('section', null,
        h('h3', null, 'Models'),
        field('Spec assistant', modelPicker(p.models.assistant, (v) => d.update((x) => (x.models.assistant = v), { outputs: false }), { label: 'Spec assistant model' })),
        field('Dry run', modelPicker(p.models.dry_run, (v) => d.update((x) => (x.models.dry_run = v), { outputs: false }), { label: 'Dry run model' })),
        (() => {
          const e = effortPicker(p.models.effort ?? null, (v) => d.update((x) => (x.models.effort = v), { outputs: false }), 'Assistant effort');
          return e ? field('Effort', e) : null;
        })(),
        h('div', { class: 'hint' }, 'The production model is set per run config, in Outputs.'),
      ),
      h('section', null,
        h('h3', null, 'App recording'),
        checkbox('Let Claude drive and record apps', capture.enabled, (v) => d.update((x) => (x.app_capture = { ...capture, enabled: v }), { outputs: false })),
        capture.enabled ? field('Apps', textInput((capture.apps ?? []).join(', '), (v) => d.update((x) => (x.app_capture = { ...capture, apps: v.split(',').map((s) => s.trim()).filter(Boolean) }), { outputs: false }), { placeholder: 'Any app' }), 'Leave empty for any app except terminals, Settings, Registry Editor, Task Manager, File Explorer and password managers.') : null,
        capture.enabled
          ? perms
            ? perms.supported
              ? h('ul', { class: 'perm-list small' },
                  h('li', null, perms.screen_recording ? '✓ Windows graphics capture is available.' : '✗ Windows graphics capture isn’t available on this system.'),
                  h('li', null, perms.accessibility ? '✓ Input and UI Automation are available. Apps running as administrator can’t be driven from a normal app.' : '✗ Input to other apps isn’t available.'),
                  h('li', null, button('Re-check', () => refreshPerms(), { appearance: 'subtle' })))
              : h('p', { class: 'small muted' }, 'Driving apps isn’t available on this system yet.')
            : h('p', { class: 'small muted' }, 'Checking…')
          : null,
      ),
      h('section', null,
        h('h3', null, 'Media search folders'),
        h('p', { class: 'hint' }, 'When a file has moved, these folders are searched by name and fingerprint.'),
        h('ul', { class: 'plain-list' }, (p.media_roots ?? []).map((r, i) => h('li', { class: 'row' }, h('span', { class: 'grow path small', title: r }, r), iconButton(Icons.remove, `Remove ${r}`, () => d.update((x) => x.media_roots.splice(i, 1)))))),
        button('Add Folder…', async () => {
          const dirs = await openFolders('Media search folder', true);
          if (dirs.length) {
            d.update((x) => (x.media_roots = [...new Set([...(x.media_roots ?? []), ...dirs])]));
            d.resolveMedia(false);
          }
        }, { glyph: Icons.add }),
      ),
    );
  });
  return root;
}
