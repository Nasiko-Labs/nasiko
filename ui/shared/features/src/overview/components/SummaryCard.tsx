// Ported from the legacy ui/ on development (3aac84f66, 49950eb1e) common/src/features/overview/components/SummaryCard.tsx:
// the Overview's summary card, split out of OverviewPage so GenUI's overview view draws the same card. Local edits:
// the React OverviewPage's card as it was (`Card render={<section …/>}`); Prettier formatting.
// The product tour (legacy ui/ on development, 3aac84f66: dd9fe0f64): the summary is a tour target.
// The bento (legacy ui/ on development, 3aac84f66: 8bf09bf29, then in OverviewPage): the headline in a
// `SpotlightCard` beside the four tiles, each its own card; `Reveal` is `RevealGroup`.
// The tiles (2d95e6e79): Spend, Tokens, Agent runs and Harnesses; the run states moved to Fleet health.
/**
 * The Overview's summary bento, shared with the EE generated views: the headline, month bar and token optimisation in the
 * lead card (the page's one effect area) beside Spend, Tokens, Agent runs and Harnesses, 2×2 (redesign 2026-10-19: the
 * tiles changed, the lead card didn't).
 */
import { Badge } from "@nasiko/ui/primitives";
import { RevealGroup, RevealItem, SpotlightCard } from "@nasiko/ui/composites";
import type { OverviewNarrative } from "../../narrative/overview";
import type { OptimisationView } from "../../tokenops/optimisation";
import type { HarnessSummary, Spend } from "../api";
import { copy } from "../copy";
import { Harnesses } from "./Harnesses";
import { Headline } from "./Headline";
import { RunsTile, SpendTile, TokensTile } from "./Kpis";
import { MonthBar } from "./MonthBar";
import { Savings } from "./Savings";

export function SummaryCard({
  days,
  narrative,
  nothing,
  spend,
  savings,
  harnesses,
}: {
  days: number;
  narrative: OverviewNarrative;
  nothing: boolean;
  spend: Spend;
  savings: { view: OptimisationView | undefined; isPending: boolean };
  harnesses: HarnessSummary;
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
      <RevealGroup className="grid gap-3 @[1100px]/overview:grid-cols-12">
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
      </RevealGroup>
    </section>
  );
}
