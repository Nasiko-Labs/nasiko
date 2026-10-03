/**
 * Mock of the PROPOSED `GET /api/observability/coding-agents/usage` (plan §4) as the OSS nasiko-server would answer
 * it, computed from seed-harness.ts: only `user_id = self`, and the landing is self
 * (docs/designs/openruntime-harness-endpoint-requirements.md §3). The rollups, windows and API_CONVENTIONS errors
 * here are the endpoint's own; the EE layer's org rules (units, landing, residual rows, paging) build on them in its
 * own mocks.
 */
import type {
  HarnessTotals,
  RecentSession,
  RowKind,
  UsageResponse,
  UsageRow,
  UsageScope,
} from '@/features/harnesses/types'
import type { HarnessSeed, HSession, HUser } from './seed-harness'
import { DAY_MS, round6 as r6 } from './seed'

export class UsageHttpError extends Error {
  readonly status: number
  readonly code: string
  constructor(status: number, code: string, message: string) {
    super(message)
    this.status = status
    this.code = code
  }
}

const RANGE_MS: Record<string, number> = { '24h': DAY_MS, '7d': 7 * DAY_MS, '30d': 30 * DAY_MS }
/** The endpoint's `scope` values (every edition validates them; only EE serves any). */
const SCOPES = ['org', 'mine', 'unassigned', 'unit', 'direct'] as const
const RECENT_SESSIONS = 20

export type UsageParams = Record<string, string | null | undefined>

// ── Populations ───────────────────────────────────────────────────────────────

/** scope_devs population: active, not a service account. */
export const inPopulation = (u: HUser) => u.is_active && !u.service_account

// ── Metrics ───────────────────────────────────────────────────────────────────

export interface Win {
  start: number
  end: number
  range?: '24h' | '7d' | '30d'
}

/** The finops window rule, shared with the mocked /finops/dashboard. */
export function resolveWin(p: UsageParams, now: number): Win {
  // `range` replaces start_time and is anchored to end_time (or now), as the finops API does
  // (handler.rs resolve_range_params: end = end_time ?? now; start = end - range).
  if (p.range) {
    const len = RANGE_MS[p.range]
    if (!len) throw new UsageHttpError(400, 'invalid_range', `range must be one of 24h, 7d, 30d`)
    const end = p.end_time ? Date.parse(p.end_time) : now
    if (Number.isNaN(end)) throw new UsageHttpError(400, 'invalid_window', 'invalid end_time')
    return { start: end - len, end, range: p.range as Win['range'] }
  }
  const start = p.start_time ? Date.parse(p.start_time) : now - RANGE_MS['30d']!
  const end = p.end_time ? Date.parse(p.end_time) : now
  if (Number.isNaN(start) || Number.isNaN(end) || start >= end)
    throw new UsageHttpError(400, 'invalid_window', 'invalid start_time/end_time')
  return { start, end }
}

function registeredHarnesses(seed: HarnessSeed, userId: string): Set<string> {
  return new Set(
    seed.agents
      .filter((a) => a.owner_id === userId && !a.deleted && a.harness && !a.spoofed)
      .map((a) => a.harness),
  )
}

const emptyTotals = (scopeDevs: number): HarnessTotals => ({
  scope_devs: scopeDevs,
  active_devs: 0,
  registered_devs: 0,
  idle_seats: 0,
  sessions: 0,
  turns: 0,
  tokens: 0,
  cost_usd: 0,
  unpriced_calls: 0,
  delta_pct: null,
})

/** Totals for a user set; `harness` restricts to one harness. Activity counts deleted rows too (S9). */
function totalsFor(
  seed: HarnessSeed,
  users: HUser[],
  sessions: HSession[],
  prev: HSession[] | null,
  harness?: string,
): HarnessTotals {
  const ids = new Set(users.map((u) => u.id))
  const mine = sessions.filter((s) => ids.has(s.user_id) && (!harness || s.harness === harness))
  const t = emptyTotals(users.length)
  const active = new Set<string>()
  const activePairs = new Set<string>()
  for (const s of mine) {
    t.sessions++
    t.turns += s.turns
    t.tokens += s.tokens
    t.cost_usd += s.cost_usd
    t.unpriced_calls += s.unpriced_turns
    active.add(s.user_id)
    activePairs.add(`${s.user_id}|${s.harness}`)
  }
  let registered = 0
  let idle = 0
  for (const u of users) {
    const regs = [...registeredHarnesses(seed, u.id)].filter((h) => !harness || h === harness)
    if (regs.length) registered++
    for (const h of regs) if (!activePairs.has(`${u.id}|${h}`)) idle++
  }
  t.active_devs = active.size
  t.registered_devs = registered
  t.idle_seats = idle
  t.cost_usd = r6(t.cost_usd)
  if (prev) {
    const prevTurns = prev
      .filter((s) => ids.has(s.user_id) && (!harness || s.harness === harness))
      .reduce((n, s) => n + s.turns, 0)
    t.delta_pct =
      prevTurns === 0 ? null : Math.round(((t.turns - prevTurns) / prevTurns) * 1000) / 10
  }
  return t
}

export function harnessesIn(seed: HarnessSeed, users: HUser[], sessions: HSession[]): string[] {
  const ids = new Set(users.map((u) => u.id))
  const set = new Set<string>()
  for (const a of seed.agents)
    if (ids.has(a.owner_id) && a.harness && !a.spoofed && !a.deleted) set.add(a.harness)
  for (const s of sessions) if (ids.has(s.user_id)) set.add(s.harness)
  return [...set]
}

export function row(
  seed: HarnessSeed,
  key: string,
  label: string,
  kind: RowKind,
  users: HUser[],
  cur: HSession[],
  prev: HSession[] | null,
  harnesses: string[],
): UsageRow {
  const ids = new Set(users.map((u) => u.id))
  const last = cur.find((s) => ids.has(s.user_id))?.started_at
  return {
    key,
    label,
    kind,
    harness_breakdown: Object.fromEntries(
      harnesses.map((h) => [h, totalsFor(seed, users, cur, prev, h)]),
    ),
    totals: totalsFor(seed, users, cur, prev),
    ...(kind === 'user' && last ? { last_active: last } : {}),
  }
}

// ── The endpoint ──────────────────────────────────────────────────────────────

/** A request's window, its sessions, and the previous window's when `compare` is on. */
export interface WindowSessions {
  win: Win
  cur: HSession[]
  prev: HSession[] | null
}

export function windowSessions(seed: HarnessSeed, p: UsageParams, now: number): WindowSessions {
  const win = resolveWin(p, now)
  const compare = p.compare === '1' || p.compare === 'true'
  const len = win.end - win.start
  return {
    win,
    cur: seed.sessions.filter((s) => s.ts >= win.start && s.ts < win.end),
    prev: compare ? seed.sessions.filter((s) => s.ts >= win.start - len && s.ts < win.start) : null,
  }
}

/** One developer's Individual level. `units`: their unit placement, for the breadcrumb (EE; OSS has no units). */
export function individualUsage(
  seed: HarnessSeed,
  target: HUser,
  { win, cur, prev }: WindowSessions,
  units?: { id: string; name: string }[],
): UsageResponse {
  const users = [target]
  const recent: RecentSession[] = cur
    .filter((s) => s.user_id === target.id)
    .slice(0, RECENT_SESSIONS)
    .map((s) => ({
      session_id: s.session_id,
      harness: s.harness,
      started_at: s.started_at,
      turns: s.turns,
      cost_usd: s.cost_usd,
    }))
  return build(
    seed,
    {
      kind: 'user',
      user_id: target.id,
      label: target.display_name,
      visibility: 'named',
      ...(units ? { units } : {}),
    },
    win,
    users,
    [],
    0,
    cur,
    prev,
    harnessesIn(seed, users, cur),
    recent,
  )
}

export const notVisible = (what: 'unit' | 'user' = 'unit') =>
  new UsageHttpError(
    404,
    what === 'user' ? 'user_not_visible' : 'unit_not_visible',
    `${what} not found or not visible`,
  )

/** A `scope` outside the endpoint's list is a coded 400 in every edition. */
export function checkScope(scope: string): void {
  if (!(SCOPES as readonly string[]).includes(scope))
    throw new UsageHttpError(400, 'invalid_scope', `scope must be one of ${SCOPES.join(', ')}`)
}

/**
 * The OSS endpoint: the viewer's own Individual level. Any scope (the org levels) or another developer is the same
 * coded 404 an EE member gets, so the tree shape never leaks.
 */
export function usage(
  seed: HarnessSeed,
  p: UsageParams,
  viewer: HUser,
  now: number,
): UsageResponse {
  const w = windowSessions(seed, p, now)
  if (p.scope) {
    checkScope(p.scope)
    throw notVisible()
  }
  if (p.user_id && p.user_id !== viewer.id) throw notVisible('user')
  return individualUsage(seed, viewer, w)
}

export function build(
  seed: HarnessSeed,
  scope: UsageScope,
  win: Win,
  users: HUser[],
  rows: UsageRow[],
  overlap: number,
  cur: HSession[],
  prev: HSession[] | null,
  harnesses: string[],
  recent?: RecentSession[],
): UsageResponse {
  const ids = new Set(users.map((u) => u.id))
  const mine = cur.filter((s) => ids.has(s.user_id))
  const byHarness = harnesses.map((h) => {
    const models = new Map<string, number>()
    for (const s of mine)
      if (s.harness === h) models.set(s.model, (models.get(s.model) ?? 0) + s.turns)
    return {
      ...totalsFor(seed, users, cur, prev, h),
      harness: h,
      top_models: [...models.entries()]
        .sort((a, b) => b[1] - a[1])
        .slice(0, 2)
        .map(([m]) => m),
    }
  })
  const series = new Map<
    string,
    { date: string; harness: string; users: Set<string>; cost_usd: number; tokens: number }
  >()
  for (const s of mine) {
    const k = `${s.date}|${s.harness}`
    const e = series.get(k) ?? {
      date: s.date,
      harness: s.harness,
      users: new Set<string>(),
      cost_usd: 0,
      tokens: 0,
    }
    e.users.add(s.user_id)
    e.cost_usd += s.cost_usd
    e.tokens += s.tokens
    series.set(k, e)
  }
  return {
    scope,
    window: {
      start_time: new Date(win.start).toISOString(),
      end_time: new Date(win.end).toISOString(),
      ...(win.range ? { range: win.range } : {}),
    },
    totals: totalsFor(seed, users, cur, prev),
    by_harness: byHarness,
    rows,
    overlap_devs: overlap,
    series: [...series.values()]
      .sort((a, b) => a.date.localeCompare(b.date) || a.harness.localeCompare(b.harness))
      .map((e) => ({
        date: e.date,
        harness: e.harness,
        active_devs: e.users.size,
        cost_usd: r6(e.cost_usd),
        tokens: e.tokens,
      })),
    ...(recent ? { recent_sessions: recent } : {}),
    // API_CONVENTIONS §1: paging fields are always present (only developer lists page).
    has_more: false,
    next_cursor: null,
    prev_cursor: null,
  }
}

// ── Live-fallback shapes (existing endpoints), for the viewer's own harness agents ──

/** `/finops/dashboard?my_agent=true` rows for the viewer's LIVE harness agents (display names). */
export function myAgentDashboardRows(seed: HarnessSeed, viewer: HUser, start: number, end: number) {
  // Every live agent the viewer owns gets a row, zero operations included, harness or not
  // (service.rs pushes a row per owned agent): the client must ignore the non-harness ones.
  const live = seed.agents.filter((a) => a.owner_id === viewer.id && !a.deleted)
  return live.map((a) => {
    const ss = seed.sessions.filter((s) => s.agent_id === a.id && s.ts >= start && s.ts < end)
    const cost = r6(ss.reduce((n, s) => n + s.cost_usd, 0))
    const ops = ss.reduce((n, s) => n + s.turns, 0)
    const tokens = ss.reduce((n, s) => n + s.tokens, 0)
    return {
      agent_id: a.id,
      agent_name: a.display_name,
      total_cost: cost,
      operations: ops,
      is_capped: false,
      avg_cost_per_operation: ops ? r6(cost / ops) : 0,
      prompt_tokens: Math.round(tokens * 0.8),
      completion_tokens: tokens - Math.round(tokens * 0.8),
      cache_read_tokens: 0,
      cache_creation_tokens: 0,
      total_tokens: tokens,
      avg_latency_ms: null,
      avg_latency_p95_ms: null,
      avg_latency_p99_ms: null,
      tool_call_count: 0,
      version: null,
      container_hours: 0,
    }
  })
}

/** The dashboard summary's unpriced count: NOT owner-scoped on the real server even with
 *  my_agent=true (service.rs filters it by window/agent/model/provider/EE user scope only). */
export function fleetUnpriced(seed: HarnessSeed, start: number, end: number): number {
  return seed.sessions
    .filter((s) => s.ts >= start && s.ts < end)
    .reduce((n, s) => n + s.unpriced_turns, 0)
}
