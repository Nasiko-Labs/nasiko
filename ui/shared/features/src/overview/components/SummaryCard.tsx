// Ported from the legacy ui/ on development (3aac84f66, 49950eb1e) common/src/features/overview/components/SummaryCard.tsx:
// the Overview's summary card, split out of OverviewPage so GenUI's overview view draws the same card. Local edits:
// the React OverviewPage's card as it was (`Card render={<section …/>}`); `FleetHealth` comes from ../fleet.ts;
// Prettier formatting.
import { Badge } from "@nasiko/ui/primitives";
import { Card } from "@nasiko/ui/primitives";
import type { OverviewNarrative } from "../../narrative/overview";
import type { OptimisationView } from "../../tokenops/optimisation";
import type { HarnessSummary, Spend } from "../api";
import { copy } from "../copy";
import type { FleetHealth } from "../fleet";
import { Harnesses } from "./Harnesses";
import { Headline } from "./Headline";
import { AgentsTile, RunsTile, SpendTile } from "./Kpis";
import { MonthBar } from "./MonthBar";
import { Savings } from "./Savings";

export function SummaryCard({
  days,
  narrative,
  nothing,
  spend,
  savings,
  fleet,
  harnesses,
}: {
  days: number;
  narrative: OverviewNarrative;
  nothing: boolean;
  spend: Spend;
  savings: { view: OptimisationView | undefined; isPending: boolean };
  fleet: FleetHealth;
  harnesses: HarnessSummary;
}) {
  return (
    // The summary as one card like TokenOps' SummaryHero: the headline on the left, the KPI tiles 2×2 on the right.
    <Card
      render={<section aria-labelledby="overview-summary-title" />}
      className="gap-0 overflow-hidden p-0 @[1100px]/overview:grid @[1100px]/overview:grid-cols-[minmax(0,1fr)_minmax(0,1.25fr)]"
    >
      <h2 id="overview-summary-title" className="sr-only">
        {copy.summary}
      </h2>
      <div className="flex flex-col justify-center gap-4 p-5">
        <Badge variant="outline" className="border-primary/30 bg-primary/10 text-primary-text">
          {copy.range.item(days)}
        </Badge>
        <Headline narrative={narrative} nothing={nothing} />
        <MonthBar spend={spend} />
        <Savings view={savings.view} isPending={savings.isPending} />
      </div>
      {/* The tiles drop their own card chrome here: the hairlines between them are the grid's gap. */}
      <div className="grid grid-cols-1 gap-px border-t border-border bg-border *:rounded-none *:border-0 @[560px]/overview:grid-cols-2 @[1100px]/overview:border-t-0 @[1100px]/overview:border-l">
        <SpendTile spend={spend} />
        <RunsTile spend={spend} />
        <AgentsTile fleet={fleet} />
        <Harnesses data={harnesses} days={days} />
      </div>
    </Card>
  );
}
