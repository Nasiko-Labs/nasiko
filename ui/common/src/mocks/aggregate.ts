/**
 * Server-mirroring aggregations over the seed (plan A19).
 *
 * Each function reproduces what the matching nasiko-server finops handler returns,
 * including its quirks, so the UI meets realistic shapes in mock mode:
 * dashboard rows use display names and only live agents; timeseries/calendar/day
 * sum ALL trace rows (no ACL, deleted agents included) and use raw names; arrays
 * are sparse; `change_pct` is null when previous is 0; `is_capped` is always false.
 *
 * `top-traces` implements the PROPOSED contract from the plan (not on the server yet).
 */
import type {
  AgentFinopsRow,
  FinopsDashboardData,
  FinopsDayDrilldown,
  FinopsSpendCalendar,
  FinopsSpendTimeseries,
  KpiValue,
  SpendTimeseriesPoint,
  TopTracesData,
  WorkflowFinopsRow,
} from '@/features/tokenops/types'
import { round6 as r6, type Seed, type SeedTrace } from './seed'
import { adminHarnessAgents, harnessTurns, type HarnessSeed } from './seed-harness'
import type { Savings, SavingsData } from '@/features/tokenops/types'

/**
 * The finops endpoints' view of the data: the seed plus the seed admin's coding-harness agents and one trace row per
 * harness turn, as `seed:live` writes them and the server counts them (live TokenOps lists the harness agents, e.g.
 * "Codex (<admin email>)"). Agent version is the agents table default, '1.0.0'.
 */
export function withHarnessTurns(seed: Seed, hs: HarnessSeed): Seed {
  const agents = adminHarnessAgents(hs)
  const byId = new Map(agents.map((a) => [a.id, a]))
  const traces: SeedTrace[] = harnessTurns(hs, agents, seed.spikeDate).map((t) => ({
    trace_id: t.trace_id,
    session_id: t.session_id,
    agent_id: t.agent_id,
    agent_name: byId.get(t.agent_id)!.name,
    model: t.model,
    provider: t.provider,
    input_tokens: t.input_tokens,
    output_tokens: t.output_tokens,
    cache_read_tokens: 0,
    cache_creation_tokens: 0,
    cost_usd: t.cost_usd,
    prompt_cost_usd: t.cost_usd * 0.3,
    completion_cost_usd: t.cost_usd * 0.7,
    latency_ms: 1200,
    tool_call_count: 1,
    started_at: t.started_at,
    ts: t.ts,
    workflow_id: null,
  }))
  return {
    ...seed,
    agents: [
      ...seed.agents,
      ...agents.map((a) => ({
        id: a.id,
        name: a.name,
        display_name: a.display_name,
        deleted: false,
        version: '1.0.0',
      })),
    ],
    traces: [...seed.traces, ...traces].sort((a, b) => a.ts - b.ts),
  }
}

export class MockHttpError extends Error {
  readonly status: number
  /** A JSON body instead of plain text (e.g. custom_providers.rs delete's 409 `{message, referencing_configs}`). */
  readonly body?: unknown
  constructor(status: number, message: string, body?: unknown) {
    super(message)
    this.status = status
    this.body = body
  }
}

const HOUR_MS = 3_600_000
const RANGE_HOURS: Record<string, number> = { '24h': 24, '7d': 168, '30d': 720 }

export interface FilterParams {
  range?: string | null
  start_time?: string | null
  end_time?: string | null
  agent_id?: string | null
  model?: string | null
  provider?: string | null
  view?: string | null
}

interface ServerWindow {
  start: number
  end: number
  bucket: 'hour' | 'day'
}

/**
 * Mirrors the server's `resolve_window` / `resolve_range_params`: range wins; hourly only
 * for 24h; default 30 days. (Named apart from the client's `resolveWindow` in window.ts.)
 */
function resolveServerWindow(p: FilterParams, now: number): ServerWindow {
  const end = p.end_time ? Date.parse(p.end_time) : now
  if (p.range) {
    const hours = RANGE_HOURS[p.range]
    if (!hours) throw new MockHttpError(400, `invalid range '${p.range}'`)
    return { start: end - hours * HOUR_MS, end, bucket: p.range === '24h' ? 'hour' : 'day' }
  }
  const start = p.start_time ? Date.parse(p.start_time) : end - 30 * 24 * HOUR_MS
  if (Number.isNaN(start) || Number.isNaN(end))
    throw new MockHttpError(400, 'invalid start_time/end_time')
  return { start, end, bucket: 'day' }
}

/**
 * Mirrors `resolve_agent_filter`: accepts a UUID or a raw name; unknown or soft-deleted
 * (`deleted_at IS NULL` server-side) → 400. Returns the raw name.
 */
function resolveAgent(seed: Seed, ref: string | null | undefined): string | null {
  if (!ref) return null
  const agent = seed.agents.find((a) => !a.deleted && (a.id === ref || a.name === ref))
  if (!agent) throw new MockHttpError(400, `unknown agent '${ref}'`)
  return agent.name
}

function inWindow(t: SeedTrace, start: number, end: number): boolean {
  return t.ts >= start && t.ts < end
}

/** One pass instead of filtering once per group (the seed is ~20k rows, on the main thread). */
function groupBy<K>(rows: SeedTrace[], key: (t: SeedTrace) => K): Map<K, SeedTrace[]> {
  const out = new Map<K, SeedTrace[]>()
  for (const t of rows) {
    const k = key(t)
    const g = out.get(k)
    if (g) g.push(t)
    else out.set(k, [t])
  }
  return out
}

function matches(t: SeedTrace, agentName: string | null, p: FilterParams): boolean {
  if (agentName && t.agent_name !== agentName) return false
  if (p.model && t.model !== p.model) return false
  if (p.provider && t.provider !== p.provider) return false
  return true
}

/** Postgres `percentile_cont`: linear interpolation between the two nearest ranks, unrounded. */
function pick(sorted: number[], p: number): number | null {
  if (!sorted.length) return null
  const x = (p / 100) * (sorted.length - 1)
  const lo = Math.floor(x)
  const hi = Math.ceil(x)
  return sorted[lo]! + (x - lo) * (sorted[hi]! - sorted[lo]!)
}

/** p50/p95/p99 from one sort, like the server's `percentile_cont(…) WITHIN GROUP (ORDER BY latency_ms)` (service.rs, ea233d20). */
function percentiles(values: number[]): {
  p50: number | null
  p95: number | null
  p99: number | null
} {
  const sorted = [...values].sort((a, b) => a - b)
  return { p50: pick(sorted, 50), p95: pick(sorted, 95), p99: pick(sorted, 99) }
}

function kpi(current: number, previous: number): KpiValue {
  return {
    current,
    previous,
    change_pct:
      previous === 0 ? null : Math.round(((current - previous) / previous) * 100 * 100) / 100,
  }
}

function sumTraces(rows: SeedTrace[]) {
  let cost = 0
  let input = 0
  let output = 0
  let cacheRead = 0
  let cacheCreate = 0
  let tools = 0
  let unpriced = 0
  const latencies: number[] = []
  for (const t of rows) {
    cost += t.cost_usd
    input += t.input_tokens
    output += t.output_tokens
    cacheRead += t.cache_read_tokens
    cacheCreate += t.cache_creation_tokens
    tools += t.tool_call_count
    if (t.cost_usd === 0 && t.input_tokens + t.output_tokens > 0) unpriced++
    latencies.push(t.latency_ms)
  }
  // `tokens` is input+output (the server's previous-window total and workflows); `tokensAll` adds cache tokens, as
  // the server's current-window agent rows and totals do (service.rs grand_total_tokens, ea233d20).
  return {
    cost: r6(cost),
    input,
    output,
    cacheRead,
    cacheCreate,
    tokens: input + output,
    tokensAll: input + output + cacheRead + cacheCreate,
    tools,
    unpriced,
    latencies,
    ops: rows.length,
  }
}

function overlapHours(
  seed: Seed,
  agentId: string | null,
  start: number,
  end: number,
): Map<string, number> {
  const out = new Map<string, number>()
  for (const s of seed.sessions) {
    if (agentId && s.agent_id !== agentId) continue
    const a = Math.max(Date.parse(s.started_at), start)
    // agents/hours_meter.rs windowed_agent_hours (ea233d20): an open session ends at its last_seen_at (the seed writes
    // it at seed time, the anchor), never at now.
    const b = Math.min(s.ended_at ? Date.parse(s.ended_at) : Date.parse(seed.anchor), end)
    if (b > a) out.set(s.agent_id, (out.get(s.agent_id) ?? 0) + (b - a) / HOUR_MS)
  }
  return out
}

export function dashboard(seed: Seed, p: FilterParams, now: number): FinopsDashboardData {
  const view = p.view ?? 'agent'
  if (view !== 'agent' && view !== 'workflow')
    throw new MockHttpError(400, `invalid view '${view}'`)
  const agentName = resolveAgent(seed, p.agent_id)
  const { start, end } = resolveServerWindow(p, now)
  const len = end - start
  const prevStart = start - len

  // get_agent_names (service.rs, ea233d20): live agents ORDER BY name (the raw name); rows keep that order.
  const live = seed.agents
    .filter((a) => !a.deleted && (!agentName || a.name === agentName))
    .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
  const hours = overlapHours(seed, null, start, end)
  const total_container_hours = r6([...hours.values()].reduce((s, h) => s + h, 0))

  const cur = seed.traces.filter((t) => inWindow(t, start, end) && matches(t, agentName, p))
  const prev = seed.traces.filter((t) => inWindow(t, prevStart, start) && matches(t, agentName, p))
  const liveNames = new Set(live.map((a) => a.name))
  const curLive = cur.filter((t) => liveNames.has(t.agent_name))
  const prevLive = prev.filter((t) => liveNames.has(t.agent_name))

  const curByAgent = groupBy(curLive, (t) => t.agent_name)
  const rows: AgentFinopsRow[] = live.map((a) => {
    const s = sumTraces(curByAgent.get(a.name) ?? [])
    const pct = percentiles(s.latencies)
    return {
      agent_id: a.id,
      agent_name: a.display_name,
      total_cost: s.cost,
      operations: s.ops,
      is_capped: false,
      avg_cost_per_operation: s.ops ? r6(s.cost / s.ops) : 0,
      prompt_tokens: s.input,
      completion_tokens: s.output,
      cache_read_tokens: s.cacheRead,
      cache_creation_tokens: s.cacheCreate,
      total_tokens: s.tokensAll,
      avg_latency_ms: pct.p50,
      avg_latency_p95_ms: pct.p95,
      avg_latency_p99_ms: pct.p99,
      tool_call_count: s.tools,
      version: a.version,
      container_hours: r6(hours.get(a.id) ?? 0),
    }
  })

  const c = sumTraces(curLive)
  const pv = sumTraces(prevLive)
  // service.rs (ea233d20): the last 24 h are measured from the real now, whatever the window, with no upper bound.
  const last24 = seed.traces.filter(
    (t) => t.ts >= now - 24 * HOUR_MS && matches(t, agentName, p) && liveNames.has(t.agent_name),
  ).length
  const activeCur = rows.filter((r) => r.operations > 0).length
  const activePrev = new Set(prevLive.map((t) => t.agent_name)).size

  // service.rs spend_by_agent (ea233d20): rows by cost, descending and stable (ties keep name order), top 5 without
  // dropping zero-cost rows; "Others" is the rest, only when above zero.
  const byCost = [...rows].sort((a, b) => b.total_cost - a.total_cost)
  const top5 = byCost.slice(0, 5)
  const pctOf = (v: number) => (c.cost ? Math.round((v / c.cost) * 10000) / 100 : 0)
  const slices = top5.map((r) => ({
    agent_name: r.agent_name,
    spend_usd: r.total_cost,
    pct: pctOf(r.total_cost),
  }))
  const othersSpend = r6(byCost.slice(5).reduce((s, r) => s + r.total_cost, 0))
  if (othersSpend > 0)
    slices.push({ agent_name: 'Others', spend_usd: othersSpend, pct: pctOf(othersSpend) })

  const curByWorkflow = groupBy(cur, (t) => t.workflow_id)
  const workflowRows: WorkflowFinopsRow[] = seed.workflows.map((w) => {
    const s = sumTraces(curByWorkflow.get(w.maf_id) ?? [])
    return {
      maf_id: w.maf_id,
      workflow_name: w.workflow_name,
      total_cost: s.cost,
      executions: s.ops,
      avg_cost_per_execution: s.ops ? r6(s.cost / s.ops) : 0,
      prompt_tokens: s.input,
      completion_tokens: s.output,
      cache_read_tokens: s.cacheRead,
      cache_creation_tokens: s.cacheCreate,
      total_tokens: s.tokens,
      // The server reports an arithmetic mean for workflows, not a percentile.
      avg_latency_ms: s.latencies.length
        ? s.latencies.reduce((a, b) => a + b, 0) / s.latencies.length
        : null,
    }
  })

  // Latency KPIs are the mean of the listed agents' own percentiles, not fleet percentiles (service.rs avg_latency,
  // ea233d20); the previous window uses the agents with previous rows.
  const mean = (xs: (number | null)[]) => {
    const v = xs.filter((x): x is number => x !== null)
    return v.length ? v.reduce((a, b) => a + b, 0) / v.length : 0
  }
  const prevPct = [...groupBy(prevLive, (t) => t.agent_name).values()].map((g) =>
    percentiles(g.map((t) => t.latency_ms)),
  )
  const allCur = sumTraces(cur)
  return {
    summary: {
      total_cost: c.cost,
      total_operations: c.ops,
      operations_last_24h: last24,
      average_cost: c.ops ? r6(c.cost / c.ops) : 0,
      active_agents: activeCur,
      total_agents: live.length,
      total_container_hours,
      // Counted over every agent name in the window, the deleted agent included (service.rs unpriced query, ea233d20).
      unpriced_calls: allCur.unpriced,
      // observability/service.rs TRACE_USAGE_AGG_QUERY (ea233d20): estimated_cost sums rows priced from inferred rates
      // (`cost_estimated`), unknown_confidence_calls counts rows with no recorded confidence, over every agent name in
      // the window (the deleted agent's rows too). Seed rows never set `cost_estimated`.
      estimated_cost: 0,
      unknown_confidence_calls: cur.length,
    },
    agents: rows,
    token_usage: {
      total_tokens: c.tokensAll,
      prompt_tokens: c.input,
      completion_tokens: c.output,
      cache_read_tokens: c.cacheRead,
      cache_creation_tokens: c.cacheCreate,
      avg_tokens_per_operation: c.ops ? Math.floor(c.tokensAll / c.ops) : 0,
    },
    kpis: {
      total_spend: kpi(c.cost, pv.cost),
      // Current includes cache tokens, previous doesn't (a server quirk that inflates change_pct).
      total_tokens: kpi(c.tokensAll, pv.tokens),
      cost_per_operation: kpi(c.ops ? r6(c.cost / c.ops) : 0, pv.ops ? r6(pv.cost / pv.ops) : 0),
      avg_latency_ms: kpi(mean(rows.map((r) => r.avg_latency_ms)), mean(prevPct.map((x) => x.p50))),
      // The server has no previous count for agents: KpiValue::new(total_agents as f64, 0.0) (service.rs, ea233d20),
      // so change_pct is always null.
      total_agents: kpi(live.length, 0),
      active_agents: kpi(activeCur, activePrev),
      total_operations: kpi(c.ops, pv.ops),
      total_tool_calls: kpi(c.tools, pv.tools),
      latency_p95_ms: kpi(
        mean(rows.map((r) => r.avg_latency_p95_ms)),
        mean(prevPct.map((x) => x.p95)),
      ),
      latency_p99_ms: kpi(
        mean(rows.map((r) => r.avg_latency_p99_ms)),
        mean(prevPct.map((x) => x.p99)),
      ),
    },
    attributions:
      view === 'workflow'
        ? { view: 'workflow', rows: workflowRows.sort((a, b) => b.total_cost - a.total_cost) }
        : { view: 'agent', rows },
    spend_by_agent: { slices, total_spend_usd: c.cost },
  }
}

function bucketStart(ts: number, bucket: 'hour' | 'day'): number {
  const d = new Date(ts)
  return bucket === 'hour'
    ? Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), d.getUTCHours())
    : Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate())
}

export function spendTimeseries(seed: Seed, p: FilterParams, now: number): FinopsSpendTimeseries {
  const agentName = resolveAgent(seed, p.agent_id)
  const { start, end, bucket } = resolveServerWindow(p, now)
  const groups = new Map<number, SeedTrace[]>()
  for (const t of seed.traces) {
    if (!inWindow(t, start, end) || !matches(t, agentName, p)) continue
    const key = bucketStart(t.ts, bucket)
    const g = groups.get(key)
    if (g) g.push(t)
    else groups.set(key, [t])
  }
  const points: SpendTimeseriesPoint[] = [...groups.entries()]
    .sort(([a], [b]) => a - b)
    .map(([key, rows]) => {
      const s = sumTraces(rows)
      const pct = percentiles(s.latencies)
      const byAgent = new Map<string, number>()
      for (const t of rows) byAgent.set(t.agent_name, (byAgent.get(t.agent_name) ?? 0) + t.cost_usd)
      const top = [...byAgent.entries()].sort((a, b) => b[1] - a[1])[0]
      return {
        bucket_start: new Date(key).toISOString(),
        spend_usd: s.cost,
        operations: s.ops,
        tool_calls: s.tools,
        top_agent_name: top ? top[0] : null,
        top_agent_spend_usd: top ? r6(top[1]) : null,
        p50_latency_ms: pct.p50,
        p95_latency_ms: pct.p95,
        p99_latency_ms: pct.p99,
      }
    })
  return { bucket, points }
}

export function spendCalendar(
  seed: Seed,
  month: string | null,
  p: FilterParams,
  now: number,
): FinopsSpendCalendar {
  if (!month || !/^\d{4}-\d{2}$/.test(month))
    throw new MockHttpError(400, 'month is required (YYYY-MM)')
  const agentName = resolveAgent(seed, p.agent_id)
  const [y, m] = month.split('-').map(Number)
  const start = Date.UTC(y, m - 1, 1)
  const end = Date.UTC(y, m, 1)
  const byDay = new Map<string, { spend: number; ops: number }>()
  for (const t of seed.traces) {
    if (!inWindow(t, start, end) || !matches(t, agentName, p)) continue
    const date = t.started_at.slice(0, 10)
    const cur = byDay.get(date) ?? { spend: 0, ops: 0 }
    cur.spend += t.cost_usd
    cur.ops += 1
    byDay.set(date, cur)
  }
  const max = Math.max(0, ...[...byDay.values()].map((v) => v.spend))
  const days = [...byDay.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([date, v]) => ({
      date,
      spend_usd: r6(v.spend),
      operations: v.ops,
      intensity: max ? v.spend / max : 0,
    }))
  const highlighted: string[] = []
  if (p.range && RANGE_HOURS[p.range]) {
    const from = now - RANGE_HOURS[p.range] * HOUR_MS
    for (const d of days) if (Date.parse(`${d.date}T23:59:59Z`) >= from) highlighted.push(d.date)
  }
  return { days, highlighted_dates: highlighted }
}

export function dayDrilldown(seed: Seed, date: string | null, p: FilterParams): FinopsDayDrilldown {
  if (!date || !/^\d{4}-\d{2}-\d{2}$/.test(date))
    throw new MockHttpError(400, 'date is required (YYYY-MM-DD)')
  const agentName = resolveAgent(seed, p.agent_id)
  const start = Date.parse(`${date}T00:00:00Z`)
  const rows = seed.traces.filter(
    (t) => inWindow(t, start, start + 24 * HOUR_MS) && matches(t, agentName, p),
  )
  // Like get_finops_spend_calendar_day: agents with no spend are dropped, and names are
  // resolved to display names through the live agents (get_agent_names excludes deleted
  // ones), so a deleted agent's slices keep the raw trace name.
  const displayByName = new Map(
    seed.agents.filter((a) => !a.deleted).map((a) => [a.name, a.display_name]),
  )
  const display = (name: string) => displayByName.get(name) ?? name
  const byAgent = new Map<string, number>()
  for (const t of rows) byAgent.set(t.agent_name, (byAgent.get(t.agent_name) ?? 0) + t.cost_usd)
  const top = [...byAgent.entries()]
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1])
    .slice(0, 4)
  const topNames = new Set(top.map(([n]) => n))
  const total = rows.reduce((s, t) => s + t.cost_usd, 0)
  const byHour = groupBy(rows, (t) => new Date(t.ts).getUTCHours())
  const hours = Array.from({ length: 24 }, (_, hour) => {
    const inHour = byHour.get(hour) ?? []
    const spend = inHour.reduce((s, t) => s + t.cost_usd, 0)
    const perAgent = new Map<string, number>()
    for (const t of inHour)
      if (topNames.has(t.agent_name))
        perAgent.set(t.agent_name, (perAgent.get(t.agent_name) ?? 0) + t.cost_usd)
    const topSpend = [...perAgent.values()].reduce((s, v) => s + v, 0)
    return {
      hour,
      spend_usd: r6(spend),
      // service.rs day drill-down (ea233d20): each hour lists the day's top agents in the day's order, those with
      // spend in that hour.
      top_agents: top
        .map(([n]) => [n, perAgent.get(n) ?? 0] as const)
        .filter(([, v]) => v > 0)
        .map(([name, v]) => ({ agent_name: display(name), spend_usd: r6(v) })),
      others_spend_usd: r6(Math.max(0, spend - topSpend)),
    }
  })
  return {
    date,
    hours,
    // The server averages the already-rounded hourly spends.
    avg_hourly_spend_usd: r6(hours.reduce((s, h) => s + h.spend_usd, 0) / 24),
    top_agents: top.map(([name, spend_usd]) => ({
      agent_name: display(name),
      spend_usd: r6(spend_usd),
    })),
    others_spend_usd: r6(total - top.reduce((s, [, v]) => s + v, 0)),
  }
}

const SORTS: Record<string, (t: SeedTrace) => number> = {
  cost: (t) => t.cost_usd,
  tokens: (t) => t.input_tokens + t.output_tokens,
  latency: (t) => t.latency_ms,
}

/** PROPOSED `/finops/top-traces` contract (plan A29). */
export function topTraces(
  seed: Seed,
  p: FilterParams & { sort_by?: string | null; limit?: string | null; offset?: string | null },
  now: number,
): TopTracesData {
  const sortKey = p.sort_by ?? 'cost'
  const sortFn = SORTS[sortKey]
  if (!sortFn) throw new MockHttpError(400, `invalid sort_by '${sortKey}'`)
  const limit = p.limit ? Number(p.limit) : 25
  const offset = p.offset ? Number(p.offset) : 0
  if (!Number.isInteger(limit) || limit < 1 || limit > 100)
    throw new MockHttpError(400, 'limit must be 1-100')
  if (!Number.isInteger(offset) || offset < 0) throw new MockHttpError(400, 'offset must be >= 0')
  const agentName = resolveAgent(seed, p.agent_id)
  const { start, end } = resolveServerWindow(p, now)
  const rows = seed.traces
    .filter((t) => inWindow(t, start, end) && matches(t, agentName, p))
    .sort(
      (a, b) =>
        sortFn(b) - sortFn(a) ||
        b.started_at.localeCompare(a.started_at) ||
        a.trace_id.localeCompare(b.trace_id),
    )
  return {
    rows: rows.slice(offset, offset + limit).map((t) => ({
      trace_id: t.trace_id,
      session_id: t.session_id,
      agent_id: t.agent_id,
      agent_name: t.agent_name,
      model: t.model,
      provider: t.provider,
      input_tokens: t.input_tokens,
      output_tokens: t.output_tokens,
      cache_read_tokens: t.cache_read_tokens,
      cache_creation_tokens: t.cache_creation_tokens,
      cost_usd: t.cost_usd,
      latency_ms: t.latency_ms,
      tool_call_count: t.tool_call_count,
      started_at: t.started_at,
    })),
    has_more: rows.length > offset + limit,
  }
}

/**
 * Savings payload for `GET /finops/savings`.
 *
 * Shaped like the real one rather than minimally: one measured category, one factor-derived
 * category sitting at zero with its reason, and a `null` percentage nowhere — because those three
 * are what the panel has to render differently, and a mock that only covers the happy path lets
 * the other two regress silently.
 */
export function savings(): SavingsData {
  const block = (over: Partial<Savings> = {}): Savings => ({
    saved_tokens: 0,
    saved_input_tokens: 0,
    saved_output_tokens: 0,
    saved_cost_usd: 0,
    actual_tokens: 0,
    actual_cost_usd: 0,
    baseline_tokens: 0,
    baseline_cost_usd: 0,
    token_reduction_pct: null,
    cost_reduction_pct: null,
    basis: 'measured',
    ...over,
  })

  return {
    window: { start: '2026-03-01T00:00:00Z', end: '2026-03-31T00:00:00Z' },
    total: block({
      saved_tokens: 1_210_000,
      saved_input_tokens: 1_210_000,
      saved_cost_usd: 91,
      actual_tokens: 4_920_000,
      actual_cost_usd: 740,
      baseline_tokens: 6_130_000,
      baseline_cost_usd: 831,
      token_reduction_pct: 19.7,
      cost_reduction_pct: 11,
      basis: 'mixed',
    }),
    by_program: [
      {
        ...block({
          saved_tokens: 1_210_000,
          saved_input_tokens: 1_210_000,
          saved_cost_usd: 91,
          token_reduction_pct: 19.7,
          cost_reduction_pct: 11,
        }),
        program: 'caveman',
        label: 'Smaller prompts',
        layers: [
          {
            ...block({ saved_tokens: 900_000, saved_input_tokens: 900_000, saved_cost_usd: 60 }),
            layer: 'compress_payload',
          },
          {
            ...block({ saved_tokens: 310_000, saved_input_tokens: 310_000, saved_cost_usd: 31 }),
            layer: 'compress_history',
          },
        ],
      },
      {
        ...block({ basis: 'seed_default' }),
        program: 'ponytail',
        label: 'Less code written',
        note: 'No coding agent has this turned on.',
        layers: [
          {
            ...block({ basis: 'seed_default' }),
            layer: 'minimal_code',
            factor: {
              input_token_delta_pct: -30,
              output_token_delta_pct: -30,
              basis: 'seed_default',
              measured_at: '2026-03-01T00:00:00Z',
              notes: 'Seed, replaced by the holdout once the sample floor is cleared.',
              eligible_input_tokens: 0,
              eligible_output_tokens: 0,
            },
          },
        ],
      },
    ],
    by_agent: [
      {
        ...block({
          saved_tokens: 1_210_000,
          saved_input_tokens: 1_210_000,
          saved_cost_usd: 91,
          token_reduction_pct: 24.59,
          cost_reduction_pct: 12.3,
        }),
        agent_id: 'sample-1',
        agent_name: 'Support Bot',
        calls: 8214,
        input_tokens_before: 4_920_000,
        input_tokens_after: 3_710_000,
      },
    ],
    by_session: [
      {
        ...block({
          saved_tokens: 820_000,
          saved_input_tokens: 820_000,
          saved_cost_usd: 61,
          token_reduction_pct: 22.4,
          cost_reduction_pct: 9.8,
        }),
        session_id: 'ctx-9f3ad21b0c74',
        started_at: '2026-03-14T09:12:00Z',
        turn_count: 18,
        agent_names: ['Support Bot'],
      },
    ],
    coverage: {
      calls_in_window: 8214,
      calls_with_any_layer_enabled: 8214,
      agents_total: 22,
      agents_optimized: 1,
      agents_with_compress_enabled: 1,
      agents_with_minimal_code_enabled: 0,
      agents_with_prompt_comments: 0,
      optimized_spend_usd: 740,
      unoptimized_spend_usd: 3204,
      top_unoptimized: {
        agent_id: 'sample-5',
        agent_name: 'Sales Assistant',
        spend_usd: 1492,
      },
      calibrated_pct: 100,
    },
  }
}
