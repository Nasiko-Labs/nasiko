/**
 * The Overview headline (plans/feat-overview.md §10): at most two templated sentences, each backed by a card below.
 * - Money: from the fleet-wide calendar, so it says "The fleet has spent", never "You've spent" (eng review R2).
 * - Attention: from Needs you's counts. A clause whose data is missing is left out, and "Nothing needs you" is only
 *   said when every Needs-you source answered with nothing (design review 5A): never while loading or after a failure.
 */
import { copy } from '@/features/overview/copy'
import { fmtMoney } from '@/lib/format'

export interface OverviewNarrativeInput {
  month: {
    mtd: number
    low: number | null
    high: number | null
    show: boolean
    vsLastMonthPct: number | null
  } | null
  /** Some calls had no price: the total is a floor, said "at least" (as the Spend card's ≥). */
  unpriced?: boolean
  /** Needs-action agents, waiting requests and other items (budgets, failing sessions); null when unknown. */
  actionCount: number | null
  waitingCount: number | null
  otherCount: number
  /** Every Needs-you source answered with nothing. */
  empty: boolean
}

export interface OverviewNarrative {
  money: string | null
  attention: string | null
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`

export function overviewNarrative(i: OverviewNarrativeInput): OverviewNarrative {
  let money: string | null = null
  if (i.month) {
    if (i.month.mtd <= 0) money = 'The fleet has no spend yet this month.'
    else {
      const pace =
        i.month.show && i.month.low !== null && i.month.high !== null
          ? `, on pace for ${fmtMoney(i.month.low)}–${fmtMoney(i.month.high)}`
          : ''
      const vs =
        i.month.vsLastMonthPct === null
          ? ''
          : ` (${i.month.vsLastMonthPct >= 0 ? 'up' : 'down'} ${Math.abs(Math.round(i.month.vsLastMonthPct))}% vs the same days last month)`
      money = `The fleet has spent ${i.unpriced ? 'at least ' : ''}${fmtMoney(i.month.mtd)} this month${pace}${vs}.`
    }
  }

  let attention: string | null = null
  if (i.empty) attention = copy.needs.nothing
  else {
    const parts: string[] = []
    if (i.actionCount) parts.push(`${plural(i.actionCount, 'agent needs', 'agents need')} action`)
    if (i.waitingCount)
      parts.push(`${plural(i.waitingCount, 'request is', 'requests are')} waiting for you`)
    if (i.otherCount)
      parts.push(`${plural(i.otherCount, 'other item needs', 'other items need')} a look`)
    if (parts.length)
      attention = `${parts.length > 1 ? `${parts.slice(0, -1).join(', ')} and ${parts.at(-1)}` : parts[0]}.`
    attention = attention ? attention.charAt(0).toUpperCase() + attention.slice(1) : null
  }
  return { money, attention }
}
