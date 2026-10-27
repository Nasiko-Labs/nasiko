/**
 * Initial focus per state (plan §7.7) without stealing it: only when nothing else holds focus
 * (the page just loaded, or the element that had focus was removed by a navigation).
 */
export function focusIfIdle(el: { focus(): void } | null | undefined) {
  if (!el || typeof document === 'undefined') return
  const active = document.activeElement
  if (!active || active === document.body) el.focus()
}
