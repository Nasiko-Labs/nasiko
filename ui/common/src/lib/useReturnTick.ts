import { useEffect, useState } from 'react'

/**
 * A counter that goes up when the user comes back to the page (tab shown again, or the
 * window focused) and at least `minAgeMs` have passed since it last went up (or since
 * mount). Pages that freeze "now" (no polling) key it on this, so a page left open over
 * lunch, across midnight, or on a second monitor moves its window forward as one unit
 * when the user returns, instead of mixing stale client bounds with fresh server data.
 */
export function useReturnTick(minAgeMs = 60_000): number {
  const [tick, setTick] = useState(0)
  useEffect(() => {
    let since = Date.now()
    const onReturn = () => {
      if (document.visibilityState === 'hidden') return
      if (Date.now() - since < minAgeMs) return
      since = Date.now()
      setTick((t) => t + 1)
    }
    document.addEventListener('visibilitychange', onReturn)
    window.addEventListener('focus', onReturn)
    return () => {
      document.removeEventListener('visibilitychange', onReturn)
      window.removeEventListener('focus', onReturn)
    }
  }, [minAgeMs])
  return tick
}

/**
 * "Now", frozen until one of `triggers` changes (TokenOps, Harnesses, Session trace). The page's
 * window is computed from it, so it never drifts between renders; pass `useReturnTick()` among the
 * triggers to move it on return.
 */
export function useFrozenNow(...triggers: unknown[]): Date {
  // State, not useMemo: the Compiler needs a literal deps array, and the triggers are variadic.
  const [frozen, setFrozen] = useState(() => ({ triggers, now: new Date() }))
  const moved =
    triggers.length !== frozen.triggers.length ||
    triggers.some((t, i) => !Object.is(t, frozen.triggers[i]))
  if (!moved) return frozen.now
  // A trigger changed: re-freeze during render (React's "adjust state on prop change" pattern).
  const next = { triggers, now: new Date() }
  setFrozen(next)
  return next.now
}
