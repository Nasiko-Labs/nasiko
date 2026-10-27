/**
 * Subagent sessions (coding-agent import plan R15; contract "R15 SUBAGENTS"): the recorded harness chat c004 ran two
 * subagents, each its own session, spawned by the two `Task` calls on its third turn (chatStore.ts `seedRecorded`).
 *
 * What the server sends (sessions_sql / service.rs, R15):
 * - `session/list` lists top-level sessions only; the parent's row gains `subagent_count` and `subagents` (the
 *   children's totals, separate from the row's own cost), and `parent_session_id: null`.
 * - `session/list?parent=<id>` lists that session's children, with `parent_session_id`, `spawn_tool_call_id` and
 *   `subagent_type`.
 * - `session/{id}` gains the same parent fields and a `subagents` entry per child.
 *
 * Only c004 and its children carry the new fields: every other row keeps the older server's shape (the live
 * fixtures predate R15), which is also what the UI's "no subagent fields" path reads.
 */
import type { SessionSummary } from '@nasiko/features/observability/types'

export const SUBAGENT_PARENT = '5eedc000-0000-4000-8000-00000000c004'
/** The `Task` calls that spawned the children: fixed ids, so the ledger can match them. */
export const SPAWN_CALLS = { explore: 'c004-task-explore', review: 'c004-task-review' } as const
/** c004's third turn starts this long after the session (chatStore.ts `turn`: 120 s apart). */
const SPAWN_TURN_OFFSET_MS = 2 * 120_000

interface ChildSeed {
  session_id: string
  type: string
  spawn: string
  turns: number
  input: number
  output: number
  /** After the spawning turn starts. */
  offsetMs: number
  durationMs: number
}

const CHILDREN: readonly ChildSeed[] = [
  {
    session_id: '5eedc000-0000-4000-8000-0000000c004a',
    type: 'Explore',
    spawn: SPAWN_CALLS.explore,
    turns: 3,
    input: 16_000,
    output: 2_400,
    offsetMs: 6_000,
    durationMs: 95_000,
  },
  {
    session_id: '5eedc000-0000-4000-8000-0000000c004b',
    type: 'general-purpose',
    spawn: SPAWN_CALLS.review,
    turns: 2,
    input: 8_800,
    output: 1_000,
    offsetMs: 10_000,
    durationMs: 48_000,
  },
]

// The harness list price the chat mock uses for c004 (claude-sonnet-4: $3 / $15 per M tokens).
const cost = (c: ChildSeed) => Math.round((c.input * 3 + c.output * 15) * 1e6) / 1e12
const tokens = (c: ChildSeed) => c.input + c.output
const iso = (ms: number) => new Date(ms).toISOString()

export const isSubagentSession = (id: string) => CHILDREN.some((c) => c.session_id === id)

const childTimes = (c: ChildSeed, parentStart: number) => {
  const start = parentStart + SPAWN_TURN_OFFSET_MS + c.offsetMs
  return { start, end: start + c.durationMs }
}

/** The children's totals on the parent's row: counted once, on the children; the row's own figures are untouched. */
export function withSubagentTotals<T extends { session_id: string }>(row: T) {
  if (row.session_id !== SUBAGENT_PARENT) return row
  return {
    ...row,
    parent_session_id: null,
    subagent_count: CHILDREN.length,
    subagents: {
      turns: CHILDREN.reduce((a, c) => a + c.turns, 0),
      tokens: CHILDREN.reduce((a, c) => a + tokens(c), 0),
      cost: Math.round(CHILDREN.reduce((a, c) => a + cost(c), 0) * 1e6) / 1e6,
    },
  }
}

/** `session/list?parent=<id>` rows: newest first, as the list is. */
export function subagentRows(parentId: string, parentStart: number, agentName: string) {
  if (parentId !== SUBAGENT_PARENT) return []
  return CHILDREN.map((c) => {
    const { start, end } = childTimes(c, parentStart)
    return {
      id: c.session_id,
      session_id: c.session_id,
      agent_id: agentName,
      session_annotations: [],
      session_annotation_summaries: [],
      num_traces: c.turns,
      status: 'ok',
      failed_flows: 0,
      error_count: 0,
      start_time: iso(start),
      end_time: iso(end),
      duration_ms: c.durationMs,
      first_input: null,
      last_output: null,
      token_usage: { total: tokens(c) },
      trace_latency_ms_p50: null,
      trace_latency_ms_p99: null,
      cost_summary: { total: { cost: cost(c) } },
      parent_session_id: parentId,
      spawn_tool_call_id: c.spawn,
      subagent_type: c.type,
      subagent_count: 0,
      subagents: { turns: 0, tokens: null, cost: null },
    } satisfies SessionSummary & Record<string, unknown>
  }).sort((a, b) => Date.parse(b.start_time) - Date.parse(a.start_time))
}

/** `session/{id}`'s R15 fields: a parent's subagents, or a child's link to its parent. Null for any other session. */
export function subagentDetailFields(id: string, parentStart: number) {
  if (id === SUBAGENT_PARENT)
    return {
      parent_session_id: null,
      spawn_tool_call_id: null,
      subagent_type: null,
      subagents: CHILDREN.map((c) => {
        const { start, end } = childTimes(c, parentStart)
        return {
          session_id: c.session_id,
          subagent_type: c.type,
          spawn_tool_call_id: c.spawn,
          turns: c.turns,
          tokens: tokens(c),
          cost: cost(c),
          start_time: iso(start),
          end_time: iso(end),
        }
      }),
    }
  const child = CHILDREN.find((c) => c.session_id === id)
  if (!child) return null
  return {
    parent_session_id: SUBAGENT_PARENT,
    spawn_tool_call_id: child.spawn,
    subagent_type: child.type,
    subagents: [],
  }
}

/** A child's title on its detail (the chat it lives in has none in this mock). */
export const subagentTitle = (id: string) => {
  const c = CHILDREN.find((x) => x.session_id === id)
  return c ? `${c.type} subagent` : null
}
