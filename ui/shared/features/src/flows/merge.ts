/**
 * Recorded steps + calls from the trace (plans/feat-flows.md F14, eng review A5, O3, O3a).
 *
 * A recorded step and a trace call are the same call when they have the same agent id and start within
 * MATCH_WINDOW_MS. Candidates form groups (a step matches two spans, a span two steps…); a group merges only when it is
 * exactly one step and one span (O3a). Any other group stays unmerged and every call in it is marked `maybeSame`: two calls to one agent a moment apart
 * must never trade outputs or failures. A merged call keeps the step's key, status and summaries and takes the span's
 * timing and id; trace calls whose parent was merged are re-pointed at the step's key.
 *
 *   steps  ──┐                    ┌── one step, one span ─────────> merged (source 'both')
 *            ├─ same agent, ±1 s ─┤
 *   spans  ──┘                    └── otherwise ──────────────────> unmerged, maybeSame
 */
import type { Call } from './calls'
import { MATCH_WINDOW_MS } from './tuning'

const byStart = (a: Call, b: Call) => a.startMs - b.startMs || a.key.localeCompare(b.key)

function combine(step: Call, span: Call): Call {
  // A call waiting on a human stays open: its transport (the proxy span) can end while the person hasn't answered.
  const waiting = step.status === 'awaiting_human'
  return {
    ...step,
    startMs: span.startMs,
    // A recorded end that is still open defers to the span; a closed span's end is exact.
    endMs: waiting ? null : (span.endMs ?? step.endMs),
    exactEnd: waiting ? step.exactEnd : span.endMs !== null || step.exactEnd,
    spanId: span.spanId,
    source: 'both',
    tokens: step.tokens ?? span.tokens,
    // The recorded parent wins (FL-3); until then the trace's.
    parentKey: step.parentKey ?? span.parentKey,
  }
}

export function mergeCalls(
  recorded: readonly Call[],
  traced: readonly Call[],
  windowMs = MATCH_WINDOW_MS,
): Call[] {
  const merged = new Map<string, Call>()
  /** span key → the step key it merged into */
  const renamed = new Map<string, string>()
  const ambiguous = new Set<string>()

  const agents = new Set(recorded.flatMap((c) => (c.agentId ? [c.agentId] : [])))
  for (const agent of agents) {
    const steps = recorded.filter((c) => c.agentId === agent).sort(byStart)
    const spans = traced.filter((c) => c.agentId === agent).sort(byStart)
    const near = (a: Call, b: Call) => Math.abs(a.startMs - b.startMs) <= windowMs

    // Connected groups of candidates (a union over the step-span edges).
    const seen = new Set<string>()
    for (const first of steps) {
      if (seen.has(first.key)) continue
      const groupSteps: Call[] = []
      const groupSpans: Call[] = []
      const queue: Call[] = [first]
      seen.add(first.key)
      for (let c = queue.shift(); c; c = queue.shift()) {
        const isStep = c.source === 'recorded'
        ;(isStep ? groupSteps : groupSpans).push(c)
        for (const other of isStep ? spans : steps)
          if (!seen.has(other.key) && near(c, other)) {
            seen.add(other.key)
            queue.push(other)
          }
      }
      if (!groupSpans.length) continue
      // O3a (user, 2026-10-06): only a one-to-one group merges. Start order can't prove which of two same-agent
      // calls is which (steps A/B at 100/200 ms, spans B/A at 210/300 ms would pair A with B's trace).
      const step = groupSteps[0]
      const span = groupSpans[0]
      if (groupSteps.length === 1 && groupSpans.length === 1 && step && span) {
        merged.set(step.key, combine(step, span))
        renamed.set(span.key, step.key)
      } else {
        for (const c of [...groupSteps, ...groupSpans]) ambiguous.add(c.key)
      }
    }
  }

  const repoint = (c: Call): Call => {
    const to = c.parentKey ? renamed.get(c.parentKey) : undefined
    return to ? { ...c, parentKey: to } : c
  }
  const mark = (c: Call): Call => (ambiguous.has(c.key) ? { ...c, maybeSame: true } : c)

  return [
    ...recorded.map((c) => merged.get(c.key) ?? c),
    ...traced.filter((c) => !renamed.has(c.key)),
  ]
    .map((c) => mark(repoint(c)))
    .sort(byStart)
}
