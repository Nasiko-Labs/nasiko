/**
 * The one- or two-sentence answer under a flow's title (plans/feat-flows.md F5): why it was slow, where it failed or
 * waited. Pure; only facts the data proves; each clause lists the calls it talks about (by `Call.key`) so the page can
 * link it to their step rows.
 *
 * Order: the outcome (failed / waiting / running / took), then at most one supporting fact (the call that took the
 * biggest share, or a fan-out). Failure reasons go through `reason()`, never the server's raw text (F15).
 */
import { waitFrom, type Call } from '@nasiko/features/flows/calls'
import { fmtDuration, fmtWait } from '@nasiko/features/flows/precision'

export interface FlowClause {
  text: string
  keys: string[]
}

export interface FlowNarrativeInput {
  status: 'running' | 'paused' | 'finishing' | 'completed' | 'failed' | 'unknown'
  /** Null while the flow runs. */
  durationMs: number | null
  /** How long it has run so far, for running flows. */
  elapsedMs: number
  /** The page's clock (epoch ms), for how long a wait has lasted. */
  now: number
  calls: readonly Call[]
  /** Whether the timing is exact (F17). */
  exact: boolean
  /** The biggest fan-out: its parent's name and how many calls ran at once. */
  fanOut: { parent: string | null; count: number; keys: string[] } | null
  /** The friendly reason for a failed call (the page's errors table). */
  reason: (call: Call) => string
}

const nameOf = (c: Call) => c.agentName ?? 'an agent'
const SHARE_MIN = 0.3

export function flowNarrative(i: FlowNarrativeInput): FlowClause[][] {
  const sentences: FlowClause[][] = []
  const failed = i.calls.find((c) => c.status === 'failed')
  const waiting = i.calls.find((c) => c.status === 'awaiting_human')
  const working = i.calls.filter((c) => c.endMs === null && c.status !== 'awaiting_human')

  if (i.status === 'failed') {
    if (failed)
      sentences.push([
        { text: `Failed at ${nameOf(failed)}: ${i.reason(failed)}.`, keys: [failed.key] },
      ])
    else {
      const last = [...i.calls].sort((a, b) => (b.endMs ?? 0) - (a.endMs ?? 0))[0]
      sentences.push([
        last
          ? { text: `Failed after ${nameOf(last)}; no step recorded the error.`, keys: [last.key] }
          : { text: 'Failed before any agent call was recorded.', keys: [] },
      ])
    }
  } else if (i.status === 'paused') {
    sentences.push([
      waiting
        ? {
            text: `Waiting on you for ${fmtWait(i.now - waitFrom(waiting))}: ${nameOf(waiting)} needs an answer.`,
            keys: [waiting.key],
          }
        : { text: 'Waiting on a human.', keys: [] },
    ])
  } else if (i.status === 'running' || i.status === 'finishing') {
    const who = working[0]
    sentences.push([
      {
        text: who
          ? `Running for ${fmtDuration(i.elapsedMs, true)}; ${nameOf(who)} is working.`
          : `Running for ${fmtDuration(i.elapsedMs, true)}.`,
        keys: who ? [who.key] : [],
      },
    ])
  } else if (i.status === 'completed' && i.durationMs !== null) {
    const total = i.durationMs
    // A call that encloses others (an agent waiting on its own fan-out) isn't the answer to "where did the time
    // go": only calls with nothing under them count.
    const parents = new Set(i.calls.flatMap((c) => (c.parentKey ? [c.parentKey] : [])))
    const longest = i.calls
      .filter((c) => c.endMs !== null && !parents.has(c.key))
      .sort((a, b) => (b.endMs ?? 0) - b.startMs - ((a.endMs ?? 0) - a.startMs))[0]
    const share = longest && total > 0 ? ((longest.endMs ?? 0) - longest.startMs) / total : 0
    const took: FlowClause = { text: `Took ${fmtDuration(total, i.exact)}`, keys: [] }
    sentences.push(
      longest && i.calls.length > 1 && share >= SHARE_MIN
        ? [
            { ...took, text: `${took.text};` },
            {
              text: ` ${nameOf(longest)} took ${Math.round(Math.min(1, share) * 100)}% of it.`,
              keys: [longest.key],
            },
          ]
        : [{ ...took, text: `${took.text}.` }],
    )
  }

  if (i.fanOut && i.fanOut.count >= 2 && sentences.length < 2)
    sentences.push([
      {
        text: `${i.fanOut.parent ?? 'One agent'} ran ${i.fanOut.count} calls at once.`,
        keys: i.fanOut.keys,
      },
    ])
  return sentences
}
