/**
 * The status the page shows, which isn't always the server's (plans/feat-flows.md F13; eng review A4, O5).
 *
 * At `6ab60326` a proxied sub-call's completion marks the whole flow `completed` (FL-8), so the server's word can come
 * early. The page trusts it only when nothing it can see is still going:
 *
 *   server running / paused / failed ───────────────────────────────> as is (any other word: Unknown)
 *   server completed ─┬─ a recorded step awaiting a human ───────────> Paused   (markedEarly, A4)
 *                     ├─ a recorded step running or pending ────────> Running  (markedEarly, A4)
 *                     ├─ the trace has an open call, or a call ends ─> Finishing, for FINISHING_MAX_MS after
 *                     │  over 1 s after completed_at                    completed_at (markedEarly, O5), then Completed
 *                     │  (completed_at is the server's clock: the window runs from when this page first saw it, if
 *                     │  that's earlier, so a client clock behind the server's can't stretch it)
 *                     └─ otherwise ─────────────────────────────────> Completed
 *
 * With Tempo off and no recorded steps nothing can contradict the server: that case waits for FL-8.
 */
import type { Flow, FlowStep } from './types'
import {
  FINISHING_MAX_MS,
  FINISH_TOLERANCE_MS,
  PAUSED_POLL_MS,
  RUNNING_POLL_MS,
  STUCK_AFTER_MS,
  STUCK_POLL_MS,
} from './tuning'

/** `unknown`: a server status this page doesn't know (never read as a failure). */
export type DisplayStatus = 'running' | 'paused' | 'finishing' | 'completed' | 'failed' | 'unknown'

/** What the flow's trace says about its end; null when no trace was read. */
export interface TraceTiming {
  /** The latest span end, epoch ms. */
  lastEndMs: number | null
  /** Any span without an end. */
  open: boolean
}

export interface FlowState {
  status: DisplayStatus
  /** The server said completed but the page can see work still going (A4, O5). */
  markedEarly: boolean
  /** Null while the flow runs. */
  durationMs: number | null
}

const parse = (iso: string | null | undefined) => {
  const t = iso ? Date.parse(iso) : NaN
  return Number.isNaN(t) ? null : t
}

function finishedDuration(flow: Flow, trace: TraceTiming | null): number | null {
  const start = parse(flow.created_at)
  const done = parse(flow.completed_at)
  // The timestamps are exact; `duration_ms` is whole seconds (FL-4), so it's only the fallback.
  const recorded =
    start !== null && done !== null ? Math.max(0, done - start) : (flow.duration_ms ?? null)
  // A trace that ran past completed_at knows the real end (O5).
  if (
    start !== null &&
    done !== null &&
    trace?.lastEndMs != null &&
    trace.lastEndMs > done + FINISH_TOLERANCE_MS
  )
    return trace.lastEndMs - start
  return recorded
}

export function flowState(
  flow: Flow,
  steps: readonly Pick<FlowStep, 'status'>[],
  trace: TraceTiming | null,
  now: number,
  /** When this page first read the flow's `completed_at` (client clock), if it has. */
  doneSeenMs: number | null = null,
): FlowState {
  const s = flow.status
  if (s === 'running') return { status: 'running', markedEarly: false, durationMs: null }
  if (s === 'paused') return { status: 'paused', markedEarly: false, durationMs: null }
  if (s === 'failed')
    return { status: 'failed', markedEarly: false, durationMs: finishedDuration(flow, trace) }
  if (s !== 'completed')
    return { status: 'unknown', markedEarly: false, durationMs: finishedDuration(flow, trace) }

  if (steps.some((x) => x.status === 'awaiting_human'))
    return { status: 'paused', markedEarly: true, durationMs: null }
  if (steps.some((x) => x.status === 'running' || x.status === 'pending'))
    return { status: 'running', markedEarly: true, durationMs: null }

  const done = parse(flow.completed_at)
  const ranPast =
    !!trace &&
    done !== null &&
    trace.lastEndMs !== null &&
    trace.lastEndMs > done + FINISH_TOLERANCE_MS
  if (trace?.open || ranPast) {
    const since = done === null ? null : doneSeenMs === null ? done : Math.min(done, doneSeenMs)
    const finishing = since === null || now - since < FINISHING_MAX_MS
    return {
      status: finishing ? 'finishing' : 'completed',
      markedEarly: true,
      durationMs: finishing ? null : finishedDuration(flow, trace),
    }
  }
  return { status: 'completed', markedEarly: false, durationMs: finishedDuration(flow, trace) }
}

/**
 * How often the flow record is re-read for a status (F13); false stops polling. A status this page doesn't know is
 * still re-read (it may be a step on the way to one it does). A moving flow with nothing new for STUCK_AFTER_MS
 * (`idleMs`) is re-read slowly: an orphaned row must not cost a read every 3 s forever.
 */
export function pollMs(status: DisplayStatus, idleMs = 0): number | false {
  if (status === 'completed' || status === 'failed') return false
  if (idleMs >= STUCK_AFTER_MS) return STUCK_POLL_MS
  if (status === 'running' || status === 'finishing') return RUNNING_POLL_MS
  return PAUSED_POLL_MS
}

/** The newest thing the record says happened (epoch ms): the flow's start or any step's start or end. */
export function lastActivityMs(
  flow: Pick<Flow, 'created_at'>,
  steps: readonly Pick<FlowStep, 'created_at' | 'completed_at'>[],
): number | null {
  const ts = [flow.created_at, ...steps.flatMap((s) => [s.created_at, s.completed_at])]
    .map(parse)
    .filter((t): t is number => t !== null)
  return ts.length ? Math.max(...ts) : null
}

/** Whether new agent-to-agent calls can still appear, so the trace is re-read (O4). A paused flow waits. */
export const isWorking = (status: DisplayStatus) => status === 'running' || status === 'finishing'
