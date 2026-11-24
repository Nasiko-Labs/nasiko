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
  /** The tour's spotlight gliding from one target to the next: damped, so a long move settles without bouncing. */
  spotlight: { type: 'spring', stiffness: 320, damping: 34 },
  /** The stepper (composites/stepper.tsx), shadcn-space's stepper-03 values: the progress track filling, the dot
   *  leading it, the current step's lift, and a step's icon popping in (a check spins in once a step is done). */
  stepTrack: { type: 'spring', stiffness: 120, damping: 20 },
  stepDot: { type: 'spring', stiffness: 160, damping: 24 },
  stepLift: { type: 'spring', stiffness: 320, damping: 18 },
  stepIcon: { type: 'spring', stiffness: 400, damping: 22 },
  /** The current step's ring breathing out and back, 2.2 s each way; never under reduced motion. */
  stepPulse: { duration: 2.2, repeat: Infinity, repeatType: 'mirror', ease: 'easeInOut' },
} as const

/** Row entrances: rise 6 px (new live rows drop 8 px), 30 ms stagger, `standard` timing. */
export const enter = {
  transition: transitions.disclosure,
  rise: 6,
  drop: -8,
  stagger: 0.03,
  /** Words of a generated line (the tour's titles, Aceternity's Text Generate Effect), one after another. */
  wordStagger: 0.06,
} as const
