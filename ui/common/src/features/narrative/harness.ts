/**
 * Harnesses summary: at most 2 sentences (plan §6, G10, V2, V7), templated, never LLM.
 *   1. Adoption: "{active} of {scope_devs} developers used a harness connected to OpenRuntime".
 *   2. Idle seats, then estimated cost (API list price). The cost clause is dropped when
 *      every harness is mostly unpriced; no share is claimed for a mostly-unpriced harness.
 * The Individual level speaks to one account ("You used 2 of your 3 connected harnesses").
 */
import { fmtMoney } from '@/lib/format'
import { copy } from '@/features/harnesses/copy'
import { harnessStyle, mostlyUnpriced } from '@/features/harnesses/rollup'
import type { UsageResponse } from '@/features/harnesses/types'

export interface HarnessNarrativeInput {
  windowLabel: string
  res: Pick<UsageResponse, 'totals' | 'by_harness'>
  /** Individual level. */
  individual?: { self: boolean; name: string }
}

const when = (l: string) =>
  /^last /i.test(l) ? `In the ${l.toLowerCase()}` : /^this /i.test(l) ? l : `In ${l}`
const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`

export function harnessNarrative(i: HarnessNarrativeInput): string[] {
  const { totals, by_harness } = i.res
  const w = when(i.windowLabel)
  // The cost clause needs at least one harness that ran with prices: an idle harness's $0.00 must
  // not stand in for an unpriced one's unknown cost (QA ISSUE-002). It then sums every known charge
  // of the harnesses that ran, and says "at least" when any of their turns had no price.
  const ran = by_harness.filter((h) => h.turns > 0)
  const partial = ran.some((h) => h.unpriced_calls > 0)
  const known = ran.reduce((n, h) => n + h.cost_usd, 0)
  const costClause = ran.some((h) => !mostlyUnpriced(h))
    ? `estimated cost ${partial ? 'at least ' : ''}${fmtMoney(known)} (API list price${partial ? '; some turns unpriced' : ''})`
    : ''

  if (i.individual) {
    const used = by_harness.filter((h) => h.active_devs > 0).length
    // Activity through a since-removed registration still counts: "connected" is registered OR used,
    // so the sentence never reads "used 2 of your 1".
    const registered = by_harness.filter((h) => h.registered_devs > 0 || h.active_devs > 0).length
    const subject = i.individual.self ? 'you' : i.individual.name
    const their = i.individual.self ? 'your' : 'their'
    if (registered === 0 && used === 0)
      return [
        i.individual.self
          ? 'You have no harness connected to OpenRuntime yet.'
          : `${subject} has no harness connected to OpenRuntime yet.`,
      ]
    const first =
      used === registered
        ? `${w}, ${subject} used ${registered === 1 ? `${their} connected harness` : `all ${registered} of ${their} connected harnesses`}.`
        : `${w}, ${subject} used ${used} of ${their} ${plural(registered, 'connected harness', 'connected harnesses')}.`
    const idle =
      totals.idle_seats > 0
        ? `${plural(totals.idle_seats, 'registered harness', 'registered harnesses')} had no activity`
        : ''
    const second = [idle, costClause].filter(Boolean).join('; ')
    return second ? [first, `${second.charAt(0).toUpperCase()}${second.slice(1)}.`] : [first]
  }

  if (totals.scope_devs === 0) return [copy.noDevelopersInScope]
  if (totals.registered_devs === 0 && totals.active_devs === 0) return [copy.noHarnesses]
  const top = [...by_harness].sort((a, b) => b.active_devs - a.active_devs)[0]
  const lead =
    by_harness.length > 1 && top && top.active_devs > 0
      ? `; ${harnessStyle(top.harness).name} leads with ${plural(top.active_devs, 'active developer')}`
      : ''
  const first = `${w}, ${totals.active_devs} of ${plural(totals.scope_devs, 'developer')} used a harness connected to OpenRuntime${lead}.`
  const idle =
    totals.idle_seats > 0 ? `${plural(totals.idle_seats, 'registered seat')} had no activity` : ''
  const second = [idle, costClause].filter(Boolean).join('; ')
  return second ? [first, `${second.charAt(0).toUpperCase()}${second.slice(1)}.`] : [first]
}
