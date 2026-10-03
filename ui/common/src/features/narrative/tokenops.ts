/**
 * TokenOps executive summary: at most 3 sentences (plan: design P6 + DX precedence).
 *   1. Spend and change ("at least" when calls are unpriced; the change clause is left
 *      out when Compare is off or the previous window is unavailable; "new this period"
 *      when the previous window had no spend).
 *   2. The driver, when the top share is ≥ 20% and there are 2+ rows. With an agent
 *      filter, sentence 1 names the agent instead and there is no driver sentence.
 *   3. The spike: the peak day, when it exceeds SPIKE_FACTOR × the median day (day
 *      buckets only). The UI appends "See sessions →" to it.
 * Month pace lives in the figures row and the meter, not in the sentence.
 */
import { SPIKE_FACTOR } from '@/features/observability/tuning'
import { median } from '@/features/tokenops/stats'
import { fmtMoney, fmtShortDay } from '@/lib/format'

export interface TokenopsNarrativeInput {
  windowLabel: string
  total: number
  /** Previous window's total; undefined when unavailable or Compare is off. */
  previous?: number
  unpriced: boolean
  /** Rows with display name and share of spend (0–100), highest first or any order. */
  rows: { name: string; sharePct: number }[]
  agentLabel?: string
  /** Day buckets in the window (UTC date + spend). Hourly windows pass []. */
  days: { date: string; spend: number }[]
}

export interface Spike {
  date: string
  spend: number
  factor: number
}

export interface TokenopsNarrative {
  /** Every sentence; when there is a spike, its sentence is the last one. */
  sentences: string[]
  spike: Spike | null
}

export function findSpike(days: { date: string; spend: number }[]): Spike | null {
  const spend = days.map((d) => d.spend)
  if (days.length < 3) return null
  const typical = median(spend)
  const peak = days.reduce((a, b) => (b.spend > a.spend ? b : a))
  if (peak.spend <= 0) return null
  // No typical day to compare with (mostly idle window): no spike claim.
  if (typical <= 0) return null
  const factor = peak.spend / typical
  return factor > SPIKE_FACTOR ? { date: peak.date, spend: peak.spend, factor } : null
}

export function spikeSentence(s: Spike): string {
  return `Spend peaked on ${fmtShortDay(s.date)} at ${fmtMoney(s.spend)} (${s.factor.toFixed(1)}× a typical day)`
}

export function tokenopsNarrative(i: TokenopsNarrativeInput): TokenopsNarrative {
  // "Last 30 days" → "In the last 30 days"; "This month" → "This month"; others → "In March 2026".
  const l = i.windowLabel
  const when = /^last /i.test(l) ? `In the ${l.toLowerCase()}` : /^this /i.test(l) ? l : `In ${l}`
  const amount = `${i.unpriced ? 'at least ' : ''}${fmtMoney(i.total)}`
  const who = i.agentLabel ? `${i.agentLabel} spent` : 'you spent'
  let change = ''
  if (i.previous !== undefined) {
    if (i.previous === 0) change = i.total > 0 ? ', all of it new this period' : ''
    else {
      const pct = Math.round(((i.total - i.previous) / i.previous) * 100)
      change =
        pct === 0
          ? ', about the same as the period before'
          : `, ${Math.abs(pct)}% ${pct > 0 ? 'more' : 'less'} than the period before`
    }
  }
  const sentences = [`${when} ${who} ${amount}${change}.`]
  if (!i.agentLabel && i.rows.length >= 2) {
    const top = i.rows.reduce((a, b) => (b.sharePct > a.sharePct ? b : a))
    if (top.sharePct >= 20) sentences.push(`${top.name} drove ${Math.round(top.sharePct)}% of it.`)
  }
  const spike = findSpike(i.days)
  if (spike) sentences.push(`${spikeSentence(spike)}.`)
  return { sentences: sentences.slice(0, 3), spike }
}
