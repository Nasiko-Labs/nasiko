/**
 * Which summary tiles have something to say (2026-11-05): a tile whose figure is zero for the range is "quiet" and gives
 * up its big zero for a sentence on what would light it up. Spend, tokens and runs all quiet are one item, not three
 * (they go quiet together when no agent ran). A tile still loading or failed is never quiet: its own skeleton or error
 * shows. Pure.
 */
import type { OptimisationView } from '@nasiko/features/tokenops/optimisation'
import type { HarnessSummary, Spend } from './api'

/** Whether the lead card's token-optimization band has a story: something saved, or something it could trim. When it
 *  hasn't, "Did you know?" takes its slot (SummaryCard). */
export const savingsShown = (view: OptimisationView | undefined): view is OptimisationView =>
  !!view && (view.tokensSaved > 0 || view.unoptimisedSpend > 0)

export type TileId = 'spend' | 'tokens' | 'runs' | 'harnesses'

export type QuietItem =
  /** No agent activity at all; `before` is the previous window's spend when it had some. */
  | { id: 'activity'; before: number | null }
  /** Runs, but no priced spend: `unpriced` when calls had no list price. */
  | { id: 'spend'; unpriced: boolean }
  | { id: 'tokens' }
  | { id: 'runs' }
  /** None connected (`connected: 0`), or connected with no turns in the range. */
  | { id: 'harnesses'; connected: number }

export interface Quiet {
  /** The tiles that keep their figure, in the summary's order. */
  tiles: TileId[]
  quiet: QuietItem[]
  /** Every input answered (data or error): a layout computed while one loads would flash and snap back. */
  settled: boolean
}

export function quietTiles(spend: Spend, harnesses: HarnessSummary): Quiet {
  const { totals, tokens } = spend
  const zero = {
    spend: !!totals && totals.spend <= 0,
    runs: !!totals && totals.runs <= 0,
    tokens: !!tokens && tokens.total <= 0,
    harnesses:
      !harnesses.isPending &&
      !harnesses.error &&
      !harnesses.notVisible &&
      harnesses.harnesses.every((h) => h.turns <= 0 && (h.cost ?? 0) <= 0),
  }
  const quiet: QuietItem[] = []
  if (zero.spend && zero.runs && zero.tokens)
    quiet.push({ id: 'activity', before: totals?.previous?.spend || null })
  else {
    if (zero.spend) quiet.push({ id: 'spend', unpriced: spend.unpriced })
    if (zero.tokens) quiet.push({ id: 'tokens' })
    if (zero.runs) quiet.push({ id: 'runs' })
  }
  if (zero.harnesses) quiet.push({ id: 'harnesses', connected: harnesses.connected })
  return {
    tiles: (['spend', 'tokens', 'runs', 'harnesses'] as const).filter((t) => !zero[t]),
    quiet,
    settled:
      (!!totals || !!spend.totalsError) &&
      (!!tokens || !!spend.driversError) &&
      !harnesses.isPending,
  }
}
