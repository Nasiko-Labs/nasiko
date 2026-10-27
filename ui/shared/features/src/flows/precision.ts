/**
 * Never more precision than the data holds (plans/feat-flows.md F17). A recorded step with only `latency_ms` is
 * whole seconds (FL-4): it reads "<1 s" / "2 s"; a span or a `completed_at` end is exact.
 */
import type { Call } from './calls'

const SEC = 1_000
const MIN = 60 * SEC

export function fmtDuration(ms: number | null, exact = true): string {
  if (ms === null || Number.isNaN(ms)) return '—'
  const v = Math.max(0, ms)
  if (v >= MIN) {
    const m = Math.floor(v / MIN)
    const s = Math.floor((v % MIN) / SEC)
    return s ? `${m} min ${s} s` : `${m} min`
  }
  if (!exact) return v < SEC ? '<1 s' : `${Math.floor(v / SEC)} s`
  return v < SEC ? `${Math.round(v)} ms` : `${(v / SEC).toFixed(1)} s`
}

/** A human wait, in whole minutes: the page re-reads a paused flow every 15 s, so seconds would always be stale. */
export function fmtWait(ms: number): string {
  const m = Math.floor(Math.max(0, ms) / MIN)
  if (m < 1) return '<1 min'
  const h = Math.floor(m / 60)
  return h ? (m % 60 ? `${h} h ${m % 60} min` : `${h} h`) : `${m} min`
}

/** Whether any closed call's timing is whole seconds: the panel then says "Timing to the nearest second". */
export const hasWholeSeconds = (calls: readonly Call[]) =>
  calls.some((c) => c.endMs !== null && !c.exactEnd)
