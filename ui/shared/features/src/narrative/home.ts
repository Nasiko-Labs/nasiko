/**
 * The home's one sentence (plans/feat-agent-centric.md PR2; mockup A): who the rows are, what they spent in the window
 * and how many need the viewer. Templated from the totals of the rows shown (never a fleet-wide figure a member's rows
 * don't add up to); a part that hasn't loaded is left out, never guessed.
 */
import { fmtMoney } from '@nasiko/ui/lib/format'

export interface HomeNarrativeInput {
  /** `unit`: an org level (EE), named by `unitName`. */
  scope: 'yours' | 'fleet' | 'unit'
  unitName?: string
  agents: number
  harnesses: number
  /** Sum of the rows' spend; null while it loads or failed. */
  spend: number | null
  /** Some calls in the window had no price: the spend is at least this much. */
  floor?: boolean
  days: number
  /** Rows that need the viewer; null while those reads load. */
  needing: number | null
}

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`

export function homeNarrative(i: HomeNarrativeInput): string | null {
  if (!i.agents && !i.harnesses) return null
  const parts = [
    i.agents ? count(i.agents, 'agent', 'agents') : null,
    i.harnesses ? count(i.harnesses, 'harness', 'harnesses') : null,
  ].filter(Boolean)
  const who =
    i.scope === 'yours'
      ? `Your ${parts.join(' and ')}`
      : i.scope === 'unit' && i.unitName
        ? `${parts.join(' and ')} owned by ${i.unitName} members`
        : `${parts.join(' and ')} you can see`
  const money =
    i.spend === null
      ? ''
      : i.spend > 0
        ? ` spent ${i.floor ? 'at least ' : ''}${fmtMoney(i.spend)} in the last ${i.days} days`
        : // Nothing priced is not nothing spent when calls went unpriced: leave the amount out.
          i.floor
          ? ''
          : ` spent nothing in the last ${i.days} days`
  const needs = i.needing ? `; ${i.needing} ${i.needing === 1 ? 'needs' : 'need'} you` : ''
  return money || needs ? `${who}${money}${needs}.` : null
}
