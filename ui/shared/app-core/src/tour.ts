/**
 * The top bar's tour button asks for a tour; the tour feature (`features/tour`) answers with the current page's
 * tour. Here so the shell can ask without importing a feature, as `draftKeys.ts` does for sign-out. The step type
 * lives here too, so a layer's tours (the `tours` slot, `app/edition.ts`) can be typed without the feature.
 */
import { useSyncExternalStore } from 'react'

/**
 * One step of a tour. A `target` points at `[data-tour~="<target>"]` and is shown only while that element is on screen
 * when the tour opens; a step without one is a centred card (every tour opens with one).
 */
export interface TourStep {
  target?: string
  title: string
  body: string
  /** Where the card sits next to the target (it moves when there is no room). Default: below. */
  side?: 'top' | 'right' | 'bottom' | 'left'
}

/** The latest ask: a new object per ask, so a change is a new ask. `tour` names one (the account menu's sidebar
 *  tour); null is the current page's. */
export interface TourAsk {
  n: number
  tour: string | null
}
let ask: TourAsk = { n: 0, tour: null }
const listeners = new Set<() => void>()
const subscribe = (fn: () => void) => {
  listeners.add(fn)
  return () => void listeners.delete(fn)
}

export function askForTour(tour: string | null = null) {
  ask = { n: ask.n + 1, tour }
  for (const fn of listeners) fn()
}

export const useTourAsks = () =>
  useSyncExternalStore(
    subscribe,
    () => ask,
    () => ask,
  )
