// The controls for a finish (the macOS FinishViews): how shots join, the beat, eased text and
// close-ups, and how the music comes and goes. Used for Settings › Finish and Project › Finish.

import { h, select, checkbox } from './dom';
import type { Finish } from './types';

export const DEFAULT_FINISH: Finish = {
  transitions: 'smooth',
  transition_s: 0.4,
  cut_on_beat: true,
  lead_in_s: 1.2,
  tail_s: 2.0,
  music_fade_in_s: 1.0,
  music_fade_out_s: 2.0,
  ease_text: true,
  guide_the_eye: true,
};

/** A finish with every key present: a missing key keeps its default, as in the core. */
export function filledFinish(f?: Partial<Finish> | null): Finish {
  return { ...DEFAULT_FINISH, ...(f ?? {}) } as Finish;
}

export function sameFinish(a?: Partial<Finish> | null, b?: Partial<Finish> | null): boolean {
  const x = filledFinish(a);
  const y = filledFinish(b);
  return (Object.keys(DEFAULT_FINISH) as (keyof Finish)[]).every((k) => x[k] === y[k]);
}

/** "0.4 s", "2 s". */
export function seconds(v: number): string {
  const s = v.toFixed(1);
  return `${s.endsWith('.0') ? s.slice(0, -2) : s} s`;
}

/** One line describing the choices, for the project inspector. */
export function finishSummary(f: Finish): string {
  const join = f.transitions === 'cuts' ? 'Plain cuts' : f.transitions === 'lively' ? 'Lively transitions' : 'Smooth transitions';
  return `${join}, ${f.cut_on_beat ? 'cuts on the beat' : 'cuts not tied to the beat'}, music in over ${seconds(f.music_fade_in_s)} and out over ${seconds(f.music_fade_out_s)}.`;
}

const TRANSITION_NOTE: Record<string, string> = {
  cuts: 'Every scene change is a plain cut. The title and the end card still fade.',
  lively: 'Scene changes use quick moving transitions (whips and pushes). Best for short social videos.',
  smooth: 'Scene changes use a short soft move; views of the same screen are joined with plain cuts, so text never ghosts.',
};

/** A seconds value with a stepper (a number input stepping by `step`, clamped to the range). */
function secondsRow(label: string, value: number, min: number, max: number, step: number, help: string, onChange: (v: number) => void): HTMLElement {
  const shown = h('span', { class: 'muted num' }, seconds(value));
  const input = h('input', { type: 'number', min, max, step, value: String(value), 'aria-label': label, class: 'stepper' }) as HTMLInputElement;
  input.addEventListener('change', () => {
    const v = Number(input.value);
    if (!Number.isFinite(v)) return;
    const c = Math.min(max, Math.max(min, Math.round(v / step) * step));
    const rounded = Math.round(c * 100) / 100;
    input.value = String(rounded);
    shown.textContent = seconds(rounded);
    onChange(rounded);
  });
  return h('div', { class: 'row finish-seconds', title: help }, h('span', { class: 'grow' }, label), shown, input);
}

/** `change` gets the whole edited finish. `compact` is the narrow inspector layout. */
export function finishEditor(value: Finish, change: (f: Finish) => void, opts: { compact?: boolean } = {}): HTMLElement {
  const f = filledFinish(value);
  const set = <K extends keyof Finish>(k: K, v: Finish[K]) => change({ ...f, [k]: v });
  const transitions = select(
    [['smooth', 'Smooth'], ['cuts', 'Plain cuts'], ['lively', 'Lively']],
    f.transitions,
    (v) => set('transitions', v as Finish['transitions']),
    { 'aria-label': 'Transitions between scenes', title: 'Smooth: a short soft move between scenes and plain cuts inside a scene. Plain cuts: no transitions except a fade for the title and end card. Lively: quick moving transitions, for social cuts.' },
  );
  return h(
    'div',
    { class: `finish-editor${opts.compact ? ' compact' : ''}` },
    h('label', { class: 'field' }, h('span', null, 'Between scenes'), transitions),
    h('p', { class: 'hint' }, TRANSITION_NOTE[f.transitions] ?? TRANSITION_NOTE.smooth),
    f.transitions !== 'cuts'
      ? secondsRow('Transition length', f.transition_s, 0.15, 1.5, 0.05, 'How long a transition between two scenes takes. 0.3 to 0.5 seconds reads as smooth without slowing the video.', (v) => set('transition_s', v))
      : null,
    checkbox('Cut on the music’s beat', f.cut_on_beat, (v) => set('cut_on_beat', v), { title: 'Cuts timed to the narration move onto the nearest beat, scene changes go to the start of a bar where one is close, and generated music is asked for with a steady beat that can be followed.' }),
    checkbox('Ease on-screen text in and out', f.ease_text, (v) => set('ease_text', v), { title: 'Text fades or slides in shortly after a cut and leaves before the next one, instead of popping on.' }),
    checkbox(opts.compact ? 'Ease into what is being talked about' : 'Ease into the part of the screen being talked about', f.guide_the_eye, (v) => set('guide_the_eye', v), { title: 'On screen recordings, a gentle zoom toward what the narration is describing, and back out before the cut.' }),
    h('hr'),
    secondsRow('Before the first word', f.lead_in_s, 0, 5, 0.1, 'Picture and music start first; the voice comes in after this long. About a second keeps the video from starting on a cold word.', (v) => set('lead_in_s', v)),
    secondsRow('After the last word', f.tail_s, 0, 8, 0.25, 'How long the end card holds after the voice stops, so the video doesn’t end on the last syllable.', (v) => set('tail_s', v)),
    secondsRow('Music fades in over', f.music_fade_in_s, 0, 5, 0.25, 'How long the music takes to come in at the start.', (v) => set('music_fade_in_s', v)),
    secondsRow('Music fades out over', f.music_fade_out_s, 0, 8, 0.25, 'Used when the track has no ending of its own to land on the end of the video. A track with a real ending closes the video on it instead.', (v) => set('music_fade_out_s', v)),
  );
}
