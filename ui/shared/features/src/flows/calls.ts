/**
 * One agent call in a flow, whatever recorded it (plans/feat-flows.md F4, F14): a recorded `flow_steps` row, an
 * `a2a.proxy` span from the flow's trace, or both once merged (`merge.ts`). Every view (swimlane, Table, Steps, Graph)
 * reads these, so none of them cares which source fed it.
 */
import type { FlatSpan } from '@nasiko/features/observability/spans'
import type { FlowStep } from './types'

type CallSource = 'recorded' | 'trace' | 'both'

export interface Call {
  /** `step:<id>` for a recorded step (merged ones keep it), `span:<hex span id>` for a call from the trace. */
  key: string
  agentId: string | null
  agentName: string | null
  /** Epoch ms. */
  startMs: number
  /** Epoch ms; null while the call is open. */
  endMs: number | null
  /** Step status words (`running`, `completed`, `failed`, `awaiting_human`, `resumed`, `pending`). */
  status: string
  source: CallSource
  stepId: string | null
  /** The HEX span id (`?span=`), never the base64 `id`. */
  spanId: string | null
  /** The key of the call that made this one; null for the Orchestrator's own calls and the root call. */
  parentKey: string | null
  input: string | null
  output: string | null
  error: string | null
  tokens: number | null
  /**
   * Whether the end time is to the millisecond. A recorded step's `completed_at` is a full timestamp, but a step with
   * only `latency_ms` is whole seconds (FL-4, F17); span times are exact.
   */
  exactEnd: boolean
  /** O3: this call and another one may be the same call (same agent, starts within the match window, ambiguous). */
  maybeSame: boolean
  /**
   * When a call waiting on a human paused (epoch ms): the dispatcher stamps `completed_at` at the pause, so the work
   * runs from the start to here and the wait from here on. Null when unknown or not waiting.
   */
  waitStartMs: number | null
}

export const PROXY_SPAN = 'a2a.proxy'
const OPEN = new Set(['running', 'pending', 'awaiting_human'])

const parse = (iso: string | null | undefined): number | null => {
  if (!iso) return null
  const t = Date.parse(iso)
  return Number.isNaN(t) ? null : t
}

/**
 * Recorded steps as calls. A step with neither `completed_at` nor a closed status is open; a step waiting on a human
 * is open too, though the server stamps `completed_at` when it pauses (`a2a_dispatch.rs`). Routed-chat steps carry no
 * `agent_id` at `6ab60326` (the dispatcher writes `agent_name` only, FL-9), so `agentOf` resolves the name, and only
 * a unique match counts: a guessed id would merge or answer the wrong call.
 */
export function callsFromSteps(
  steps: readonly FlowStep[],
  agentOf: (name: string) => string | null = () => null,
): Call[] {
  return steps.flatMap((s) => {
    const start = parse(s.created_at)
    if (start === null) return []
    const waiting = s.status === 'awaiting_human'
    // A paused step's `completed_at` is when it paused, not an end (a2a_dispatch.rs at 6ab60326).
    const paused = waiting ? parse(s.completed_at) : null
    const done = waiting ? null : parse(s.completed_at)
    const fromLatency =
      done === null && !OPEN.has(s.status) && typeof s.latency_ms === 'number'
        ? start + s.latency_ms
        : null
    return [
      {
        key: `step:${s.id}`,
        agentId: s.agent_id ?? (s.agent_name ? agentOf(s.agent_name) : null),
        agentName: s.agent_name || null,
        startMs: start,
        endMs: done ?? fromLatency,
        status: s.status,
        source: 'recorded' as const,
        stepId: s.id,
        spanId: null,
        parentKey: s.parent_step_id ? `step:${s.parent_step_id}` : null,
        input: s.input_summary ?? null,
        output: s.output_summary ?? null,
        error: s.error_message ?? null,
        tokens: typeof s.tokens_used === 'number' && s.tokens_used > 0 ? s.tokens_used : null,
        exactEnd: done !== null,
        maybeSame: false,
        waitStartMs: paused !== null && paused >= start ? paused : null,
      },
    ]
  })
}

/** When a call's wait on a human began: its pause, else (no pause recorded) its start. */
export const waitFrom = (c: Pick<Call, 'startMs' | 'waitStartMs'>) => c.waitStartMs ?? c.startMs

/** The flow's `a2a.proxy` spans, in start order: the agent-to-agent calls the trace knows about (A1). */
export function proxySpans(spans: readonly FlatSpan[]): FlatSpan[] {
  return spans
    .filter((s) => s.node.name === PROXY_SPAN)
    .sort(
      (a, b) =>
        (parse(a.node.start_time) ?? 0) - (parse(b.node.start_time) ?? 0) ||
        a.node.span_id.localeCompare(b.node.span_id),
    )
}

/**
 * Calls from the trace. `targets` maps a span's base64 `id` to its callee's agent id (from SpanDetail `agent.id`,
 * A1); only spans with a known target become calls, so the 30-call cap and failed detail reads simply leave spans
 * out. The parent is the nearest proxy span above this one: a call made from inside another agent's call.
 */
export function callsFromSpans(
  spans: readonly FlatSpan[],
  targets: ReadonlyMap<string, string>,
  nameOf: (agentId: string) => string | null,
): Call[] {
  const byId = new Map(spans.map((s) => [s.node.id, s]))
  const parentProxy = (s: FlatSpan): FlatSpan | null => {
    const seen = new Set<string>()
    for (let p = s.node.parent_id; p && !seen.has(p); p = byId.get(p)?.node.parent_id) {
      seen.add(p)
      const up = byId.get(p)
      if (up?.node.name === PROXY_SPAN && targets.has(up.node.id)) return up
    }
    return null
  }
  return proxySpans(spans).flatMap((s) => {
    const agentId = targets.get(s.node.id)
    const start = parse(s.node.start_time)
    if (!agentId || start === null) return []
    const end = parse(s.node.end_time)
    const up = parentProxy(s)
    return [
      {
        key: `span:${s.node.span_id}`,
        agentId,
        agentName: nameOf(agentId),
        startMs: start,
        endMs: end,
        status: s.node.status_code === 'ERROR' ? 'failed' : end === null ? 'running' : 'completed',
        source: 'trace' as const,
        stepId: null,
        spanId: s.node.span_id,
        parentKey: up ? `span:${up.node.span_id}` : null,
        input: null,
        output: null,
        error: null,
        tokens: s.node.token_count_total > 0 ? s.node.token_count_total : null,
        exactEnd: true,
        maybeSame: false,
        waitStartMs: null,
      },
    ]
  })
}
