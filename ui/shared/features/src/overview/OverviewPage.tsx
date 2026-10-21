// Ported from nasiko-ui-lab@1b99bdda common/src/features/overview/OverviewPage.tsx. Local edits: `"use client"`;
// `meQuery` is `meQueryOptions(scope)`; the return-tick invalidations use the scoped keys (`productKey(scope,
// 'agents')` exact, `…'agents', 'deployment'`, `routerKeys.budgets(scope)`, `…'tokenops', 'calendar'`: the lab's
// `['agents']`, `['agents', 'deployment']`, `['router', 'budgets']`, `['tokenops', 'calendar']`); the range
// `ToggleGroup type="single"` is Base UI's value array (`value={[range]}`, an emptied pick ignored as Radix's `v &&`
// did); `Card asChild` + `<section>` -> `Card render={<section …/>}`; `Button asChild` + `Link` -> `ButtonLink`
// (../agents/components/ButtonLink.tsx); the summary card is ./components/SummaryCard.tsx (development 49950eb1e, shared
// with GenUI's overview view); Prettier formatting.
// The product tour (legacy ui/ on development, 3aac84f66: dd9fe0f64): the range and summary are tour targets, and
// the header has its own tour button (no top bar on desktop).
// The redesign (legacy ui/ on development, 3aac84f66: 8bf09bf29): the summary bento (./components/SummaryCard.tsx)
// and the twelve-column grid without budgets; the budgets placement is unchanged (budgets are hidden here).
"use client";

/**
 * The Overview homepage (plans/feat-overview.md): what needs you, where the money goes, and how healthy the fleet is,
 * each card linking to the page that owns it. The header's range (`?range=`, 7/30/90 days) drives the KPI row, Spend and
 * Harnesses; the other cards keep the windows their rules are defined on.
 *
 * "Now" is frozen per visit; coming back to the page 1 min+ later moves it (useReturnTick, the TokenOps rule), so the
 * windows and the cards refresh together (design review 8A). Pending requests aren't refetched here: they already
 * refetch on focus (eng review correction).
 * The grids follow the page area's width, not the viewport, because the sidebar changes it (design review 1A). The summary
 * is a bento: the headline in a lead card beside the KPI tiles (2×2) from 1100 px of content, stacked below (tiles 2
 * across from 560 px). Below it, from 1100 px, twelve columns: Needs you and Spend each span the row, then Fleet health
 * beside Recent sessions and Recent chats beside Quick actions; 2 columns from 700 px, 1 below. DOM order stays the
 * priority order (Needs you first); wide layouts place cards
 * explicitly, so nothing leaves a hole with or without budgets.
 */
import { TourButton } from "@nasiko/app-core/shell/TopBar";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ToggleGroup, ToggleGroupItem } from "@nasiko/ui/primitives";
import { RotateCw } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { PageHeader } from "@nasiko/ui/composites";
import { PageLoader } from "@nasiko/ui/composites";
import { StateCard } from "@nasiko/ui/composites";
import { Button } from "@nasiko/ui/primitives";
import { useChatSessions } from "../chat/api";
import { overviewNarrative } from "../narrative/overview";
import { meQueryOptions, productKey, useProductScope, type Me } from "@nasiko/data-access/product";
import { AGENTS_FEATURE } from "../agents/api";
import { ButtonLink } from "../agents/components/ButtonLink";
import { routerKeys } from "../router/api";
import { TOKENOPS_FEATURE } from "../tokenops/api";
import { useReturnTick } from "@nasiko/app-core/useReturnTick";
// import { fmtMonthYear } from '@nasiko/ui/lib' // Budgets hidden: used only by the Budgets card.
// import { useBudgetCard, useFleetHealth, useHarnessSummary, useNeedsYou, useSpend } from './api'
import {
  useFleetHealth,
  useHarnessSummary,
  useNeedsYou,
  useOverviewSavings,
  useSpend,
} from "./api";
// Budgets hidden: no server support for /api/budgets yet (R-L10). Restore when it lands.
// import { Budget } from './components/Budget'
import { FirstRun, FirstRunLead, FirstRunPreview } from "./components/FirstRun";
import { FleetHealth } from "./components/FleetHealth";
import { HarnessesLine } from "./components/Harnesses";
import { NeedsYou } from "./components/NeedsYou";
import { usePausedFlows } from "../flows/paused";
import { QuickActions } from "./components/QuickActions";
import { RecentChats } from "./components/RecentChats";
import { RecentSessions } from "./components/RecentSessions";
import { SetupGuide } from "./components/SetupGuide";
import { Spend } from "./components/Spend";
import { SummaryCard } from "./components/SummaryCard";
import { copy } from "./copy";
import { DEFAULT_RANGE, RANGE_DAYS, RANGES, type OverviewSearch } from "./search";
import { RETURN_REFRESH_MS } from "./tuning";

type SetSearch = (patch: Partial<OverviewSearch>) => void;

function Frame({
  description,
  actions,
  children,
}: {
  description?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="@container/overview mx-auto flex w-full max-w-page flex-col gap-4">
      <PageHeader title={copy.title} description={description} actions={actions} />
      {children}
    </div>
  );
}

function HeaderActions({
  search,
  setSearch,
  setup,
}: {
  search: OverviewSearch;
  setSearch: SetSearch;
  /** False on first run: the Setup guide (or deploy) card is on the page already. */
  setup: boolean;
}) {
  return (
    <>
      <ToggleGroup
        data-tour="overview-range"
        variant="outline"
        size="sm"
        value={[search.range ?? DEFAULT_RANGE]}
        onValueChange={(vs) => {
          const v = vs[0] as OverviewSearch["range"];
          if (v) setSearch({ range: v === DEFAULT_RANGE ? undefined : v });
        }}
        aria-label={copy.range.label}
      >
        {RANGES.map((r) => (
          <ToggleGroupItem
            key={r}
            value={r}
            aria-label={copy.range.item(RANGE_DAYS[r])}
            className="px-3 text-xs pointer-coarse:min-h-11"
          >
            {r}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      {setup ? <SetupGuide /> : null}
      {/* No top bar here on desktop (phones keep it, with its own button). */}
      <TourButton className="max-md:hidden" />
    </>
  );
}

/** A first-run row that spans the grid at every width. */
const FULL_ROW = "@[700px]/overview:col-span-2 @[1100px]/overview:col-span-3";

/**
 * Where each card sits from 1100 px (4 columns) and from 700 px (2), with and without budgets. Literal class names, so
 * Tailwind sees them.
 */
const PLACE = {
  budgets: {
    needs:
      "@[700px]/overview:col-span-2 @[1100px]/overview:col-span-1 @[1100px]/overview:col-start-4 @[1100px]/overview:row-start-1",
    spend:
      "@[700px]/overview:col-span-2 @[1100px]/overview:col-span-3 @[1100px]/overview:col-start-1 @[1100px]/overview:row-start-1 @[1100px]/overview:row-span-2",
    budget: "@[1100px]/overview:col-start-4 @[1100px]/overview:row-start-2",
    health:
      "@[1100px]/overview:col-span-2 @[1100px]/overview:col-start-3 @[1100px]/overview:row-start-4",
    sessions:
      "@[700px]/overview:col-span-2 @[1100px]/overview:col-span-3 @[1100px]/overview:col-start-1 @[1100px]/overview:row-start-3",
    chats:
      "@[1100px]/overview:col-span-2 @[1100px]/overview:col-start-1 @[1100px]/overview:row-start-4",
    actions: "@[1100px]/overview:col-start-4 @[1100px]/overview:row-start-3",
  },
  // Redesign 2026-10-13: twelve columns from 1100 px. Needs you and Spend span the row; then Fleet health beside Recent
  // sessions and Recent chats beside Quick actions, alternating widths. Heights match within a row, so no card
  // stretches far past its content.
  none: {
    needs: "@[700px]/overview:col-span-2 @[1100px]/overview:col-span-12",
    spend: "@[700px]/overview:col-span-2 @[1100px]/overview:col-span-12",
    budget: "",
    health: "@[700px]/overview:col-span-2 @[1100px]/overview:col-span-5",
    sessions: "@[700px]/overview:col-span-2 @[1100px]/overview:col-span-7",
    chats: "@[700px]/overview:col-span-2 @[1100px]/overview:col-span-7",
    actions: "@[700px]/overview:col-span-2 @[1100px]/overview:col-span-5",
  },
} as const;

export function OverviewPage({
  search,
  setSearch,
}: {
  search: OverviewSearch;
  setSearch: SetSearch;
}) {
  const meQ = useQuery(meQueryOptions(useProductScope()));
  const me = meQ.data;
  // A non-401 failure (server down, 5xx) must not spin forever; /status explains how to start the server (/ship review).
  if (!me && meQ.isError) {
    return (
      <Frame>
        <div data-testid="overview-me-error">
          <StateCard
            tone="error"
            title={copy.meFailed}
            action={
              <div className="flex flex-wrap gap-2">
                <ButtonLink href="/status" size="sm" variant="outline">
                  {copy.checkStatus}
                </ButtonLink>
                <Button size="sm" variant="outline" onClick={() => void meQ.refetch()}>
                  <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
                </Button>
              </div>
            }
          />
        </div>
      </Frame>
    );
  }
  if (!me)
    return (
      <Frame>
        <PageLoader label={copy.loadingPage} />
      </Frame>
    );
  return <Overview me={me} search={search} setSearch={setSearch} />;
}

function Overview({
  me,
  search,
  setSearch,
}: {
  me: Me;
  search: OverviewSearch;
  setSearch: SetSearch;
}) {
  const range = search.range ?? DEFAULT_RANGE;
  const days = RANGE_DAYS[range];
  const qc = useQueryClient();
  const scope = useProductScope();
  const returnTick = useReturnTick(RETURN_REFRESH_MS);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const now = useMemo(() => new Date(), [returnTick]);
  useEffect(() => {
    if (!returnTick) return;
    // Window-keyed queries (finops, sessions) move with `now`; these keys don't, so refresh them.
    void qc.invalidateQueries({ queryKey: productKey(scope, AGENTS_FEATURE), exact: true });
    void qc.invalidateQueries({ queryKey: productKey(scope, AGENTS_FEATURE, "deployment") });
    void qc.invalidateQueries({ queryKey: routerKeys.budgets(scope) });
    // The month calendar is keyed by month, not window: refresh it too, as TokenOps does (/ship review).
    void qc.invalidateQueries({ queryKey: productKey(scope, TOKENOPS_FEATURE, "calendar") });
  }, [returnTick, qc, scope]);

  const fleet = useFleetHealth(now);
  // A failed agent list must not freeze the other cards: they load, and Needs you says agent health failed (/ship review).
  const ready = !!fleet.error || (!!fleet.summary && fleet.agentCount > 0);
  // First run: the directory answered with no agents but harnesses (design review 7A).
  const firstRun = !!fleet.summary && fleet.agentCount === 0;
  const spend = useSpend(now, range, ready);
  const savings = useOverviewSavings(now, range, ready);
  const needsYou = useNeedsYou(now, me, fleet, ready);
  const harnesses = useHarnessSummary(now, me, range);
  // The Recent chats card's own query (same key): first paint waits for it too.
  const chats = useChatSessions();
  // Budgets hidden: no server support for /api/budgets yet (R-L10). Restore when it lands.
  // const budget = useBudgetCard(fleet, ready)
  // const budgets = !budget.absent
  const budgets = false;
  const { needs } = needsYou;
  // Read only when there are requests to link (plans/feat-flows.md O2, amended 2026-10-06).
  const pausedFlows = usePausedFlows(needs.rows.some((r) => r.kind === "request"));
  const narrative = useMemo(
    () =>
      overviewNarrative({
        month: spend.summary,
        unpriced: spend.unpriced,
        actionCount: needs.actionCount,
        waitingCount: needs.waitingCount,
        otherCount: needs.rows.filter((r) => r.kind === "budget" || r.kind === "sessions").length,
        empty: needs.empty,
      }),
    [spend.summary, spend.unpriced, needs],
  );

  const grid =
    "grid grid-cols-1 items-stretch gap-3 @[700px]/overview:grid-cols-2 @[1100px]/overview:grid-cols-3";
  const description = copy.greeting(now.getHours(), me.username, days);
  const actions = <HeaderActions search={search} setSearch={setSearch} setup={!firstRun} />;
  // One page loader until the first paint's reads settle (data or error: a failed read renders its card's error), in
  // place of every card's skeleton. Latched: a later range change or return tick keeps the cards' own loading states.
  // Needs you's rating and session checks aren't waited for: its card shows them loading (a fan-out can be slow).
  const coldPending =
    (!fleet.summary && !fleet.error) ||
    harnesses.isPending ||
    (!firstRun &&
      (spend.isPending ||
        (!spend.totals && !spend.totalsError) ||
        needsYou.sessions.recent.isPending ||
        chats.isPending));
  const [painted, setPainted] = useState(false);
  if (!painted && !coldPending) setPainted(true);
  if (!painted && coldPending) {
    return (
      <Frame description={description} actions={actions}>
        <PageLoader label={copy.loadingPage} />
      </Frame>
    );
  }
  if (firstRun) {
    return (
      <Frame description={description} actions={actions}>
        <FirstRunLead />
        <div className={grid}>
          {/* From 1100 px Quick actions and Harnesses' one line stack beside the first-run card, so neither stretches
              to its height; below that each spans the grid. The preview of later cards sits apart, after a wider gap. */}
          <FirstRun />
          <div className="contents @[1100px]/overview:flex @[1100px]/overview:flex-col @[1100px]/overview:gap-3">
            <QuickActions firstRun className={`${FULL_ROW} @[1100px]/overview:flex-1`} />
            <HarnessesLine data={harnesses} days={days} className={FULL_ROW} />
          </div>
          <FirstRunPreview className={`${FULL_ROW} mt-3`} />
        </div>
      </Frame>
    );
  }
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- `budgetsAbsent` is the lab's deliberate constant until /api/budgets ships; the branch is kept for that server
  const place = budgets ? PLACE.budgets : PLACE.none;
  return (
    <Frame description={description} actions={actions}>
      <SummaryCard
        days={days}
        narrative={narrative}
        nothing={needs.empty}
        spend={spend}
        savings={savings}
        fleet={fleet}
        harnesses={harnesses}
      />
      <div className="grid grid-cols-1 items-stretch gap-3 @[700px]/overview:grid-cols-2 @[1100px]/overview:grid-cols-12">
        <NeedsYou
          data={needsYou}
          now={now.getTime()}
          userId={me.sub}
          pausedFlows={pausedFlows}
          className={place.needs}
        />
        <Spend spend={spend} now={now.getTime()} className={place.spend} />
        {/* Budgets hidden: no server support for /api/budgets yet (R-L10). Restore when it lands.
        {budgets ? (
          <Budget data={budget} month={fmtMonthYear(now)} className={place.budget} />
        ) : null}
        */}
        <FleetHealth fleet={fleet} className={place.health} />
        <RecentSessions
          sessions={needsYou.sessions}
          displayName={fleet.displayName}
          now={now.getTime()}
          className={place.sessions}
        />
        <RecentChats
          byId={fleet.byId}
          username={me.username}
          now={now.getTime()}
          className={place.chats}
        />
        <QuickActions budgets={budgets} className={place.actions} />
      </div>
    </Frame>
  );
}
