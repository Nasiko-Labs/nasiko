/**
 * R2 budgets, pure logic (plans/feat-llm-router.md §5, §5.1): the forecast band (TokenOps' `summarizeMonth` over the
 * status `daily` series), the row view (state badge, bar, over-forecast), editor rules, and which scopes a new budget
 * can take. Components stay thin; `budgets.test.ts` covers this file.
 */
import { FORECAST_MIN_DAYS, summarizeMonth } from '@/features/tokenops/forecast'
import { utcMonthStart } from '@/features/tokenops/window'
import { copy } from './copy'
import type { Budget, BudgetAction, BudgetState, BudgetStatus, UpdateBudgetBody } from './types'

export { DEFAULT_THRESHOLDS } from './tuning'

export interface Forecast {
  /** Hidden before a few days of spend (TokenOps' FORECAST_MIN_DAYS) or with no spend. */
  show: boolean
  /** Why it's hidden: too early in the month, or no spend yet. */
  hidden: 'early' | 'no-spend' | null
  low: number | null
  high: number | null
  /** The high end passes the limit. */
  overLimit: boolean
}

/**
 * Month-end band for one budget, from the status' daily series (the server returns no forecast, R-L10). `now` must be
 * when the status was read (the query's `dataUpdatedAt`), so the elapsed days and the series agree.
 */
export function budgetForecast(status: BudgetStatus, limit: number, now: Date): Forecast {
  const days = status.daily.map((d) => ({
    date: d.date,
    spend_usd: d.cost_usd,
    operations: 0,
    intensity: 0,
  }))
  const m = summarizeMonth(days, [], now)
  const hidden = m.show ? null : m.elapsedDays < FORECAST_MIN_DAYS ? 'early' : 'no-spend'
  return {
    show: m.show,
    hidden,
    low: m.low,
    high: m.high,
    overLimit: m.show && m.high !== null && m.high > limit,
  }
}

/** The period's reset: the status' `resets_at`, else the first of next UTC month. */
export const budgetResetsAt = (status: BudgetStatus[] | undefined, now: Date) =>
  status?.[0]?.resets_at ?? utcMonthStart(now, 1).toISOString()

export type StateView = { kind: BudgetState; label: string }

export function stateView(status: BudgetStatus | undefined): StateView | null {
  if (!status) return null
  if (status.state === 'exceeded') return { kind: 'exceeded', label: copy.budgetExceeded }
  if (status.state === 'warning')
    return { kind: 'warning', label: copy.budgetWarning(status.crossed ?? 0) }
  return { kind: 'ok', label: copy.budgetOk }
}

/** Bar fill 0-100, capped: over the limit still reads as full. */
export const usedPercent = (used: number, limit: number) =>
  limit > 0 ? Math.min(100, Math.max(0, (used / limit) * 100)) : 0

/** "You (calls billed to you)" or the agent's display name; a deleted or unknown agent keeps its id visible. */
export function budgetLabel(b: Budget, agentName: (id: string) => string | undefined): string {
  if (b.scope === 'owner') return copy.budgetOwner
  return (b.agent_id && agentName(b.agent_id)) || copy.budgetUnknownAgent(b.agent_id ?? '')
}

/** The editor's threshold rules, as the contract validates them (whole percents 1-100, unique). */
export function thresholdError(values: readonly number[]): string | null {
  if (!values.length) return copy.thresholdsRequired
  if (values.some((v) => !Number.isInteger(v) || v < 1 || v > 100)) return copy.thresholdRange
  if (new Set(values).size !== values.length) return copy.thresholdDuplicate
  return null
}

/** A threshold typed into "Add threshold": a whole percent, or null (the chip isn't added). */
export function parseThreshold(text: string): number | null {
  const v = Number(text.trim().replace(/%$/, ''))
  return Number.isInteger(v) && v >= 1 && v <= 100 ? v : null
}

/** "$1,250.50" → 1250.5, in cents; NaN when it isn't a number. */
export const parseLimit = (text: string) =>
  Math.round(Number(text.trim().replace(/^\$/, '').replace(/,/g, '')) * 100) / 100

export function limitError(text: string): string | null {
  if (!text.trim()) return copy.required
  // Checked after rounding: "0.004" would be sent as 0.
  const v = parseLimit(text)
  return Number.isFinite(v) && v >= 0.01 ? null : copy.limitPositive
}

/** "Switch to Alert only": the same budget with the action flipped (PUT is a full replace, guarded by updated_at). */
export const toAlertOnly = (b: Budget): UpdateBudgetBody => ({
  limit_usd: b.limit_usd,
  thresholds: b.thresholds,
  action: 'alert',
  expected_updated_at: b.updated_at,
})

/** A `stop` budget always alerts at 100 (R-L10), so its 100% chip can't be removed. */
export const withStopMark = (thresholds: readonly number[], action: BudgetAction) =>
  action === 'stop' && !thresholds.includes(100)
    ? [...thresholds, 100].sort((a, b) => a - b)
    : [...thresholds]

/** Owner-aware subject for sentences: null means "your budget" (the copy words it). */
export const sentenceName = (b: Budget, agentName: (id: string) => string | undefined) =>
  b.scope === 'owner' ? null : budgetLabel(b, agentName)

/** Scopes a new budget can take: the owner budget once, and each owned agent without one. */
export function openScopes(
  budgets: readonly Budget[],
  agents: readonly { id: string; name: string }[],
) {
  const taken = new Set(budgets.filter((b) => b.scope === 'agent').map((b) => b.agent_id))
  return {
    owner: !budgets.some((b) => b.scope === 'owner'),
    agents: agents.filter((a) => !taken.has(a.id)),
  }
}

/** Budgets in display order: the owner budget, then agents by name. */
export function sortBudgets(
  budgets: readonly Budget[],
  agentName: (id: string) => string | undefined,
): Budget[] {
  return [...budgets].sort((a, b) =>
    a.scope === b.scope
      ? budgetLabel(a, agentName).localeCompare(budgetLabel(b, agentName))
      : a.scope === 'owner'
        ? -1
        : 1,
  )
}
