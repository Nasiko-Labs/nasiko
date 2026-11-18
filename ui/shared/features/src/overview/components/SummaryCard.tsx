/**
 * The Overview's summary bento, shared with Weave's catalog view: the headline, month bar and token optimisation in the
 * lead card (the page's one effect area) beside Spend, Tokens, Agent runs and Harnesses, 2×2 (redesign 2026-10-19: the
 * tiles changed, the lead card didn't).
 *
 * A tile with nothing in the range is quiet (2026-11-05, `quiet.ts`): it leaves the grid and the quiet panel takes the
 * room, so the 2×2 never shows a bare zero and never a hole. Four tiles: as before. Three: the panel is the fourth cell.
 * One or two: they share the top row and the panel the bottom one. None: the panel fills the 2×2, its cells stacked.
 * While a range change reloads, the last settled layout holds (the tiles show their own loading), so the grid moves at
 * most once, and the cells glide to their new places (`layout="position"`; at rest under reduced motion).
 *
 * The token-optimization band shows only beside activity the tiles show (user 2026-11-06): it reads `token_usage`,
 * written at call time, while the tiles read `trace_usage`, filled from traces minutes later, so a fresh call would
 * otherwise read "<$0.01 could be trimmed" beside "No agent activity". Until the tiles catch up, "Did you know?" takes
 * the slot.
 */
import { useState, type ReactNode } from 'react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { SpotlightCard } from '@nasiko/ui/composites/spotlight-card'
import { Reveal, RevealItem } from '@nasiko/ui/composites/reveal'
import type { OverviewNarrative } from '@nasiko/features/narrative/overview'
import type { OptimisationView } from '@nasiko/features/tokenops/optimisation'
import type { HarnessSummary, Spend } from '../api'
import { copy } from '../copy'
import { Harnesses } from './Harnesses'
import { Sampled } from './HarnessPreview'
import { Headline } from './Headline'
import { RunsTile, SpendTile, TokensTile } from './Kpis'
import { cn } from '@nasiko/ui/lib/utils'
import { usePreviewSample } from '../preview'
import { quietTiles, savingsShown, type Quiet, type TileId } from '../quiet'
import { MonthBar } from './MonthBar'
import { DidYouKnow } from './DidYouKnow'
import { QuietPanel } from './QuietPanel'
import { Savings } from './Savings'

const sameLayout = (a: Quiet, b: Quiet) =>
  JSON.stringify([a.tiles, a.quiet]) === JSON.stringify([b.tiles, b.quiet])

export function SummaryCard({
  days,
  narrative,
  nothing,
  spend,
  savings,
  harnesses,
}: {
  days: number
  narrative: OverviewNarrative
  nothing: boolean
  spend: Spend
  savings: { view: OptimisationView | undefined; isPending: boolean }
  harnesses: HarnessSummary
}) {
  // While Home's coding harness preview plays past Today, the Harnesses tile is Home's own on sample numbers.
  const sample = usePreviewSample()
  const computed = quietTiles(spend, sample ?? harnesses)
  const [settled, setSettled] = useState(computed)
  if (computed.settled && !sameLayout(computed, settled)) setSettled(computed)
  const { tiles, quiet } = computed.settled ? computed : settled
  // Band or tip once both the savings read and the tiles' layout are known, so the two never swap.
  const lead =
    savings.isPending || !(computed.settled || settled.settled)
      ? null
      : savingsShown(savings.view) && !quiet.some((q) => q.id === 'activity')
        ? 'band'
        : 'tip'
  const [now] = useState(() => Date.now())
  const tile: Record<TileId, ReactNode> = {
    spend: <SpendTile spend={spend} />,
    tokens: <TokensTile spend={spend} />,
    runs: <RunsTile spend={spend} />,
    harnesses: sample ? (
      <Sampled>
        <Harnesses data={sample} days={days} />
      </Sampled>
    ) : (
      <Harnesses data={harnesses} days={days} />
    ),
  }
  // One or two tiles share the top row (one alone takes it), the panel the row under; three leave the panel a cell.
  const wide = '@[560px]/overview:col-span-2'
  return (
    // The summary as a bento, as on TokenOps and Optimization: the headline in the lead card (the page's one effect
    // area), the four KPI tiles beside it, 2×2.
    <section
      data-tour="overview-summary"
      aria-labelledby="overview-summary-title"
      className="@container/overview"
    >
      <h2 id="overview-summary-title" className="sr-only">
        {copy.summary}
      </h2>
      <Reveal className="grid gap-3 @[1100px]/overview:grid-cols-12">
        <RevealItem className="@[1100px]/overview:col-span-6">
          <SpotlightCard className="h-full">
            <div className="relative flex flex-1 flex-col justify-center gap-4 p-5">
              <Badge
                variant="outline"
                className="border-primary/30 bg-primary/10 text-primary-text"
              >
                {copy.range.item(days)}
              </Badge>
              <Headline narrative={narrative} nothing={nothing} />
              <MonthBar spend={spend} />
              {/* The band's slot when it has nothing to say, or the tiles show no activity yet (user 2026-11-06: the
                  lead card never ends in blank space): a tip. */}
              {lead === 'band' ? (
                <Savings view={savings.view} isPending={savings.isPending} />
              ) : lead === 'tip' ? (
                <DidYouKnow now={now} className="rounded-lg border border-border bg-card/80 p-4" />
              ) : null}
            </div>
          </SpotlightCard>
        </RevealItem>
        <div className="grid grid-cols-1 gap-3 @[560px]/overview:grid-cols-2 @[1100px]/overview:col-span-6">
          {tiles.map((t) => (
            <RevealItem
              key={t}
              layout="position"
              className={cn('flex *:flex-1', tiles.length === 1 && wide)}
            >
              {tile[t]}
            </RevealItem>
          ))}
          {quiet.length ? (
            <RevealItem
              key="quiet"
              layout="position"
              className={cn('flex *:flex-1', tiles.length !== 3 && wide)}
            >
              <QuietPanel items={quiet} days={days} stacked={!tiles.length} />
            </RevealItem>
          ) : null}
        </div>
      </Reveal>
    </section>
  )
}
