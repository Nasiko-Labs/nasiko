/**
 * The Overview's summary bento, shared with Weave's catalog view: the headline, month bar and token optimisation in the
 * lead card (the page's one effect area) beside Spend, Tokens, Agent runs and Harnesses, 2×2 (redesign 2026-10-19: the
 * tiles changed, the lead card didn't).
 */
import { Badge } from '@nasiko/ui/primitives/badge'
import { SpotlightCard } from '@nasiko/ui/composites/spotlight-card'
import { Reveal, RevealItem } from '@nasiko/ui/composites/reveal'
import type { OverviewNarrative } from '@nasiko/features/narrative/overview'
import type { OptimisationView } from '@nasiko/features/tokenops/optimisation'
import type { HarnessSummary, Spend } from '../api'
import { copy } from '../copy'
import { Harnesses } from './Harnesses'
import { Headline } from './Headline'
import { RunsTile, SpendTile, TokensTile } from './Kpis'
import { MonthBar } from './MonthBar'
import { Savings } from './Savings'

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
              <Savings view={savings.view} isPending={savings.isPending} />
            </div>
          </SpotlightCard>
        </RevealItem>
        <div className="grid grid-cols-1 gap-3 @[560px]/overview:grid-cols-2 @[1100px]/overview:col-span-6">
          <RevealItem className="flex *:flex-1">
            <SpendTile spend={spend} />
          </RevealItem>
          <RevealItem className="flex *:flex-1">
            <TokensTile spend={spend} />
          </RevealItem>
          <RevealItem className="flex *:flex-1">
            <RunsTile spend={spend} />
          </RevealItem>
          <RevealItem className="flex *:flex-1">
            <Harnesses data={harnesses} days={days} />
          </RevealItem>
        </div>
      </Reveal>
    </section>
  )
}
