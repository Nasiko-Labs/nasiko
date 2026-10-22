// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/HomePage.tsx. Local edits: `meQuery` is
// `meQueryOptions(scope)` and the return tick invalidates the scoped keys (`productKey(scope, 'agents')` exact, `…,
// 'builds', 'agent'`, `…, 'home', 'org'`, `…, 'budgets', 'v1'`: the lab's `['agents']`, `['builds', 'agent']`, `['home',
// 'org']`, `['budgets', 'v1']`); the persona's replace checks `usePathname()` (the lab's router state); `ToggleGroup
// type="single"` is Base UI's value array (`value={[v]}`, an emptied pick ignored as Radix's `v &&` did); the unit
// `Select` takes `items` (value → name, so the trigger shows the unit's name), a nullable value and each option's
// typeahead `label` (Radix's `textValue`); `Button asChild` + `Link` -> `ButtonLink`; the lab's `EmptyState` is
// `LabEmptyState`; optional props admit `undefined` (exactOptionalPropertyTypes); Prettier formatting.
/**
 * The scoped home at `/` (plans/feat-agent-centric.md PR2a; design "Home", mockup A: a flat roll-up strip and one dense
 * table). It replaces the Overview's card grid, keeping what the design review mapped across: Needs you on top, the
 * money as a roll-up strip, fleet health as each row's health, coding harnesses as rows, Quick actions as the header's
 * "Deploy an agent". Recent sessions moved to each agent's hub and Recent chats to Chat.
 *
 * Scope (`?scope=yours|fleet`): Yours is what the viewer owns, Fleet everything they can see. With no key the persona
 * decides (`scope.ts`), and until it does the table shows neutral skeleton rows. The window is the Overview's
 * `?range=` (7/30/90 days). "Now" is frozen per visit and moves on return, as the Overview's did.
 *
 * PR2b adds the spend sparklines, last change, budget use (a preview) and owner grouping, and two-line rows below
 * 640 px; PR2c the EE levels; PR2d the chart band between the strip and the table (`components/ChartBand`), whose health
 * bar filters the table (`?health=`).
 */
import { useSlots } from "@nasiko/app-core/edition/edition-context";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { usePathname } from "@nasiko/app-core/navigation";
import { RotateCw } from "lucide-react";
import { lazy, Suspense, useEffect, useRef, useState, type ReactNode } from "react";
import { KpiTile } from "@nasiko/ui/composites";
import { PageHeader } from "@nasiko/ui/composites";
import { PageLoader } from "@nasiko/ui/composites";
import { PanelSkeleton } from "@nasiko/ui/composites";
import { LabEmptyState as EmptyState, StateCard } from "@nasiko/ui/composites";
import { Badge } from "@nasiko/ui/primitives";
import { Button } from "@nasiko/ui/primitives";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@nasiko/ui/primitives";
import { ToggleGroup, ToggleGroupItem } from "@nasiko/ui/primitives";
import { DeployAgentButton } from "../deploy/components/DeployAgentButton";
import { usePausedFlows } from "../flows/paused";
import { homeNarrative } from "../narrative/home";
import { usePersona } from "../onboarding/api";
import { homeScopeFor } from "../onboarding/logic";
import { FirstRun, FirstRunLead, FirstRunPreview } from "../overview/components/FirstRun";
import { NeedsYou } from "../overview/components/NeedsYou";
import { SetupGuide } from "../overview/components/SetupGuide";
import { copy as overviewCopy } from "../overview/copy";
import { DEFAULT_RANGE, RANGE_DAYS, RANGES } from "../overview/search";
import { meQueryOptions, productKey, useProductScope, type Me } from "@nasiko/data-access/product";
import { AGENTS_FEATURE } from "../agents/api";
import { fmtLocalTime, fmtMoney, fmtMoneyFloor, fmtTokens } from "@nasiko/ui/lib";
import { useMediaQuery } from "@nasiko/ui/lib";
import { useFrozenNow, useReturnTick } from "@nasiko/app-core/useReturnTick";
import { budgetsShown, useHome, type HomeBudgets, type SourceStatus } from "./api";
import { filterByHealth } from "./charts";
import { PhoneRows } from "./components/PhoneRows";
import { RowsTable } from "./components/RowsTable";
import { copy } from "./copy";
import { budgetMonth, DEFAULT_SORT, sortRows, totals, type Sort } from "./rows";
import { rememberScope, resolveScope, storedScope } from "./scope";
import { isUnitScope, type HomeSearch, type Scope } from "./search";
import {
  BAND_SKELETON_PX,
  PHONE_QUERY,
  RETURN_REFRESH_MS,
  SKELETON_ROW_PX,
  SKELETON_ROWS,
  TOUCH,
} from "./tuning";
import { useBudgetStamp } from "./useBudgetStamp";
import { ButtonLink } from "../agents/components/ButtonLink";

/** The chart band is its own chunk (PR2d, #63): the chart kit loads only when it draws. */
const ChartBand = lazy(() => import("./components/ChartBand"));

type SetSearch = (patch: Partial<HomeSearch>, opts?: { replace?: boolean }) => void;

function Frame({
  description,
  actions,
  children,
}: {
  description?: ReactNode | undefined;
  actions?: ReactNode | undefined;
  children: ReactNode;
}) {
  return (
    <div className="@container/overview mx-auto flex w-full max-w-page flex-col gap-4">
      <PageHeader title={overviewCopy.title} description={description} actions={actions} />
      {children}
    </div>
  );
}

export function HomePage({ search, setSearch }: { search: HomeSearch; setSearch: SetSearch }) {
  const meQ = useQuery(meQueryOptions(useProductScope()));
  const me = meQ.data;
  if (!me && meQ.isError)
    return (
      <Frame>
        <StateCard
          tone="error"
          title={overviewCopy.meFailed}
          action={
            <div className="flex flex-wrap gap-2">
              <ButtonLink size="sm" variant="outline" href="/status">
                {overviewCopy.checkStatus}
              </ButtonLink>
              <Button size="sm" variant="outline" onClick={() => void meQ.refetch()}>
                <RotateCw className="size-3.5" aria-hidden /> {overviewCopy.retry}
              </Button>
            </div>
          }
        />
      </Frame>
    );
  if (!me)
    return (
      <Frame>
        <PageLoader label={overviewCopy.loadingPage} />
      </Frame>
    );
  return <Home me={me} search={search} setSearch={setSearch} />;
}

function Home({ me, search, setSearch }: { me: Me; search: HomeSearch; setSearch: SetSearch }) {
  const range = search.range ?? DEFAULT_RANGE;
  const days = RANGE_DAYS[range];
  const qc = useQueryClient();
  const product = useProductScope();
  const returnTick = useReturnTick(RETURN_REFRESH_MS);
  // Frozen per visit, moved on return (the Overview's rule); no hook-rule suppression, so the Compiler keeps the page.
  const now = useFrozenNow(returnTick);
  useEffect(() => {
    if (!returnTick) return;
    // Window-keyed reads move with `now`; the directory, the rows' build records and the org levels don't.
    void qc.invalidateQueries({ queryKey: productKey(product, AGENTS_FEATURE), exact: true });
    void qc.invalidateQueries({ queryKey: productKey(product, "builds", "agent") });
    // An edition's org levels too (units and members change elsewhere: Access control, the IdP sync).
    void qc.invalidateQueries({ queryKey: productKey(product, "home", "org") });
    // The Budget burn panel's burn and At risk (Needs you's budget rows too).
    void qc.invalidateQueries({ queryKey: productKey(product, "budgets", "v1") });
  }, [returnTick, qc, product]);

  // Scope: the URL, else the stored choice, else the persona's default once it settles (scope.ts).
  const persona = usePersona();
  const [interacted, setInteracted] = useState(false);
  const resolved = resolveScope({
    url: search.scope,
    stored: storedScope(me.sub),
    persona: persona.settled ? homeScopeFor(persona.persona, me.is_superuser) : null,
    interacted,
  });
  const replaced = useRef(false);
  const pathname = usePathname();
  useEffect(() => {
    // Only while still on the home: the persona can answer during a navigation away (the old route stays mounted until
    // the next one loads), and a write then would pull the user back to `/` (review: onboarding's Open Sessions).
    if (resolved.replace && !replaced.current && pathname === "/") {
      replaced.current = true;
      setSearch({ scope: resolved.replace }, { replace: true });
    }
  }, [resolved.replace, setSearch, pathname]);
  const scope = resolved.scope;
  const change = (patch: Partial<HomeSearch>) => {
    setInteracted(true);
    if (patch.scope) rememberScope(me.sub, patch.scope);
    setSearch(patch);
  };

  const home = useHome(now, range, me, scope);
  const { needsYou } = home;
  const pausedFlows = usePausedFlows(needsYou.needs.rows.some((r) => r.kind === "request"));
  const [sort, setSort] = useState<Sort>(DEFAULT_SORT);
  const phone = useMediaQuery(PHONE_QUERY, false);
  const rows = home.rows ? sortRows(home.rows, sort) : null;
  const t = rows ? totals(rows) : null;
  const unit = isUnitScope(scope);
  const sentence = t
    ? homeNarrative({
        scope: unit ? "unit" : (scope ?? "fleet"),
        unitName: home.org.level?.label,
        ...t,
        floor: home.unpriced,
        days,
      })
    : null;

  // Budget burn (plans/feat-budget-burn.md F10, D13): budgets are Enterprise only, so the panel is the layer's. It
  // still reports `pending`, because the band holds its row and withholds Over time until it answers (no swap
  // mid-look), and an OSS build answers "no panel, not pending" without a request.
  const { node: burnPanel, pending: burnPending } = useSlots().budgets.useHomePanel({
    scope,
    meSub: me.sub,
    isSuperuser: me.is_superuser,
    unitLabel: home.org.level?.label ?? null,
  });

  // The health bar narrows the table only (#62): the strip and the charts keep the whole scope.
  const health = search.health;
  // Every row reads Not rated until the ratings answer, so the filter waits for them: a shared `?health=watch` link
  // never opens on "No rows are Watch".
  const healthPending = !!health && home.ratingsPending;
  const shown = rows ? filterByHealth(rows, healthPending ? undefined : health) : null;
  const healthName = (h: NonNullable<HomeSearch["health"]>) =>
    h === "unknown" ? copy.charts.notRated : overviewCopy.rating[h];

  const rowsProps = {
    rows: shown ?? [],
    // Fleet and an org level group by owner; Yours is one owner.
    grouped: scope !== "yours",
    days,
    range,
    now,
    win: home.win,
    series: me.is_superuser,
    ratingsPending: home.ratingsPending,
    savedStatus: home.savedStatus,
    budgets: home.budgets,
    ownerName: home.owner,
    unpriced: home.unpriced,
  };

  // First run: nothing deployed or registered yet. Its Setup guide (or deploy) card is on the page already.
  // A unit link shows the unit's own states even on an empty fleet (an OSS build, or a first run): never the first run.
  const firstRun = !!home.dir.data && !home.dir.data.length && !unit;
  const actions = (
    <>
      <ToggleGroup
        variant="outline"
        size="sm"
        value={scope === "yours" || scope === "fleet" ? [scope] : []}
        onValueChange={(vs) => {
          const v = vs[0] as Scope | undefined;
          if (v) change({ scope: v });
        }}
        aria-label={copy.scope.label}
      >
        <ToggleGroupItem
          value="yours"
          title={copy.scope.yoursHint}
          className={`px-3 text-xs ${TOUCH}`}
        >
          {copy.scope.yours}
        </ToggleGroupItem>
        <ToggleGroupItem
          value="fleet"
          title={copy.scope.fleetHint}
          className={`px-3 text-xs ${TOUCH}`}
        >
          {copy.scope.fleet}
        </ToggleGroupItem>
      </ToggleGroup>
      {/* The org levels an edition adds (EE: units, PR2c). A member, or OSS, has none and sees no picker. */}
      {home.org.levels.length ? (
        // A unit the viewer can't pick shows the placeholder, never a blank trigger.
        <Select
          value={home.org.level ? (scope ?? null) : null}
          onValueChange={(v: string | null) => {
            if (v) change({ scope: v as Scope });
          }}
          // The trigger shows the unit's name, not its `unit:<id>` value (Base UI reads labels from `items`).
          items={Object.fromEntries(home.org.levels.map((l) => [`unit:${l.id}`, l.label]))}
        >
          <SelectTrigger
            size="sm"
            className={`h-8 w-fit max-w-[min(16rem,100%)] text-xs ${TOUCH}`}
            aria-label={copy.scope.unitLabel}
          >
            <SelectValue placeholder={copy.scope.unitPlaceholder} />
          </SelectTrigger>
          <SelectContent>
            {home.org.levels.map((l) => (
              // The indent sits on the item, outside its text (which the trigger repeats); the path names it, so
              // same-named units under different parents differ for a screen reader.
              <SelectItem
                key={l.id}
                value={`unit:${l.id}`}
                aria-label={l.path}
                label={l.label}
                className={TOUCH}
                style={{ paddingInlineStart: `${0.5 + l.depth}rem` }}
              >
                {l.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : null}
      <ToggleGroup
        variant="outline"
        size="sm"
        value={[range]}
        onValueChange={(vs) => {
          const v = vs[0] as HomeSearch["range"];
          if (v) change({ range: v === DEFAULT_RANGE ? undefined : v });
        }}
        aria-label={overviewCopy.range.label}
      >
        {RANGES.map((r) => (
          <ToggleGroupItem
            key={r}
            value={r}
            aria-label={overviewCopy.range.item(RANGE_DAYS[r])}
            className={`px-3 text-xs ${TOUCH}`}
          >
            {r}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      {/* On first run the page's own card holds the one next action (design 7A). */}
      {firstRun ? null : <DeployAgentButton variant="outline" />}
      {firstRun ? null : <SetupGuide />}
    </>
  );

  // The Overview's first-run layout, unchanged.
  if (firstRun)
    return (
      <Frame actions={actions}>
        <FirstRunLead />
        <div className="grid grid-cols-1 gap-3">
          <FirstRun />
          <FirstRunPreview className="mt-3" />
        </div>
      </Frame>
    );

  return (
    <Frame description={sentence ?? undefined} actions={actions}>
      {/* An empty fleet (only reachable here through a unit link) reads no sessions, so Needs you would never settle. */}
      {home.dir.data?.length === 0 ? null : (
        <NeedsYou data={needsYou} now={now.getTime()} userId={me.sub} pausedFlows={pausedFlows} />
      )}
      {rows && t ? (
        <Strip
          totals={t}
          member={!me.is_superuser}
          scope={scope}
          asOf={home.asOf}
          unpriced={home.unpriced}
          savedStatus={home.savedStatus}
          savedCapped={home.savedCapped}
          budgets={home.budgets}
        />
      ) : null}
      {scope === "fleet" && !me.is_superuser ? (
        <p className="m-0 text-xs text-muted-foreground">{copy.scope.sharedWithYou}</p>
      ) : null}
      {unit && home.org.level ? (
        <p className="m-0 text-xs text-muted-foreground">
          {copy.scope.unitCaption(home.org.level.path, !me.is_superuser)}
        </p>
      ) : null}
      {unit && home.org.state === "stale" ? (
        <p role="status" className="m-0 flex items-center gap-2 text-sm text-destructive">
          {copy.unitStale}
          <Button size="sm" variant="ghost" className={TOUCH} onClick={home.org.retry}>
            {copy.retry}
          </Button>
        </p>
      ) : null}
      {home.spendStatus === "failed" ||
      home.spendStatus === "stale" ||
      home.savedStatus === "failed" ||
      home.savedStatus === "stale" ? (
        <p role="status" className="m-0 flex items-center gap-2 text-sm text-destructive">
          {home.spendStatus === "failed" || home.savedStatus === "failed"
            ? copy.moneyFailed
            : copy.moneyStale}
          <Button size="sm" variant="ghost" className={TOUCH} onClick={home.retryMoney}>
            {copy.retry}
          </Button>
        </p>
      ) : null}
      {home.dirFailed ? (
        // A directory outage costs the table, never Needs you above it (the Overview kept its cards apart too).
        <StateCard
          tone="error"
          title={copy.error}
          action={
            <Button
              size="sm"
              variant="outline"
              className={TOUCH}
              onClick={() => void home.dir.refetch()}
            >
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
      ) : unit && home.org.state === "unknown" ? (
        // A unit link the viewer can't pick (deleted, outside a manager's subtrees, or a build without org levels).
        <EmptyState title={copy.empty.unknownUnit}>
          <Button
            size="sm"
            variant="outline"
            className={`mt-3 ${TOUCH}`}
            onClick={() => {
              change({ scope: "fleet" });
            }}
          >
            {copy.empty.showFleet}
          </Button>
        </EmptyState>
      ) : unit && home.org.state === "failed" ? (
        <StateCard
          tone="error"
          title={copy.unitFailed}
          action={
            <Button size="sm" variant="outline" className={TOUCH} onClick={home.org.retry}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
      ) : !rows ? (
        <PanelSkeleton height={SKELETON_ROWS * SKELETON_ROW_PX} />
      ) : !rows.length && unit ? (
        <EmptyState title={copy.empty.noUnitRows}>
          <p className="m-0">{copy.empty.noUnitRowsBody}</p>
        </EmptyState>
      ) : !rows.length ? (
        <EmptyState title={copy.empty.noOwned}>
          <p className="m-0">{copy.empty.noOwnedBody}</p>
          <Button
            size="sm"
            variant="outline"
            className={`mt-3 ${TOUCH}`}
            onClick={() => {
              change({ scope: "fleet" });
            }}
          >
            {copy.empty.showFleet}
          </Button>
        </EmptyState>
      ) : (
        <>
          {t && t.spend === 0 && !home.unpriced && !rows.some((r) => (r.requests ?? 0) > 0) ? (
            <p className="m-0 text-sm text-muted-foreground">{copy.empty.noUsage(days)}</p>
          ) : null}
          <Suspense fallback={<PanelSkeleton height={BAND_SKELETON_PX} />}>
            <ChartBand
              rows={rows}
              superuser={me.is_superuser}
              fleet={scope === "fleet"}
              days={days}
              win={home.win}
              perf={home.perf}
              spendStatus={home.spendStatus}
              dashError={home.dashError}
              savedStatus={home.savedStatus}
              savedCapped={home.savedCapped}
              unpriced={home.unpriced}
              retryMoney={home.retryMoney}
              ratingsPending={home.ratingsPending}
              health={health}
              onHealth={(h) => {
                change({ health: h });
              }}
              burn={burnPanel}
              burnPending={burnPending}
            />
          </Suspense>
          {health ? (
            <div className="flex flex-wrap items-center gap-2 text-sm">
              {/* Only the sentence is live: Clear isn't re-announced with each change. */}
              <p role="status" className="m-0">
                {healthPending
                  ? copy.charts.filterPending(healthName(health))
                  : shown?.length
                    ? copy.charts.filtered(healthName(health), shown.length, rows.length)
                    : copy.charts.noneMatch(healthName(health))}
              </p>
              <Button
                size="sm"
                variant="ghost"
                className={TOUCH}
                onClick={() => {
                  change({ health: undefined });
                }}
              >
                {copy.charts.clear}
              </Button>
            </div>
          ) : null}
          {!shown?.length ? null : phone ? (
            <PhoneRows {...rowsProps} />
          ) : (
            <RowsTable {...rowsProps} sort={sort} onSort={setSort} />
          )}
        </>
      )}
    </Frame>
  );
}

function Strip({
  totals: t,
  member,
  scope,
  asOf,
  unpriced,
  savedStatus,
  savedCapped,
  budgets,
}: {
  totals: ReturnType<typeof totals>;
  member: boolean;
  scope: Scope | null;
  asOf: number;
  unpriced: boolean;
  savedStatus: SourceStatus;
  savedCapped: boolean;
  budgets: HomeBudgets;
}) {
  const savedKnown = savedStatus === "ok" || savedStatus === "stale";
  // Yours shows the viewer's own monthly budget; Fleet has no authoritative fleet budget to show, and owner and agent
  // budgets overlap, so they are never summed into one (design "Home scope semantics").
  const own = scope === "yours" && budgetsShown(budgets.state) ? budgets.own : null;
  const stamp = useBudgetStamp(budgets.updatedAt);
  return (
    <section aria-label={copy.strip.label} className="flex flex-col gap-1">
      <div
        className={`grid grid-cols-1 gap-px overflow-hidden rounded-lg border border-border bg-border *:rounded-none *:border-0 ${own ? "@[560px]/overview:grid-cols-2 @[880px]/overview:grid-cols-4" : "@[560px]/overview:grid-cols-3"}`}
      >
        <KpiTile
          label={copy.strip.spend}
          aside={
            member && scope !== "yours" ? (
              <span className="text-xs text-muted-foreground">{copy.strip.spendVisible}</span>
            ) : null
          }
          value={t.spend === null ? "—" : fmtMoneyFloor(t.spend, unpriced)}
        >
          {unpriced ? (
            <span className="text-xs text-muted-foreground">{copy.strip.unpriced}</span>
          ) : null}
          {t.spendHasEstimates ? (
            <span className="text-xs text-muted-foreground">{copy.strip.estimates}</span>
          ) : null}
        </KpiTile>
        <KpiTile
          label={copy.strip.saved}
          title={copy.strip.savedTooltip}
          value={
            !savedKnown || t.saved === null ? (
              "—"
            ) : (
              <>
                {savedCapped ? "≥ " : ""}
                {fmtTokens(t.saved)}{" "}
                <span className="text-sm font-normal text-muted-foreground">
                  {copy.strip.tokens}
                </span>
              </>
            )
          }
        >
          {savedKnown && t.saved === null ? (
            <span className="text-xs text-muted-foreground">{copy.strip.notReported}</span>
          ) : null}
          {savedKnown && t.savedBasis ? (
            <span className="text-xs text-muted-foreground">{copy.strip.basis(t.savedBasis)}</span>
          ) : null}
        </KpiTile>
        {own ? (
          <KpiTile
            label={copy.budget.own}
            aside={
              budgets.preview ? (
                <Badge variant="outline" className="text-3xs">
                  {copy.budget.preview}
                </Badge>
              ) : null
            }
            // Floored, so 99.6% never reads as a full 100% while under the limit; a floor (≥) with unpriced calls.
            value={
              own.limit > 0
                ? copy.budget.pct(Math.floor((own.used / own.limit) * 100), own.unpriced)
                : "—"
            }
          >
            <span className="text-xs text-muted-foreground">
              {copy.budget.use(
                fmtMoneyFloor(own.used, own.unpriced),
                fmtMoney(own.limit),
                budgetMonth(own.resetsAt),
              )}
            </span>
            <span className="text-xs text-muted-foreground">{copy.budget.ownHint}</span>
          </KpiTile>
        ) : null}
        <KpiTile label={copy.strip.needs} value={t.needing === null ? "—" : String(t.needing)} />
      </div>
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 text-xs text-muted-foreground">
        {/* One note for both layouts: the table header's badge and stamp aren't announced, and phones have no header. */}
        <p
          className="m-0"
          role={budgets.state === "failed" || budgets.state === "stale" ? "status" : undefined}
        >
          {budgets.state === "off"
            ? copy.budget.off
            : budgets.state === "failed"
              ? copy.budget.failed
              : budgets.state === "stale"
                ? `${copy.budget.stale} · ${(budgets.preview ? copy.budget.note : copy.budget.liveNote)(stamp)}`
                : (budgets.preview ? copy.budget.note : copy.budget.liveNote)(stamp)}
        </p>
        {asOf ? <p className="m-0">{copy.strip.asOf(fmtLocalTime(asOf))}</p> : null}
      </div>
    </section>
  );
}
