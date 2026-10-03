import { useSyncExternalStore } from 'react'

/** Read once, at event time (e.g. just before scrolling); false where matchMedia is missing. */
export function prefersReducedMotion(): boolean {
  return (
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  )
}

/**
 * Live `matchMedia` result. Use it when a hidden subtree must not mount at all (charts
 * measure a 0×0 box under `display: none`); plain responsive layout stays in CSS.
 * Without `matchMedia` (jsdom) it reports `fallback`.
 */
export function useMediaQuery(query: string, fallback = true): boolean {
  return useSyncExternalStore(
    (onChange) => {
      if (typeof window.matchMedia !== 'function') return () => {}
      const mql = window.matchMedia(query)
      mql.addEventListener('change', onChange)
      return () => mql.removeEventListener('change', onChange)
    },
    () => (typeof window.matchMedia === 'function' ? window.matchMedia(query).matches : fallback),
    () => fallback,
  )
}
