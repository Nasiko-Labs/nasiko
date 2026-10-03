/**
 * Pure session derivations: day keys, the live day scan, p95 lanes, status labels.
 * Mock and live mode both go through these; nothing here fetches.
 */
import { LANE_MIN_PER_AGENT, LANE_MIN_SESSIONS, P95 } from './tuning'
import type { SessionSummary, TraceEntry } from './types'

export const sessionCost = (s: Pick<SessionSummary, 'cost_summary'>): number | null =>
  s.cost_summary?.total?.cost ?? null
/** `agent_id` is the raw agent name; "" when the server couldn't resolve it (deleted/unknown). */
export const isUnknownAgent = (s: Pick<SessionSummary, 'agent_id'>) => !s.agent_id

/**
 * UTC day of `start_time` (the finops day key). Null start → outside every day.
 * The server filters sessions on `created_at`, so a session created at 23:59 whose first
 * trace starts at 00:01 is fetched for one day and shown on the next; offset paging can
 * also shift under new arrivals. The day scan is never "exact".
 */
export function sessionDay(s: Pick<SessionSummary, 'start_time'>): string | null {
  if (!s.start_time) return null
  const t = Date.parse(s.start_time)
  return Number.isNaN(t) ? null : new Date(t).toISOString().slice(0, 10)
}

export function dayBounds(day: string): { start: Date; end: Date } {
  const start = new Date(`${day}T00:00:00.000Z`)
  return { start, end: new Date(start.getTime() + 86_400_000) }
}

/** Nearest-rank percentile of the non-null values; null when there are none. */
export function percentile(values: readonly (number | null | undefined)[], q = P95): number | null {
  const xs = values
    .filter((v): v is number => typeof v === 'number' && Number.isFinite(v))
    .sort((a, b) => a - b)
  if (!xs.length) return null
  const rank = Math.max(1, Math.ceil(q * xs.length))
  return xs[rank - 1]
}

export type Status = 'failed' | 'ok' | 'unchecked' | 'unknown' | 'checking'

export interface Lanes {
  /** Lanes are hidden below LANE_MIN_SESSIONS loaded rows. */
  ranked: boolean
  slow: Set<string>
  costly: Set<string>
  failing: Set<string>
  /** Rows with a definite status (failed or ok). */
  checked: number
  /** Rows whose check finished, including "unknown" (a failed fetch). */
  settled: number
  costP95: number | null
}

/**
 * Slow: duration above the agent's p95 (fleet p95 when the agent has < LANE_MIN_PER_AGENT
 * sessions). Costly: cost above the loaded set's p95. Strictly greater; nulls excluded.
 * Unknown-agent rows never get a per-agent p95.
 */
export function computeLanes(
  rows: readonly SessionSummary[],
  status: ReadonlyMap<string, Status>,
): Lanes {
  const fleetDur = percentile(rows.map((r) => r.duration_ms))
  const costP95 = percentile(rows.map(sessionCost))
  const byAgent = new Map<string, SessionSummary[]>()
  for (const r of rows)
    if (!isUnknownAgent(r)) byAgent.set(r.agent_id, [...(byAgent.get(r.agent_id) ?? []), r])
  const agentP95 = new Map<string, number | null>()
  for (const [agent, list] of byAgent)
    agentP95.set(
      agent,
      list.length >= LANE_MIN_PER_AGENT ? percentile(list.map((r) => r.duration_ms)) : fleetDur,
    )

  const slow = new Set<string>()
  const costly = new Set<string>()
  const failing = new Set<string>()
  let checked = 0
  let settled = 0
  for (const r of rows) {
    const threshold = isUnknownAgent(r) ? fleetDur : (agentP95.get(r.agent_id) ?? fleetDur)
    if (r.duration_ms != null && threshold != null && r.duration_ms > threshold)
      slow.add(r.session_id)
    const c = sessionCost(r)
    if (c != null && costP95 != null && c > costP95) costly.add(r.session_id)
    const st = status.get(r.session_id)
    if (st === 'failed' || st === 'ok') checked++
    if (st === 'failed' || st === 'ok' || st === 'unknown') settled++
    if (st === 'failed') failing.add(r.session_id)
  }
  return {
    ranked: rows.length >= LANE_MIN_SESSIONS,
    slow,
    costly,
    failing,
    checked,
    settled,
    costP95,
  }
}

export type SortKey = 'cost' | 'time'

/** Cost sort puts null costs last; time sort is newest first (nulls last). */
export function sortSessions(rows: readonly SessionSummary[], by: SortKey): SessionSummary[] {
  const t = (s: SessionSummary) => (s.start_time ? Date.parse(s.start_time) : null)
  return [...rows].sort((a, b) => {
    const [x, y] = by === 'cost' ? [sessionCost(a), sessionCost(b)] : [t(a), t(b)]
    if (x == null && y == null) return 0
    if (x == null) return 1
    if (y == null) return -1
    return y - x
  })
}

export function dedupeSessions(rows: readonly SessionSummary[]): SessionSummary[] {
  const seen = new Set<string>()
  return rows.filter((r) => (seen.has(r.session_id) ? false : (seen.add(r.session_id), true)))
}

export interface ScanPage {
  sessions: SessionSummary[]
  hasNextPage: boolean
}

export interface DayScan {
  /** The day's sessions, deduped. */
  rows: SessionSummary[]
  scanned: number
  pages: number
  /** Stopped because the server has no more rows (or a row older than the day showed up). */
  complete: boolean
  /** Stopped at the page cap without reaching the day start. */
  capped: boolean
  /** None of the scanned rows belonged to the day, and the scan was capped. */
  missedDay: boolean
}

/**
 * Fold scan pages (newest first, from `start_time = day start`) into the day's rows.
 * Rows newer than the day end are skipped; the scan is complete when a page reports no
 * next page. Offset paging can shift under new arrivals: rows are deduped, never "exact".
 */
export function foldDayScan(day: string, pages: readonly ScanPage[], maxPages: number): DayScan {
  const { start } = dayBounds(day)
  const all = dedupeSessions(pages.flatMap((p) => p.sessions))
  const rows = all.filter((s) => {
    const d = sessionDay(s)
    return d !== null && d === day
  })
  const last = pages[pages.length - 1]
  const reachedOlder = all.some((s) => s.start_time && Date.parse(s.start_time) < start.getTime())
  const complete = !last || !last.hasNextPage || reachedOlder
  const capped = !complete && pages.length >= maxPages
  return {
    rows,
    scanned: all.length,
    pages: pages.length,
    complete,
    capped,
    missedDay: capped && rows.length === 0,
  }
}

/** Did the whole page come back without trace data? (Tempo not running, or not exported yet.) */
export function noTraceData(rows: readonly SessionSummary[]): boolean {
  return rows.length > 0 && rows.every((r) => r.num_traces == null)
}

/** A trace entry's cost, from `root_span.trace.cost_summary` (a free-form JSON value on the server). */
export function entryCost(t: Pick<TraceEntry, 'root_span'>): number | null {
  const v = (t.root_span.trace.cost_summary as unknown as { total?: { cost?: unknown } } | null)
    ?.total?.cost
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

/** Largest first: by cost when the server reports it, else by tokens. */
export function bySize<T extends Pick<TraceEntry, 'root_span'>>(entries: readonly T[]): T[] {
  return [...entries].sort((a, b) => {
    const ca = entryCost(a)
    const cb = entryCost(b)
    if (ca !== null && cb !== null && ca !== cb) return cb - ca
    return b.root_span.cumulative_token_count_total - a.root_span.cumulative_token_count_total
  })
}
