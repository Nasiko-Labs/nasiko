/**
 * Subagents (coding-agent import plan R15): a coding harness's subagent run is its own session, linked to the session
 * that spawned it. The server stores the link on the child (`parent_session_id`, `spawn_tool_call_id`,
 * `subagent_type`) and counts the child's tokens and cost once, on the child.
 *
 * - `session/list` rows gain `parent_session_id`, `subagent_count` and `subagents` (the children's totals, separate
 *   from the row's own cost); the default list is top-level only, `?parent=<id>` lists one session's children.
 * - `session/{id}` gains the same parent fields plus `subagents`, one entry per child.
 *
 * Older servers send none of these: every field defaults (and a malformed one falls back), so the UI shows no
 * subagent UI at all instead of failing. Read the fields only through `rowSubagents` / `detailSubagents`.
 */
import { z } from 'zod'
import type { SessionDetail, SessionSummary } from './types'

const MAX_ID = 256

const id = z.string().min(1).max(MAX_ID)
/** An optional id or label: absent, null or malformed all read as null. */
const optional = <T extends z.ZodType>(t: T) => t.nullable().default(null).catch(null)
const count = z.number().int().nonnegative().default(0).catch(0)
const amount = z.number().nonnegative().nullable().default(null).catch(null)

const parentFields = {
  parent_session_id: optional(id),
  spawn_tool_call_id: optional(id),
  subagent_type: optional(z.string().min(1)),
}

const rowSchema = z.looseObject({
  ...parentFields,
  subagent_count: count,
  subagents: z
    .looseObject({ turns: count, tokens: amount, cost: amount })
    .nullable()
    .default(null)
    .catch(null),
})

const childSchema = z.looseObject({
  session_id: id,
  subagent_type: optional(z.string().min(1)),
  spawn_tool_call_id: optional(id),
  turns: count,
  tokens: amount,
  cost: amount,
  start_time: optional(z.string()),
  end_time: optional(z.string()),
})

const detailSchema = z.looseObject({
  ...parentFields,
  // One bad entry drops itself, not the list.
  subagents: z
    .array(z.unknown())
    .default([])
    .catch([])
    .transform((xs) =>
      xs.flatMap((x) => {
        const r = childSchema.safeParse(x)
        return r.success ? [r.data] : []
      }),
    ),
})

export interface ParentLink {
  parentSessionId: string | null
  spawnToolCallId: string | null
  subagentType: string | null
}

/** A session list row's subagent fields; `count` 0 means the row has none (or the server predates them). */
export interface RowSubagents extends ParentLink {
  count: number
  /** The children's totals, separate from the row's own; null when the server sends none. */
  totals: { turns: number; tokens: number | null; cost: number | null } | null
}

/** One subagent run, from either source (a `?parent=` row or a detail's `subagents` entry). */
export interface SubagentRun {
  sessionId: string
  type: string | null
  spawnToolCallId: string | null
  turns: number | null
  tokens: number | null
  cost: number | null
  startTime: string | null
  durationMs: number | null
}

export interface DetailSubagents extends ParentLink {
  subagents: SubagentRun[]
}

const link = (x: z.infer<typeof rowSchema> | z.infer<typeof detailSchema>): ParentLink => ({
  parentSessionId: x.parent_session_id,
  spawnToolCallId: x.spawn_tool_call_id,
  subagentType: x.subagent_type,
})

const NONE: RowSubagents = {
  parentSessionId: null,
  spawnToolCallId: null,
  subagentType: null,
  count: 0,
  totals: null,
}

export function rowSubagents(row: SessionSummary): RowSubagents {
  const r = rowSchema.safeParse(row)
  if (!r.success) return NONE
  return { ...link(r.data), count: r.data.subagent_count, totals: r.data.subagents }
}

const span = (start: string | null, end: string | null): number | null => {
  const a = start ? Date.parse(start) : Number.NaN
  const b = end ? Date.parse(end) : Number.NaN
  return Number.isNaN(a) || Number.isNaN(b) || b < a ? null : b - a
}

export function detailSubagents(detail: SessionDetail): DetailSubagents {
  const r = detailSchema.safeParse(detail)
  if (!r.success)
    return { parentSessionId: null, spawnToolCallId: null, subagentType: null, subagents: [] }
  return {
    ...link(r.data),
    subagents: r.data.subagents.map((c) => ({
      sessionId: c.session_id,
      type: c.subagent_type,
      spawnToolCallId: c.spawn_tool_call_id,
      turns: c.turns,
      tokens: c.tokens,
      cost: c.cost,
      startTime: c.start_time,
      durationMs: span(c.start_time, c.end_time),
    })),
  }
}

/** A `session/list?parent=` row as a subagent run (its turns are the row's stored count). */
export function childRun(row: SessionSummary): SubagentRun {
  const f = rowSubagents(row)
  return {
    sessionId: row.session_id,
    type: f.subagentType,
    spawnToolCallId: f.spawnToolCallId,
    turns: row.num_traces ?? null,
    tokens: row.token_usage.total ?? null,
    cost: row.cost_summary.total.cost ?? null,
    startTime: row.start_time ?? null,
    durationMs: row.duration_ms ?? null,
  }
}

/**
 * The cost line beside a parent's own cost: `incl` (own + subagents) when both are known, `only` (the subagents')
 * when the session's own cost isn't, null when the subagents report none. Never a replacement for the own cost.
 */
export function costWithSubagents(
  own: number | null | undefined,
  sub: number | null,
): { kind: 'incl' | 'only'; value: number } | null {
  if (sub === null) return null
  return own === null || own === undefined
    ? { kind: 'only', value: sub }
    : { kind: 'incl', value: own + sub }
}
