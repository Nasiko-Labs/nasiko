/** The village's numbers (plans/feat-village-overview.md). */

/** The insight panel's width plus its inset: the part of the canvas Fit and Take me there keep clear. */
export const PANEL_RESERVE = 360 + 28
/** Kiko speaks up this long after the village loads, once per visit, when something is broken. */
export const KIKO_DELAY_MS = 1500
/** A selection eases the view to fit the place and every place its links reach (Ans, 2026-10-17: the fixed close-up
 *  was too much), never closer than this many times the fitted view (the viewport caps it at 4). */
export const SELECT_ZOOM = 2.4
/** A place with no links eases in to this many times the fitted view. */
export const SELECT_ZOOM_ALONE = 1.6
/** A broken or slow piece floats what its failed runs cost once it reaches this much in the range (V56): below it the
 *  badge would be noise ("−$0.03") on a piece whose pose already says something is wrong. */
export const LOST_MIN_USD = 0.5
