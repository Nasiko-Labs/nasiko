/**
 * The village canvas's frame, shared by the canvas and its loading state, so one replaces the other without moving
 * anything on the page.
 */

/** The frame's box: a rounded card with the canvas's dot grid. */
export const FRAME =
  'relative overflow-hidden rounded-xl border bg-card bg-[radial-gradient(var(--border)_1.1px,transparent_1.2px)] [background-size:22px_22px]'

/** The canvas fills the window below its top edge (the prototype's frame), between 560 and 940 px. */
export function fillHeight(el: Element): number {
  const room = window.innerHeight - el.getBoundingClientRect().top - 16
  return Math.round(Math.min(940, Math.max(560, Number.isFinite(room) ? room : 680)))
}
