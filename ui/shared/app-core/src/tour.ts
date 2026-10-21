// Ported from the legacy ui/ on development (3aac84f66) common/src/lib/tour.ts (dd9fe0f64). Local edits: optional
// fields admit `undefined`; Prettier formatting.
/**
 * The top bar's tour button asks for a tour; the tour feature (`@nasiko/features/tour`) answers with the current
 * page's tour. Here so the shell can ask without importing a feature, as `draftKeys.ts` does for sign-out. The step
 * type lives here too, so a layer's tours (the `tours` slot, `edition/edition.ts`) can be typed without the feature.
 */
import { useSyncExternalStore } from "react";

/**
 * One step of a tour. A `target` points at `[data-tour~="<target>"]` and is shown only while that element is on screen
 * when the tour opens; a step without one is a centred card (every tour opens with one).
 */
export interface TourStep {
  target?: string | undefined;
  title: string;
  body: string;
  /** Where the card sits next to the target (it moves when there is no room). Default: below. */
  side?: "top" | "right" | "bottom" | "left" | undefined;
}

let asks = 0;
const listeners = new Set<() => void>();
const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
};

export function askForTour() {
  asks += 1;
  for (const fn of listeners) fn();
}

/** How many times a tour was asked for: a change is a new ask. */
export const useTourAsks = () =>
  useSyncExternalStore(
    subscribe,
    () => asks,
    () => asks,
  );
