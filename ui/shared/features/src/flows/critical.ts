/**
 * The critical path (plans/feat-flows.md F22): the chain of calls that decided when the flow ended, following parent
 * links. Among siblings it starts at the one that ended last and steps back to the sibling that ended before that one
 * started (the call it waited for); each call on the chain adds its own children the same way, bounded by its end.
 *
 *   writer ◀── research ◀── (inside research) sql-analyst ◀── the slowest search
 *
 * Drawn only when it's proven: every call closed with an exact end (F17), at least one parent link known (else there
 * is no chain, only a sequence), and the chain doesn't mark every call (then it says nothing).
 */
import type { Call } from './calls'

export type CriticalPath =
  | { keys: ReadonlySet<string>; reason: null }
  | { keys: null; reason: 'timing' | 'parents' | 'everything' }

/** The sibling that ended last, no later than `bound`. */
const lastEnded = (calls: readonly Call[], bound: number): Call | undefined =>
  calls.reduce<Call | undefined>((best, c) => {
    const end = c.endMs ?? 0
    if (end > bound) return best
    const bestEnd = best?.endMs ?? 0
    return !best || end > bestEnd || (end === bestEnd && c.key < best.key) ? c : best
  }, undefined)

export function criticalPath(calls: readonly Call[]): CriticalPath {
  if (calls.some((c) => c.endMs === null || !c.exactEnd)) return { keys: null, reason: 'timing' }
  const keys = new Set(calls.map((c) => c.key))
  const hasParent = (c: Call) => !!c.parentKey && keys.has(c.parentKey)
  if (!calls.some(hasParent)) return { keys: null, reason: 'parents' }

  const chain = new Set<string>()
  const walk = (siblings: readonly Call[], bound: number) => {
    let at = lastEnded(siblings, bound)
    while (at && !chain.has(at.key)) {
      chain.add(at.key)
      const parent = at.key
      walk(
        calls.filter((c) => c.parentKey === parent),
        at.endMs ?? 0,
      )
      at = lastEnded(siblings, at.startMs)
    }
  }
  walk(
    calls.filter((c) => !hasParent(c)),
    Number.POSITIVE_INFINITY,
  )
  if (chain.size >= calls.length) return { keys: null, reason: 'everything' }
  return { keys: chain, reason: null }
}
