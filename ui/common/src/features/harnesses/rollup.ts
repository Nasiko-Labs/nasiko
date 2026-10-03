/**
 * Pure display math for the Harnesses page (plan §2, §6, E5): one small function per
 * metric. The server (or its mock) does the rollups; these decide what to show.
 */
import { isRealDate } from '@/lib/search'
import { DAY_MS } from '@/features/tokenops/window'
import {
  CALLOUT_MIN_GROWTH_PCT,
  CALLOUT_MIN_SCOPE_DEVS,
  HARNESSES,
  MAX_WINDOW_DAYS,
  MOSTLY_UNPRICED_RATIO,
} from './constants'
import type { HarnessId, HarnessTotals, UsageResponse, UsageRow } from './types'

export interface HarnessStyle {
  id: HarnessId
  name: string
  color: string
  /** Lines, dots and thin marks draw in the edge; a pastel fill is too light for them on white. */
  edge: string
  known: boolean
}

/** Name and colour token for a harness; unknown ids render as "Other" (Decision 2, G7). */
export function harnessStyle(id: HarnessId): HarnessStyle {
  const known = HARNESSES.find((h) => h.id === id)
  return known
    ? { id, name: known.name, color: known.color, edge: known.edge, known: true }
    : {
        id,
        name: 'Other',
        color: 'var(--chart-other)',
        edge: 'var(--chart-other-edge)',
        known: false,
      }
}

/** Known harnesses first, in the fixed order; unknown ids after, alphabetically. */
export function orderHarnesses(ids: Iterable<HarnessId>): HarnessId[] {
  const set = new Set(ids)
  const known = HARNESSES.map((h) => h.id as string).filter((id) => set.has(id))
  const other = [...set].filter((id) => !known.includes(id)).sort()
  return [...known, ...other]
}

const ratio = (n: number, d: number): number | null => (d > 0 ? n / d : null)

/** active / scope_devs; null when nobody is in scope. */
export const adoption = (t: HarnessTotals) => ratio(t.active_devs, t.scope_devs)
/** Est. cost per active developer; null ("—") at zero. */
export const costPerActiveDev = (t: HarnessTotals) => ratio(t.cost_usd, t.active_devs)
/** Est. cost per session; null ("—") at zero. */
export const costPerSession = (t: HarnessTotals) => ratio(t.cost_usd, t.sessions)

/** More than MOSTLY_UNPRICED_RATIO of the turns have no price: cost reads "unpriced". */
export function mostlyUnpriced(t: Pick<HarnessTotals, 'turns' | 'unpriced_calls'>): boolean {
  return t.turns > 0 && t.unpriced_calls / t.turns > MOSTLY_UNPRICED_RATIO
}

export type CostView = { kind: 'value'; value: number } | { kind: 'unpriced' }

/** How to show a cost figure. `perHarnessUnpricedKnown=false` (live fallback) disables the unpriced rule (N9). */
export function costView(t: HarnessTotals, perHarnessUnpricedKnown = true): CostView {
  if (perHarnessUnpricedKnown && mostlyUnpriced(t)) return { kind: 'unpriced' }
  return { kind: 'value', value: t.cost_usd }
}

export type CellState = 'value' | 'idle' | 'none'

/** Team-level cell (G3): active → value; registered but idle → "idle"; neither → "—". */
export function cellState(t: HarnessTotals | undefined): CellState {
  if (!t || (t.registered_devs === 0 && t.active_devs === 0)) return 'none'
  if (t.active_devs === 0) return 'idle'
  return 'value'
}

export type Metric = 'active' | 'cost' | 'sessions'

export function metricValue(t: HarnessTotals | undefined, metric: Metric): number {
  if (!t) return 0
  return metric === 'active' ? t.active_devs : metric === 'cost' ? t.cost_usd : t.sessions
}

/** Residual rows (Unassigned / Direct members) are pinned last and never sorted (G11). */
export function splitRows(rows: UsageRow[]): { main: UsageRow[]; residual: UsageRow[] } {
  const isResidual = (r: UsageRow) => r.kind === 'unassigned' || r.kind === 'direct'
  return { main: rows.filter((r) => !isResidual(r)), residual: rows.filter(isResidual) }
}

export interface SortSpec {
  metric: Metric
  /** A harness column, or undefined for the row total. */
  column?: HarnessId
  /** Sort by label instead. */
  byLabel?: boolean
  dir: 'asc' | 'desc'
}

/** Sort the main rows; residual rows stay pinned after them in their original order. */
export function sortRows(rows: UsageRow[], spec: SortSpec): UsageRow[] {
  const { main, residual } = splitRows(rows)
  const value = (r: UsageRow) =>
    metricValue(spec.column ? r.harness_breakdown[spec.column] : r.totals, spec.metric)
  const sorted = [...main].sort((a, b) => {
    const d = spec.byLabel ? a.label.localeCompare(b.label) : value(a) - value(b)
    return (spec.dir === 'asc' ? d : -d) || a.label.localeCompare(b.label)
  })
  return [...sorted, ...residual]
}

/** Idle seats are never a callout: the summary sentence always states them (review 4c). */
export type Callout =
  | { kind: 'growth'; harness: HarnessId; pct: number }
  | { kind: 'lowest'; label: string; adoption: number }

/**
 * One callout chip, by precedence (V2, Q3): fastest-growing harness →
 * lowest-adoption unit (units with fewer than CALLOUT_MIN_SCOPE_DEVS people skipped).
 */
export function pickCallout(
  res: Pick<UsageResponse, 'totals' | 'by_harness' | 'rows'>,
): Callout | null {
  // Nothing ran: every unit ties at 0%, so singling one out would be arbitrary.
  if (res.totals.scope_devs === 0 || res.totals.active_devs === 0) return null
  const growing = res.by_harness
    .filter((h) => h.delta_pct !== null && h.delta_pct >= CALLOUT_MIN_GROWTH_PCT)
    .sort((a, b) => (b.delta_pct ?? 0) - (a.delta_pct ?? 0))[0]
  if (growing && growing.delta_pct !== null)
    return { kind: 'growth', harness: growing.harness, pct: growing.delta_pct }
  const units = res.rows
    .filter((r) => r.kind === 'unit' && r.totals.scope_devs >= CALLOUT_MIN_SCOPE_DEVS)
    .map((r) => ({ label: r.label, adoption: adoption(r.totals) ?? 0 }))
    .sort((a, b) => a.adoption - b.adoption)[0]
  return units ? { kind: 'lowest', label: units.label, adoption: units.adoption } : null
}

/** Every UTC day the window touches, oldest first ("YYYY-MM-DD"), at most MAX_WINDOW_DAYS (the
 *  newest). Charts use these, so quiet days stay visible. */
export function windowDays(start: Date, end: Date): string[] {
  const out: string[] = []
  const last = new Date(end.getTime() - 1).toISOString().slice(0, 10)
  const first = new Date(
    Math.max(
      Date.parse(`${start.toISOString().slice(0, 10)}T00:00:00Z`),
      Date.parse(`${last}T00:00:00Z`) - (MAX_WINDOW_DAYS - 1) * DAY_MS,
    ),
  )
  for (let d = first; ; d = new Date(d.getTime() + DAY_MS)) {
    const key = d.toISOString().slice(0, 10)
    if (key > last) break
    out.push(key)
  }
  return out
}

/** A custom `from` no earlier than MAX_WINDOW_DAYS before `to` (both "YYYY-MM-DD"). */
export function clampFrom(from: string | undefined, to: string | undefined): string | undefined {
  if (!from || !to || !isRealDate(from) || !isRealDate(to)) return from
  const min = new Date(Date.parse(`${to}T00:00:00Z`) - (MAX_WINDOW_DAYS - 1) * DAY_MS)
  const minKey = min.toISOString().slice(0, 10)
  return from < minKey ? minKey : from
}

/** The known harnesses `isRegistered` says no to, in display order (the "Not connected" list). */
export function unconnectedHarnesses(isRegistered: (harness: string) => boolean): string[] {
  return HARNESSES.map((h) => h.id as string).filter((h) => !isRegistered(h))
}
