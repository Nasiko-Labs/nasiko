import type { AgentFinopsRow, FinopsAttributions, WorkflowFinopsRow } from './types'
import { OTHER_SERIES, seriesAt, type Series } from '@nasiko/ui/lib/chart'
import type { SortKey } from './search'

/**
 * "Who drives cost" rows (plan F3, A12, A14).
 *
 * Current and previous windows come from two dashboard queries (both ACL-scoped);
 * rows are joined by stable id (`agent_id` / `maf_id`), never by name, because the
 * dashboard's `agent_name` is a display name that can change.
 */
type DeltaKind = 'change' | 'new' | 'no-spend' | 'unavailable'

export interface AttributionRow {
  id: string
  name: string
  kind: 'agent' | 'workflow'
  cost: number
  sharePct: number
  deltaPct: number | null
  deltaKind: DeltaKind
  tokens: number
  operations: number
  costPerOp: number
  /**
   * Agents: p50 latency. Workflows: the server's arithmetic mean
   * (`WorkflowFinopsRow.avg_latency_ms`) — label it "Avg", not p50.
   */
  latency: number | null
  p95: number | null
  /** Cache-read tokens as a share of input tokens, 0–100. */
  cacheRatioPct: number | null
  containerHours: number | null
  capped: boolean
}

interface Normalized {
  id: string
  name: string
  cost: number
  tokens: number
  operations: number
  costPerOp: number
  latency: number | null
  p95: number | null
  prompt: number
  cacheRead: number
  hours: number | null
  capped: boolean
}

function fromAgent(r: AgentFinopsRow): Normalized {
  return {
    id: r.agent_id,
    name: r.agent_name,
    cost: r.total_cost,
    tokens: r.total_tokens,
    operations: r.operations,
    costPerOp: r.avg_cost_per_operation,
    latency: r.avg_latency_ms,
    p95: r.avg_latency_p95_ms,
    prompt: r.prompt_tokens,
    cacheRead: r.cache_read_tokens,
    hours: r.container_hours,
    capped: r.is_capped,
  }
}

function fromWorkflow(r: WorkflowFinopsRow): Normalized {
  return {
    id: r.maf_id,
    name: r.workflow_name,
    cost: r.total_cost,
    tokens: r.total_tokens,
    operations: r.executions,
    costPerOp: r.avg_cost_per_execution,
    latency: r.avg_latency_ms,
    p95: null,
    prompt: r.prompt_tokens,
    cacheRead: r.cache_read_tokens,
    hours: null,
    capped: false,
  }
}

function normalize(a: FinopsAttributions): Normalized[] {
  return a.view === 'agent' ? a.rows.map(fromAgent) : a.rows.map(fromWorkflow)
}

/**
 * Cache-read tokens as a share of input, 0–100. Providers count differently: OpenAI's `prompt_tokens` include the
 * cached ones, Anthropic's `input_tokens` exclude them (so cached can be many times "prompt", which printed
 * 6550732.8%). More cached than prompt can only mean they were counted apart, so add them back.
 * ponytail: per-row heuristic; an Anthropic row with fewer cached than uncached tokens still overstates. Exact once
 * the dashboard row says whether prompt_tokens include cache reads (server recommendation).
 */
export function cacheShare(prompt: number, cacheRead: number): number | null {
  const input = cacheRead > prompt ? prompt + cacheRead : prompt
  return input > 0 ? (cacheRead / input) * 100 : null
}

/** `previous` is undefined while loading or when its query failed → Δ "unavailable". */
export function buildAttribution(
  current: FinopsAttributions,
  previous: FinopsAttributions | undefined,
): AttributionRow[] {
  const rows = normalize(current)
  const total = rows.reduce((s, r) => s + r.cost, 0)
  const prevById =
    previous && previous.view === current.view
      ? new Map(normalize(previous).map((r) => [r.id, r.cost]))
      : null

  return rows.map((r) => {
    let deltaKind: DeltaKind = 'unavailable'
    let deltaPct: number | null = null
    if (prevById) {
      const prev = prevById.get(r.id) ?? 0
      if (prev === 0 && r.cost > 0) deltaKind = 'new'
      else if (r.cost === 0 && prev > 0) {
        deltaKind = 'no-spend'
        deltaPct = -100
      } else if (prev === 0 && r.cost === 0) deltaKind = 'no-spend'
      else {
        deltaKind = 'change'
        deltaPct = ((r.cost - prev) / prev) * 100
      }
    }
    return {
      id: r.id,
      name: r.name,
      kind: current.view,
      cost: r.cost,
      sharePct: total > 0 ? (r.cost / total) * 100 : 0,
      deltaPct,
      deltaKind,
      tokens: r.tokens,
      operations: r.operations,
      costPerOp: r.costPerOp,
      latency: r.latency,
      p95: r.p95,
      cacheRatioPct: cacheShare(r.prompt, r.cacheRead),
      containerHours: r.hours,
      capped: r.capped,
    }
  })
}

const NUMERIC: Record<Exclude<SortKey, 'name'>, (r: AttributionRow) => number> = {
  cost: (r) => r.cost,
  tokens: (r) => r.tokens,
  operations: (r) => r.operations,
  latency: (r) => r.latency ?? -1,
  hours: (r) => r.containerHours ?? -1,
}

export function sortRows(rows: AttributionRow[], sort: SortKey): AttributionRow[] {
  const copy = [...rows]
  if (sort === 'name') return copy.sort((a, b) => a.name.localeCompare(b.name))
  const f = NUMERIC[sort]
  return copy.sort((a, b) => f(b) - f(a) || a.name.localeCompare(b.name))
}

export function searchRows(rows: AttributionRow[], q: string | undefined): AttributionRow[] {
  const needle = q?.trim().toLowerCase()
  return needle ? rows.filter((r) => r.name.toLowerCase().includes(needle)) : rows
}

/** Spend concentration: how many named slices before the rest fold into Other. */
export const TOP = 5

interface Slice {
  key: string
  name: string
  cost: number
  pct: number
  series: Series
}

/** Top five by spend, then everything else as one Other slice. */
export function concentrationSlices(rows: AttributionRow[]): Slice[] {
  const total = rows.reduce((s, r) => s + r.cost, 0)
  if (total <= 0) return []
  const sorted = [...rows].filter((r) => r.cost > 0).sort((a, b) => b.cost - a.cost)
  const slices: Slice[] = sorted.slice(0, TOP).map((r, i) => ({
    key: r.id,
    name: r.name,
    cost: r.cost,
    pct: (r.cost / total) * 100,
    series: seriesAt(i),
  }))
  const rest = sorted.slice(TOP).reduce((s, r) => s + r.cost, 0)
  if (rest > 0)
    slices.push({
      key: '__other',
      name: `Other (${sorted.length - TOP})`,
      cost: rest,
      pct: (rest / total) * 100,
      series: OTHER_SERIES,
    })
  return slices
}
