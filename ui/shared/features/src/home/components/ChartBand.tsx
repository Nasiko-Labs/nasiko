// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/components/ChartBand.tsx. Local edits:
// links are the navigation port's `Link` (`href`) and picking a Cost × p95 row is `useRouter().push('/agents/<id>')`;
// `ToggleGroup type="single"` is Base UI's value array (`value={[v]}`, an emptied pick ignored or cleared as the
// lab's Radix value did); the series reads take the product scope (`seriesQuery(scope, …)`); optional props admit
// `undefined` (exactOptionalPropertyTypes); Prettier formatting.
/**
 * The home's chart band (plans/feat-agent-centric.md PR2d, decisions #59-63): spend over time (superusers) or by agent,
 * TokenOps' Cost vs performance panel, saved tokens by agent, and the health bar that filters the table below.
 *
 * Its own lazy chunk (#63): the chart kit loads only when the band draws. The kit's marks are aria-hidden, so each chart
 * carries a text summary, and By agent is the list Over time's numbers can be read from.
 */
import { useQueries } from "@tanstack/react-query";
import Link, { useRouter } from "@nasiko/app-core/navigation";
import { useProductScope } from "@nasiko/data-access/product";
import { Suspense, useState, type ReactNode } from "react";
import { Bar as KitBar } from "@nasiko/ui/charts-kit";
import { BarChart } from "@nasiko/ui/charts-kit";
import { BarXAxis } from "@nasiko/ui/charts-kit";
import { Grid } from "@nasiko/ui/charts-kit";
import { ChartTooltip } from "@nasiko/ui/charts-kit";
import { YAxis } from "@nasiko/ui/charts-kit";
import { Swatch } from "@nasiko/ui/composites";
import { Panel, PanelEmpty, PanelError, PanelSkeleton } from "@nasiko/ui/composites";
import { Button } from "@nasiko/ui/primitives";
import { ToggleGroup, ToggleGroupItem } from "@nasiko/ui/primitives";
import { copy as overviewCopy } from "../../overview/copy";
import { CostPerformance } from "../../tokenops/components/CostPerformance";
import type { ResolvedWindow } from "../../tokenops/window";
import { OTHER_SERIES, seriesAt, type Series } from "@nasiko/ui/lib";
import { fmtMoney, fmtMoneyAxis, fmtMoneyFloor, fmtPct, fmtTokens } from "@nasiko/ui/lib";
import { useMediaQuery } from "@nasiko/ui/lib";
import { cn } from "@nasiko/ui/cn";
import { seriesQuery, type SourceStatus } from "../api";
import {
  costPerfRows,
  healthCounts,
  HEALTH_ORDER,
  stackDays,
  topBars,
  topSpenders,
  type AgentPerf,
  type Bars,
  type HealthKey,
  type StackDay,
} from "../charts";
import { copy } from "../copy";
import type { HomeRow } from "../rows";
import { BAND_BARS, BAND_STACK, PHONE_QUERY, SAVED_BARS, TOUCH } from "../tuning";

export interface ChartBandProps {
  /** The scope's rows, unfiltered: the charts cover the whole scope (#62). */
  rows: HomeRow[];
  /** Superusers get spend over time (`spend-timeseries` isn't ACL-scoped, #60). */
  superuser: boolean;
  /** The scope is the whole fleet: Over time draws the rest of it as Other. */
  fleet: boolean;
  days: number;
  win: Pick<ResolvedWindow, "start" | "end">;
  perf: ReadonlyMap<string, AgentPerf> | null;
  spendStatus: SourceStatus;
  dashError: unknown;
  savedStatus: SourceStatus;
  savedCapped: boolean;
  unpriced: boolean;
  retryMoney: () => void;
  ratingsPending: boolean;
  health: HealthKey | undefined;
  onHealth: (h: HealthKey | undefined) => void;
  /** The Budget burn panel (plans/feat-budget-burn.md F10, D13): the first row when its burn read answered; then Spend
   *  keeps By agent only (the panel's chart is the scope's spend over the month). */
  burn?: ReactNode | undefined;
  /** The scope's burn read is still out: its row is held (a skeleton) and Over time waits, so neither flashes. */
  burnPending?: boolean | undefined;
}

const SPEND = seriesAt(0);
const SAVED = seriesAt(1);
const CHART_H = 192;
const MARGIN = { top: 8, right: 8, bottom: 28, left: 44 };

export default function ChartBand(p: ChartBandProps) {
  const router = useRouter();
  return (
    <section
      aria-label={copy.charts.label}
      className="grid grid-cols-1 gap-3 @[880px]/overview:grid-cols-2"
    >
      {p.burn ? (
        <Suspense fallback={<BurnRowSkeleton />}>{p.burn}</Suspense>
      ) : p.burnPending ? (
        <BurnRowSkeleton />
      ) : null}
      <SpendPanel {...p} />
      <div className="[&>section]:h-full">
        <CostPerformance
          rows={costPerfRows(p.rows, p.perf)}
          loading={p.spendStatus === "loading"}
          error={p.spendStatus === "failed" ? p.dashError : null}
          onRetry={p.retryMoney}
          onSelect={(id) => {
            router.push(`/agents/${encodeURIComponent(id)}`);
          }}
          selectHint={copy.charts.openHub}
        />
      </div>
      <SavedPanel {...p} />
      <HealthPanel {...p} />
    </section>
  );
}

/** The burn panel's row while it loads: full width, like the panel. */
function BurnRowSkeleton() {
  return (
    <div className="@[880px]/overview:col-span-2">
      <PanelSkeleton height={CHART_H} />
    </div>
  );
}

// ─── Spend ─────────────────────────────────────────────

function SpendPanel(p: ChartBandProps) {
  const [view, setView] = useState<"time" | "agent">("time");
  // With the Budget burn panel above, Over time is that panel's job (D13).
  const overTime = p.superuser && !p.burn && !p.burnPending;
  const time = overTime && view === "time";
  const bars = topBars(p.rows, "spend", BAND_BARS);
  const known = p.spendStatus === "ok" || p.spendStatus === "stale";
  const top = topSpenders(p.rows, BAND_STACK);
  const withSpend = p.rows.filter((r) => (r.spend ?? 0) > 0).length;
  return (
    <Panel
      title={copy.charts.spend(p.days)}
      labelledBy="home-spend-title"
      subtitle={
        !known || !bars.bars.length
          ? undefined
          : time
            ? copy.charts.trendSubtitle(top.length, withSpend, p.fleet)
            : copy.charts.byAgentSubtitle
      }
      actions={
        overTime ? (
          <ToggleGroup
            variant="outline"
            size="sm"
            value={[view]}
            onValueChange={(vs) => {
              const v = vs[0] as "time" | "agent" | undefined;
              if (v) setView(v);
            }}
            aria-label={copy.charts.view}
          >
            <ToggleGroupItem value="time" className={`px-3 text-xs ${TOUCH}`}>
              {copy.charts.overTime}
            </ToggleGroupItem>
            <ToggleGroupItem value="agent" className={`px-3 text-xs ${TOUCH}`}>
              {copy.charts.byAgent}
            </ToggleGroupItem>
          </ToggleGroup>
        ) : undefined
      }
    >
      {p.spendStatus === "loading" ? (
        <PanelSkeleton height={CHART_H} />
      ) : p.spendStatus === "failed" ? (
        <PanelError error={p.dashError} onRetry={p.retryMoney} what={copy.charts.trendFailed} />
      ) : !bars.bars.length ? (
        <PanelEmpty title={copy.charts.noSpend(p.days)} />
      ) : time ? (
        <>
          <SpendTrend top={top} fleet={p.fleet} win={p.win} days={p.days} />
          {top.some((r) => r.harness) ? (
            <p className="m-0 text-xs text-muted-foreground">{copy.charts.estimate}</p>
          ) : null}
        </>
      ) : (
        <BarList
          data={bars}
          series={SPEND}
          format={fmtMoney}
          // Like the table: a row's spend is its own figure, a sum of rows is a floor (≥) when calls had no price.
          formatOther={(v) => fmtMoneyFloor(v, p.unpriced)}
          note={p.rows.some((r) => r.harness && (r.spend ?? 0) > 0) ? copy.charts.estimate : null}
        />
      )}
    </Panel>
  );
}

/** The top spenders' series (the sparklines' own queries) and, on Fleet, the fleet's, stacked per day. */
function SpendTrend({
  top,
  fleet,
  win,
  days,
}: {
  top: HomeRow[];
  fleet: boolean;
  win: Pick<ResolvedWindow, "start" | "end">;
  days: number;
}) {
  const ids: (string | null)[] = [...top.map((r) => r.id), ...(fleet ? [null] : [])];
  const scope = useProductScope();
  const qs = useQueries({ queries: ids.map((id) => seriesQuery(scope, id, win)) });
  const phone = useMediaQuery(PHONE_QUERY, false);
  const failed = qs.find((q) => q.isError);
  if (failed)
    return (
      <PanelError
        error={failed.error}
        onRetry={() => {
          qs.forEach((q) => q.isError && void q.refetch());
        }}
        what={copy.charts.trendFailed}
      />
    );
  const series = qs.flatMap((q) => (q.data ? [q.data] : []));
  if (series.length < qs.length) return <PanelSkeleton height={CHART_H} />;
  const data = stackDays(
    series.slice(0, top.length),
    fleet ? (series[top.length] ?? null) : null,
    win,
  );
  const keys = [
    ...top.map((r, i) => ({ key: `s${i}`, label: r.displayName, series: seriesAt(i) })),
    ...(fleet ? [{ key: "other", label: copy.charts.other, series: OTHER_SERIES }] : []),
  ];
  return (
    <figure className="m-0 flex flex-col gap-2">
      <figcaption className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
        {keys.map((k) => (
          <span key={k.key} className="inline-flex min-w-0 items-center gap-1.5">
            <Swatch series={k.series} /> <span className="truncate">{k.label}</span>
          </span>
        ))}
        <span className="sr-only">{copy.charts.trendSummary(top.length, days, fleet)}</span>
      </figcaption>
      <div role="presentation" style={{ height: CHART_H }}>
        <BarChart
          data={data as unknown as Record<string, unknown>[]}
          xDataKey="label"
          stacked
          aspectRatio="auto"
          className="h-full"
          margin={MARGIN}
          barGap={0.3}
        >
          <Grid />
          <YAxis numTicks={3} formatValue={fmtMoneyAxis} />
          {keys.map((k) => (
            <KitBar
              key={k.key}
              dataKey={k.key}
              fill={k.series.fill}
              edge={k.series.edge}
              stroke={k.series.edge}
              lineCap="butt"
            />
          ))}
          <BarXAxis maxLabels={phone ? 4 : 7} />
          <ChartTooltip
            showDatePill={false}
            content={({ point }) => <TrendTip day={point as unknown as StackDay} keys={keys} />}
          />
        </BarChart>
      </div>
    </figure>
  );
}

function TrendTip({
  day,
  keys,
}: {
  day: StackDay;
  keys: { key: string; label: string; series: Series }[];
}) {
  return (
    <div className="px-3 py-2 text-xs text-popover-foreground">
      <div className="mb-1 font-medium">{day.label}</div>
      <div className="grid grid-cols-[auto_auto] gap-x-3 gap-y-0.5 tabular-nums">
        {keys.map((k) => (
          <span key={k.key} className="contents">
            <span className="flex items-center gap-1.5 text-muted-foreground">
              <Swatch series={k.series} /> {k.label}
            </span>
            <span className="text-right">
              {fmtMoney(Number(day[k.key as keyof StackDay]) || 0)}
            </span>
          </span>
        ))}
      </div>
    </div>
  );
}

// ─── By agent ─────────────────────────────────────────────

/** Ranked bars with their values, each name opening the agent's hub; the rest of the scope folds into an Other row. */
function BarList({
  data,
  series,
  format,
  formatOther = format,
  note,
}: {
  data: Bars;
  series: Series;
  format: (v: number) => string;
  formatOther?: ((v: number) => string) | undefined;
  note?: string | null | undefined;
}) {
  const width = (v: number) => `${data.max > 0 ? Math.max(1, (v / data.max) * 100) : 0}%`;
  const row = "grid grid-cols-[minmax(0,9rem)_minmax(0,1fr)_auto] items-center gap-3 text-xs";
  const bar = (v: number, s: Series) => (
    <span aria-hidden className="h-2.5 w-full">
      <span
        className="block h-full rounded-xs"
        style={{ width: width(v), background: s.fill, borderTop: `2px solid ${s.edge}` }}
      />
    </span>
  );
  return (
    <div className="flex flex-col gap-2">
      <ol className="m-0 flex list-none flex-col gap-1 p-0">
        {data.bars.map((b) => (
          <li key={b.id} className={row}>
            <Link
              href={`/agents/${encodeURIComponent(b.id)}`}
              className={`truncate underline-offset-4 hover:underline ${TOUCH} pointer-coarse:flex pointer-coarse:items-center`}
              title={b.name}
            >
              {b.name}
            </Link>
            {bar(b.value, series)}
            <span className="text-right tabular-nums">{format(b.value)}</span>
          </li>
        ))}
        {data.other ? (
          <li className={row}>
            <span className="text-muted-foreground">
              {copy.charts.otherCount(data.other.count)}
            </span>
            {/* No bar: the rest of the scope summed would dwarf the agents named above it. */}
            <span />
            <span className="text-right tabular-nums">{formatOther(data.other.value)}</span>
          </li>
        ) : null}
      </ol>
      {note ? <p className="m-0 text-xs text-muted-foreground">{note}</p> : null}
    </div>
  );
}

// ─── Saved ─────────────────────────────────────────────

function SavedPanel(p: ChartBandProps) {
  const bars = topBars(p.rows, "saved", SAVED_BARS);
  const bases = new Set(p.rows.flatMap((r) => (r.savedBasis ? [r.savedBasis] : [])));
  const basis = bases.size > 1 ? "mixed" : ([...bases][0] ?? null);
  return (
    <Panel
      title={copy.charts.saved}
      labelledBy="home-saved-title"
      subtitle={bars.bars.length ? copy.charts.savedSubtitle(basis) : undefined}
    >
      {p.savedStatus === "loading" ? (
        <PanelSkeleton height={120} />
      ) : p.savedStatus === "failed" ? (
        <p className="m-0 text-sm text-muted-foreground">{copy.charts.savedFailed}</p>
      ) : !bars.bars.length ? (
        <PanelEmpty title={copy.charts.noSaved} />
      ) : (
        <BarList
          data={bars}
          series={SAVED}
          format={fmtTokens}
          note={p.savedCapped ? copy.charts.savedCapped : null}
        />
      )}
    </Panel>
  );
}

// ─── Health ─────────────────────────────────────────────

/** Each rating's fill, hover included (a ghost button would hover to the accent). */
const HEALTH_FILL: Record<HealthKey, string> = {
  action: "bg-destructive hover:bg-destructive",
  watch: "bg-warning hover:bg-warning",
  healthy: "bg-success hover:bg-success",
  unknown: "bg-muted-foreground/30 hover:bg-muted-foreground/30",
};

const healthName = (k: HealthKey) =>
  k === "unknown" ? copy.charts.notRated : overviewCopy.rating[k];

function HealthPanel(p: ChartBandProps) {
  const counts = healthCounts(p.rows);
  const keys = HEALTH_ORDER.filter((k) => counts[k] > 0);
  const toggle = (k: HealthKey) => {
    p.onHealth(p.health === k ? undefined : k);
  };
  return (
    <Panel
      title={copy.charts.health}
      labelledBy="home-health-title"
      subtitle={p.ratingsPending ? undefined : copy.charts.healthSubtitle}
    >
      {p.ratingsPending ? (
        <p className="m-0 text-sm text-muted-foreground" role="status">
          {copy.charts.healthChecking}
        </p>
      ) : (
        <div className="flex flex-col gap-2">
          {/* The bar is a pointer shortcut; the toggles below are the keyboard and screen-reader path. */}
          <div aria-hidden className="flex h-3 w-full gap-px overflow-hidden rounded-full">
            {keys.map((k) => (
              <Button
                key={k}
                variant="ghost"
                size="xs"
                tabIndex={-1}
                data-testid={`home-health-${k}`}
                onClick={() => {
                  toggle(k);
                }}
                style={{ flexGrow: counts[k] }}
                className={cn(
                  "h-full min-w-1 rounded-none p-0",
                  HEALTH_FILL[k],
                  p.health && p.health !== k && "opacity-30",
                )}
              />
            ))}
          </div>
          <ToggleGroup
            size="sm"
            value={p.health ? [p.health] : []}
            onValueChange={(vs) => {
              p.onHealth(vs[0] as HealthKey | undefined);
            }}
            aria-label={copy.charts.health}
            className="w-full flex-col items-stretch"
          >
            {keys.map((k) => (
              <ToggleGroupItem
                key={k}
                value={k}
                aria-label={copy.charts.healthFilter(healthName(k), counts[k])}
                className={`w-full justify-start gap-2 px-2 text-xs ${TOUCH}`}
              >
                <span aria-hidden className={cn("size-2.5 rounded-full", HEALTH_FILL[k])} />
                {healthName(k)}
                <span className="ml-auto tabular-nums">{counts[k]}</span>
                <span className="w-12 text-right text-muted-foreground tabular-nums">
                  {fmtPct((counts[k] / p.rows.length) * 100)}
                </span>
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        </div>
      )}
    </Panel>
  );
}
