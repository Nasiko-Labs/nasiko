/**
 * The app's motion presets (plans/feat-app-shell.md §5.4). New motion uses these names, never raw
 * numbers. CSS-driven motion (the sidebar width, the shadcn sheets) can't import them, so
 * `motion.test.ts` checks those class strings against the values here.
 *
 * The existing inline timings in Sessions, TokenOps, Harnesses and the trace page stay where they
 * are for now (eng D12, TODOS.md); `standard` and `enter` pin their values so moving them later
 * changes nothing on screen (eng D5).
 */

/** Milliseconds. */
export const durations = {
  /** Menus, popovers, tooltips (tw-animate-css default). */
  fast: 150,
  /** Sidebar collapse / expand. */
  base: 180,
  /** Today's disclosures, row entrances and fades (0.2 s). */
  standard: 200,
  /** Sheets and side panels opening. */
  panelIn: 220,
  /** Sheets and side panels closing. */
  panelOut: 160,
  /** The login showcase's headline: how long each word stays before the next one (the prototype's 2.8 s). */
  wordHold: 2800,
} as const

export const ease = 'easeOut' as const

const s = (ms: number) => ms / 1000

/** Motion `transition` objects. */
export const transitions = {
  collapse: { duration: s(durations.base), ease },
  fade: { duration: s(durations.fast), ease },
  panelIn: { duration: s(durations.panelIn), ease },
  panelOut: { duration: s(durations.panelOut), ease },
  disclosure: { duration: s(durations.standard), ease },
  /** Shared-layout morphs (`layoutId`) keep Motion's default transition. */
  morph: {},
} as const

/** Row entrances: rise 6 px (new live rows drop 8 px), 30 ms stagger, `standard` timing. */
export const enter = {
  transition: transitions.disclosure,
  rise: 6,
  drop: -8,
  stagger: 0.03,
} as const
