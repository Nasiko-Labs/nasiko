/**
 * The canvas frame (village and Map), shared by the canvas and its loading state, so one replaces the other
 * without moving anything on the page.
 */

/** The frame's box: a rounded card with the canvas's dot grid. */
export const FRAME =
  'relative overflow-hidden rounded-lg border bg-card bg-[radial-gradient(var(--border)_1.1px,transparent_1.2px)] [background-size:22px_22px]'

/**
 * The canvas fills the window below its top edge down to the page's bottom padding (Ans, 2026-11-04: "fill the
 * page"), however tall the window is; never under 560 px, so a short window scrolls instead of squashing the board.
 */
export function fillHeight(el: Element): number {
  const room = window.innerHeight - el.getBoundingClientRect().top - 12
  return Math.round(Math.max(560, Number.isFinite(room) ? room : 680))
}
