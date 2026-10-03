/**
 * Fleet health (plans/feat-overview.md §5): each agent gets Healthy / Watch / Needs action / Unknown from explainable
 * rules, and its worst dimension wins. Thresholds are relative to the agent's own history (tuning.ts), never a
 * fleet-wide bar. Pure: `useFleetHealth()` (api.ts) gathers the inputs, Overview and the Agents catalog render the
 * result (eng review R1).
 *
 * Server facts behind the inputs (nasiko-server ea233d20):
 * - There is no recent-crash rule: `crashed_at` is only set on a crashed deployment row, and a restart inserts a new
 *   row with it null, so a running agent never carries it (/ship review; R3/D4 dropped). Server gap in docs/designs.
 * - Timeseries `top_agent_name` is the raw trace name; a spike counts only when exactly one visible agent has that
 *   name (R4/D5, the AgentLink rule).
 * - There is no per-agent failing-sessions rule: the Overview checks only the newest 25 sessions (R7, R8/D9).
 */
import type { DisplayStatus } from '@/features/agents/status'
import { WATCH_CAP_MS } from '@/features/agents/tuning'
import { median } from '@/features/tokenops/stats'
import { copy } from './copy'
import {
  ACTIVITY_DROP_PCT,
  COST_PER_OP_FACTOR,
  COST_RISE_PCT,
  DAY,
  IDLE_MIN_AGE_DAYS,
  MIN_SPIKE_DAYS,
  MIN_BASE_OPS,
  P95_RISE_PCT,
  SPIKE_ACTION_FACTOR,
  SPIKE_LOOKBACK_DAYS,
} from './tuning'

export type Rating = 'healthy' | 'watch' | 'action' | 'unknown'
export type Level = 'watch' | 'action'
export type Dimension = 'reliability' | 'cost' | 'activity' | 'latency'

export interface Reason {
  dimension: Dimension
  level: Level
  text: string
  /** A stable code for reasons other code branches on (never compare `text`). */
  code?: 'budgetStopped'
}

/** One agent's finops numbers for a window; a missing dashboard row is a real zero, not unknown. */
export interface UsageWindow {
  cost: number
  operations: number
  p95: number | null
}

export interface AgentHealthInput {
  id: string
  name: string
  display: DisplayStatus
  /** The raw status, for the reason text ("crashed", "failed"). */
  raw: string
  createdAt: string
  updatedAt: string
  /** The last 7 days; `null` when the dashboard failed (the cost dimensions are then unknown). */
  current: UsageWindow | null
  /** The 7 days before; `null` when unavailable (comparisons are skipped). */
  previous: UsageWindow | null
  /** 30-day average cost per operation, or null. */
  costPerOp30d: number | null
  budget?: { stopped: boolean; exceeded: boolean }
  spike?: SpikeHit
}

export interface AgentHealth {
  id: string
  name: string
  rating: Rating
  /** Needs action first, then Watch. */
  reasons: Reason[]
}

export interface SpikeHit {
  date: string
  factor: number
}

/** Statuses that get rated (and count as deployed); the rest are Unknown. */
export const RATED_STATUSES: readonly DisplayStatus[] = ['running', 'deploying', 'attention']
const pctChange = (now: number, before: number) => ((now - before) / before) * 100

export function rateAgent(i: AgentHealthInput, now: number): AgentHealth {
  // Stopped, not deployed, unknown and harness rows have nothing to rate: Unknown is never Healthy.
  if (!RATED_STATUSES.includes(i.display))
    return { id: i.id, name: i.name, rating: 'unknown', reasons: [] }
  const reasons: Reason[] = []
  const add = (dimension: Dimension, level: Level, text: string, code?: Reason['code']) =>
    reasons.push({ dimension, level, text, ...(code ? { code } : {}) })

  // Reliability.
  if (i.display === 'attention') add('reliability', 'action', copy.reason.status(i.raw))
  if (i.display === 'deploying' && now - Date.parse(i.updatedAt) > WATCH_CAP_MS)
    add('reliability', 'action', copy.reason.stuckDeploying)

  // Cost.
  if (i.budget?.stopped) add('cost', 'action', copy.reason.budgetStopped, 'budgetStopped')
  else if (i.budget?.exceeded) add('cost', 'watch', copy.reason.overBudget)
  if (i.spike && i.spike.factor >= SPIKE_ACTION_FACTOR)
    add('cost', 'action', copy.reason.spike(i.spike))
  const cur = i.current
  const prev = i.previous
  const based = !!prev && prev.operations >= MIN_BASE_OPS
  if (cur && prev && based && prev.cost > 0) {
    const rise = pctChange(cur.cost, prev.cost)
    if (rise > COST_RISE_PCT) add('cost', 'watch', copy.reason.costUp(rise))
  }
  if (cur && i.costPerOp30d && i.costPerOp30d > 0 && cur.operations >= MIN_BASE_OPS) {
    const factor = cur.cost / cur.operations / i.costPerOp30d
    if (factor > COST_PER_OP_FACTOR) add('cost', 'watch', copy.reason.costPerOp(factor))
  }

  // Activity (never Needs action on its own).
  const oldEnough = now - Date.parse(i.createdAt) >= IDLE_MIN_AGE_DAYS * DAY
  if (cur && i.display === 'running' && oldEnough && cur.operations === 0)
    add('activity', 'watch', copy.reason.idle)
  else if (cur && prev && based && cur.operations > 0) {
    const drop = -pctChange(cur.operations, prev.operations)
    if (drop > ACTIVITY_DROP_PCT) add('activity', 'watch', copy.reason.activityDown(drop))
  }

  // Latency (optional; never Needs action).
  if (cur?.p95 && prev?.p95 && based) {
    const rise = pctChange(cur.p95, prev.p95)
    if (rise > P95_RISE_PCT) add('latency', 'watch', copy.reason.p95Up(rise))
  }

  reasons.sort((a, b) => (a.level === b.level ? 0 : a.level === 'action' ? -1 : 1))
  const rating: Rating = reasons.some((r) => r.level === 'action')
    ? 'action'
    : reasons.length
      ? 'watch'
      : cur
        ? 'healthy'
        : 'unknown'
  return { id: i.id, name: i.name, rating, reasons }
}

export interface FleetHealthSummary {
  counts: Record<Rating, number>
  byId: Map<string, AgentHealth>
  /** Watch agents, most reasons first, then by name. */
  watch: AgentHealth[]
  /** Needs-action agents, for Needs you (they don't repeat in the Fleet health card). */
  action: AgentHealth[]
}

export function summarizeFleet(rated: readonly AgentHealth[]): FleetHealthSummary {
  const counts: Record<Rating, number> = { healthy: 0, watch: 0, action: 0, unknown: 0 }
  const byId = new Map<string, AgentHealth>()
  for (const a of rated) {
    counts[a.rating]++
    byId.set(a.id, a)
  }
  const order = (x: AgentHealth, y: AgentHealth) =>
    y.reasons.length - x.reasons.length || x.name.localeCompare(y.name)
  return {
    counts,
    byId,
    watch: rated.filter((a) => a.rating === 'watch').sort(order),
    action: rated.filter((a) => a.rating === 'action').sort(order),
  }
}

/**
 * Spike days in the last week of a zero-filled daily series, by agent id: a day at or above SPIKE_ACTION_FACTOR ×
 * the median day, whose raw `top_agent_name` resolves to exactly one visible agent (R4/D5). The highest factor wins.
 */
export function spikeDrivers(
  days: readonly { date: string; spend: number; topAgent: string | null }[],
  resolve: (rawName: string) => string | undefined,
  now: number,
): Map<string, SpikeHit> {
  const out = new Map<string, SpikeHit>()
  if (days.length < MIN_SPIKE_DAYS) return out
  const typical = median(days.map((d) => d.spend))
  if (typical <= 0) return out
  const since = now - SPIKE_LOOKBACK_DAYS * DAY
  for (const d of days) {
    if (Date.parse(`${d.date}T00:00:00Z`) < since - DAY || !d.topAgent) continue
    const factor = d.spend / typical
    if (factor < SPIKE_ACTION_FACTOR) continue
    const id = resolve(d.topAgent)
    if (!id) continue
    const had = out.get(id)
    if (!had || factor > had.factor) out.set(id, { date: d.date, factor })
  }
  return out
}

/** The directory's raw-name index → an id only when exactly one visible agent has the name (AgentLink's rule). */
export function uniqueByName(
  byNameAll: ReadonlyMap<string, readonly { id: string }[]>,
): (rawName: string) => string | undefined {
  return (rawName) => {
    const hits = byNameAll.get(rawName)
    return hits?.length === 1 ? hits[0]?.id : undefined
  }
}
