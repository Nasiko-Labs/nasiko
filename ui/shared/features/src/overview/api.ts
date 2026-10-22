// Ported from nasiko-ui-lab@1b99bdda common/src/features/overview/api.ts. Local edits: `useFleetHealth` and
// `FleetHealth` live in ./fleet.ts (ported earlier for the Agents catalog) and are re-exported, so their imports go;
// the results' `retry` are function properties (the cards pass them on unbound) and the harness fallback's `retry`
// wraps `live.refetch` in an arrow (unbound-method lint); development (be271b564, b3dd6d4f8, 1c28b0bb9): Needs you's
// budget rows come from the budgets slot (`useBudgetNeeds`; Enterprise only), the Budgets card and its router reads are
// gone, and the result carries `waiting` for the home's per-agent counts; Prettier formatting. Ported from
// nasiko-cloud-rs development (480650a69): the spend hook reads one dashboard (`compare=previous`) and one grouped
// timeseries instead of a request per driver, and the Overview's session reads are two small filtered pages
// (`useSessionPage`).
// Development (2d95e6e79): `Spend.tokens` (the dashboard's token KPI and split) and `useSavedByAgent`.
/**
 * The Overview's shared data (plans/feat-overview.md §5, eng review R1), now the home's (plans/feat-agent-centric.md
 * PR2a): `useFleetHealth()` is the one source of the fleet ratings, called by the home, the agent hub and the Agents
 * catalog while `?health=` is set, so all apply the same rules to the same directory rows; `useNeedsYou()` feeds Needs
 * you. Both reuse existing query keys, so TokenOps and the Agents pages open warm.
 */
import { isNotVisible, useUsage } from "../harnesses/api";
import { useLiveIndividual } from "../harnesses/liveIndividual";
import { mostlyUnpriced } from "../harnesses/rollup";
import { sessionRows, useChatSessions } from "../chat/api";
import { useWaiting } from "../chat/waiting";
import { useSessionPage } from "../sessions/api";
import type { Me } from "@nasiko/data-access/product";
import { useMemo } from "react";
import { useSlots } from "@nasiko/app-core/edition/edition-context";
import { findSpike, type Spike } from "../narrative/tokenops";
import { useCalendar, useDashboard, useSavings, useTimeseries } from "../tokenops/api";
import { buildAttribution, type AttributionRow } from "../tokenops/attribution";
import { summarizeMonth, type MonthSummary } from "../tokenops/forecast";
import { zeroFillTimeline, type TimelinePoint } from "../tokenops/series";
import type { AgentFinopsRow, FinopsAttributions } from "../tokenops/types";
import { summarizeOptimisation } from "../tokenops/optimisation";
import { monthKey, resolveWindow, utcMonthStart, type ResolvedWindow } from "../tokenops/window";
import { RANGE_DAYS, type Range } from "./search";
import { isEndpointAbsent } from "@nasiko/data-access/product";
import { copy } from "./copy";
import { mergeNeeds, type Needs, type NeedsInput, type SourceId } from "./needs";
import { stackSpend, type Stack } from "./stack";
import { FAILED_SESSION_ROWS, RECENT_SESSION_ROWS, SAVINGS_ROWS, TOP_DRIVERS } from "./tuning";

// useFleetHealth and FleetHealth moved to ./fleet (ported early for the Agents catalog's ?health= filter).
export { useFleetHealth, type FleetHealth } from "./fleet";
import type { FleetHealth } from "./fleet";

/**
 * The agent attributions of the preceding window, rebuilt from `compare=previous`
 * on the agent rows. `undefined` when the response didn't carry a comparison, so
 * `buildAttribution` reports deltas as unavailable rather than as a drop to zero.
 */
const previousAttributions = (rows: readonly AgentFinopsRow[] | undefined) =>
  rows?.some((r) => r.previous)
    ? ({
        view: "agent" as const,
        rows: rows.map((r) => ({
          agent_id: r.agent_id,
          agent_name: r.agent_name,
          total_cost: r.previous?.total_cost ?? 0,
          operations: r.previous?.operations ?? 0,
          total_tokens: r.previous?.total_tokens ?? 0,
          tool_call_count: r.previous?.tool_call_count ?? 0,
          avg_latency_ms: r.previous?.avg_latency_ms ?? null,
          avg_cost_per_operation: 0,
          is_capped: false,
          prompt_tokens: 0,
          completion_tokens: 0,
          cache_read_tokens: 0,
          cache_creation_tokens: 0,
          avg_latency_p95_ms: null,
          avg_latency_p99_ms: null,
          version: null,
          container_hours: 0,
        })),
      } satisfies FinopsAttributions)
    : undefined;

/** 7d and 30d are the server's own ranges; 90d has none, so it sends explicit UTC days ending today. */
export function rangeWindow(range: Range, now: Date): ResolvedWindow {
  if (range !== "90d") return resolveWindow({ preset: range }, now);
  const to = now.toISOString().slice(0, 10);
  const from = new Date(now.getTime() - 89 * 86_400_000).toISOString().slice(0, 10);
  return resolveWindow({ preset: "custom", from, to }, now);
}

/** The previous window of the same length, as a window of its own (its own query key). */
export function previousWindow(win: ResolvedWindow): ResolvedWindow {
  return {
    ...win,
    start: win.prevStart,
    end: win.prevEnd,
    params: win.prevParams,
    key: `${win.key}|previous`,
  };
}

export interface Spend {
  /** Month-to-date and forecast from the fleet-wide calendar (eng review R2): said as fleet spend. */
  summary: MonthSummary | null;
  isPending: boolean;
  error: unknown;
  /** The range's length in days (for per-day averages and "last N days"). */
  rangeDays: number;
  /** Fleet spend and runs (operations) over the range and the window before it, from the fleet-wide timeseries;
   *  `previous` is null while it loads or when it failed. */
  totals: {
    spend: number;
    runs: number;
    previous: { spend: number; runs: number } | null;
  } | null;
  /** The range's timeseries failed: no totals, no chart. */
  totalsError: unknown;
  /** Tokens over the range and their change, from the dashboard's KPIs (ACL-scoped, as the drivers are), with the
   *  input / output split from its token usage; null while it loads or when it failed. */
  tokens: { total: number; changePct: number | null; input: number; output: number } | null;
  /** The range's UTC days, zero-filled (fleet-wide). */
  days: TimelinePoint[];
  /** Those days split by the top drivers plus Other; null while their series load, or when one failed (the chart
   *  then draws the fleet total alone). */
  stack: Stack | null;
  /** The drivers or their series are still loading: the chart waits rather than redraw as a stack. */
  stackPending: boolean;
  /** The 30-day peak when it is a spike (TokenOps' rule); may fall before the strip. */
  spike: Spike | null;
  /** The top cost drivers over the range, ACL-scoped (only agents you can access). */
  drivers: AttributionRow[];
  /** Every other accessible agent, summed; null when there are none. `deltaPct` is null without a previous cost. */
  other: {
    count: number;
    cost: number;
    sharePct: number;
    deltaPct: number | null;
    unavailable: boolean;
  } | null;
  driversError: unknown;
  /** Some calls in the range had no price: totals are a floor. */
  unpriced: boolean;
  retry: () => void;
}

export function useSpend(now: Date, range: Range, enabled = true): Spend {
  const thisMonth = utcMonthStart(now);
  const lastMonth = utcMonthStart(now, -1);
  const win = useMemo(() => rangeWindow(range, now), [range, now]);
  const calThis = useCalendar(monthKey(thisMonth), {}, enabled);
  const calLast = useCalendar(monthKey(lastMonth), {}, enabled);
  // Three requests where there were ten: the dashboard carries its own previous
  // window, and one timeseries carries the fleet line, the previous window and a
  // series per top driver. The drivers used to be one request each.
  const cur = useDashboard(win, {}, "agent", "current", enabled, true);
  const ts = useTimeseries(win, {}, enabled, {
    groupByAgent: true,
    top: TOP_DRIVERS,
    compare: true,
  });
  // Just after a UTC month rollover the kept-previous calendar is last month's: loading, never summarised as this month.
  const stale = calThis.isPlaceholderData || calLast.isPlaceholderData;
  const summary = useMemo(
    () =>
      calThis.data && calLast.data && !stale
        ? summarizeMonth(calThis.data.days, calLast.data.days, now)
        : null,
    [calThis.data, calLast.data, stale, now],
  );
  const days = useMemo(
    () =>
      ts.data?.bucket === "day" ? zeroFillTimeline(ts.data.points, "day", win.start, win.end) : [],
    [ts.data, win],
  );
  const totals = useMemo(() => {
    if (ts.data?.bucket !== "day" || ts.isPlaceholderData) return null;
    const sum = (pts: readonly { spend_usd: number; operations: number }[]) => ({
      spend: pts.reduce((n, p) => n + p.spend_usd, 0),
      runs: pts.reduce((n, p) => n + p.operations, 0),
    });
    // Same response as the current window now (`compare=previous`).
    const previous = ts.data.previous ? sum(ts.data.previous) : null;
    return { ...sum(ts.data.points), previous };
  }, [ts.data, ts.isPlaceholderData]);
  const { drivers, other } = useMemo(() => {
    if (!cur.data) return { drivers: [], other: null };
    const prevRows = previousAttributions(cur.data.agents);
    const all = buildAttribution(cur.data.attributions, prevRows).sort((a, b) => b.cost - a.cost);
    const top = all.slice(0, TOP_DRIVERS);
    const rest = all.slice(TOP_DRIVERS);
    if (!rest.length) return { drivers: top, other: null };
    const topIds = new Set(top.map((r) => r.id));
    const cost = rest.reduce((n, r) => n + r.cost, 0);
    // The previous window's cost of everyone outside the top: an agent new to the top was "other" before.
    const before =
      prevRows?.view === "agent"
        ? prevRows.rows.reduce((n, r) => n + (topIds.has(r.agent_id) ? 0 : r.total_cost), 0)
        : null;
    return {
      drivers: top,
      other: {
        count: rest.length,
        cost,
        sharePct: rest.reduce((n, r) => n + r.sharePct, 0),
        deltaPct: before ? ((cost - before) / before) * 100 : null,
        unavailable: before === null,
      },
    };
  }, [cur.data]);
  // Each driver's own days, from the `group_by=agent` series on the one
  // timeseries response. This was `useQueries` over one filtered request per
  // driver — a fan-out whose size grew with the chart.
  const withSpend = useMemo(() => drivers.filter((d) => d.cost > 0), [drivers]);
  const stack = useMemo(() => {
    const series = ts.data?.series;
    if (!withSpend.length || !days.length || !series || ts.data?.bucket !== "day") return null;
    // Keyed by agent id where the server resolved one, else by the raw name the
    // series carries — the same name `top_agent_name` uses.
    const byAgent = new Map(
      series.filter((s) => !s.is_other).map((s) => [s.agent_id ?? s.agent_name ?? "", s]),
    );
    return stackSpend(
      days,
      withSpend.map((d) => ({
        id: d.id,
        name: d.name,
        days: zeroFillTimeline(
          byAgent.get(d.id)?.points ?? byAgent.get(d.name)?.points ?? [],
          "day",
          win.start,
          win.end,
        ),
      })),
    );
  }, [withSpend, days, ts.data, win]);
  return {
    summary,
    isPending: calThis.isPending || calLast.isPending || stale,
    error: calThis.error ?? calLast.error,
    rangeDays: RANGE_DAYS[range],
    totals,
    totalsError: ts.error,
    tokens: cur.data
      ? {
          total: cur.data.kpis.total_tokens.current,
          changePct: cur.data.kpis.total_tokens.change_pct,
          input: cur.data.token_usage.prompt_tokens,
          output: cur.data.token_usage.completion_tokens,
        }
      : null,
    days,
    stack,
    stackPending: cur.isPending || ts.isPending,
    spike: findSpike(days.map((p) => ({ date: p.iso.slice(0, 10), spend: p.spend }))),
    drivers,
    other,
    driversError: cur.error,
    unpriced: (cur.data?.summary.unpriced_calls ?? 0) > 0,
    retry: () => {
      void calThis.refetch();
      void calLast.refetch();
      void cur.refetch();
      void ts.refetch();
    },
  };
}

export interface NeedsYou {
  needs: Needs;
  /** The oldest answer among the sources that answered (design review 8A's "Checked …"), or 0. */
  lastChecked: number;
  retry: Record<SourceId, () => void>;
  /** The newest sessions of the last 7 days and their checked status (eng review R7); the home counts failures per agent. */
  sessions: ReturnType<typeof useRecentSessions>;
  /** The pending-requests match this card used, so the home counts requests per agent without a second matcher. */
  waiting: ReturnType<typeof useWaiting>;
}

/**
 * The last 7 days' sessions, in two small reads under the Overview's own keys.
 *
 * This used to share the Sessions page's 100-row page and then fan out to
 * `session/{id}` for the newest 25 and `trace/{id}` for 3 traces each — up to
 * 100 browser requests, every one of them a trace-store read on the server, to
 * compute one boolean per row. `session/list` now carries `status` and counts
 * the matches in SQL, so the whole fan-out is two requests:
 *
 * - `recent` — the 5 rows the card shows.
 * - `failed` — every failed session in the window. `matching_count` is the real
 *   total, so Needs you states a fact instead of "N of the M latest checked".
 */
function useRecentSessions(now: Date, enabled: boolean) {
  const win = useMemo(() => resolveWindow({ preset: "7d" }, now), [now]);
  const recent = useSessionPage(
    win.key,
    win.start,
    win.end,
    { limit: RECENT_SESSION_ROWS },
    enabled,
  );
  const failed = useSessionPage(
    win.key,
    win.start,
    win.end,
    { limit: FAILED_SESSION_ROWS, status: "failed" },
    enabled,
  );
  return { recent, failed };
}

export function useNeedsYou(now: Date, me: Me, fleet: FleetHealth, enabled = true): NeedsYou {
  const waiting = useWaiting(me.sub, me.is_superuser);
  const chats = useChatSessions();
  // The fleet's directory, not a second observer: one enabled late would refetch a failed list and flicker it pending.
  const dir = { byId: fleet.byId };
  // Budgets (plans/feat-budget-burn.md F8): the viewer's scope's At risk, or R-L10's own list on an older server.
  const shownAsAgent = (agentId: string) =>
    // The agent row shows only its first reason: a stop listed lower down isn't said there.
    fleet.summary?.byId.get(agentId)?.reasons[0]?.code === "budgetStopped";
  const agentLabel = (agentId: string) => {
    const a = dir.byId.get(agentId);
    return a?.display_name || a?.name;
  };
  // Budgets are Enterprise only: an OSS build's source is absent, so Needs you simply never lists a budget row.
  const budgetNeeds = useSlots().budgets.useBudgetNeeds(me, enabled, agentLabel, shownAsAgent);
  const sessions = useRecentSessions(now, enabled);
  // Stable inputs for the memo below: query result objects are new on every render.
  const { match, failedCold, isPending: waitingPending } = waiting;

  const input: NeedsInput = useMemo(() => {
    const titles = new Map(sessionRows(chats.data).map((r) => [r.session_id, r.title]));
    const requests: NeedsInput["requests"] = failedCold
      ? { state: "failed" }
      : waitingPending
        ? { state: "loading" }
        : {
            state: "ok",
            value: {
              chats: match.chats.flatMap((c) => {
                const first = c.requests[0];
                if (!first) return [];
                return {
                  sessionId: c.sessionId,
                  firstId: first.id,
                  kind: first.kind,
                  count: c.requests.length,
                  chatTitle: titles.get(c.sessionId) || copy.needs.untitledChat,
                  at: first.created_at,
                };
              }),
              outside: match.outside,
            },
          };
    // Answered only once every rating input settled, or a spike or stopped budget could land after "Nothing needs
    // you" (design review 5A; /ship review).
    // Failed only with no data: a failed background refresh keeps the cached ratings, as Fleet health does.
    const agents: NeedsInput["agents"] =
      fleet.error && !fleet.summary
        ? { state: "failed" }
        : !fleet.summary || fleet.ratingsPending
          ? { state: "loading" }
          : {
              state: "ok",
              value: fleet.summary.action.map((a) => ({
                id: a.id,
                name: a.name,
                reason: a.reasons[0]?.text ?? "",
                budget: a.reasons[0]?.code === "budgetStopped",
              })),
              // Rows are real, but with a cost input down the source can't vouch that nothing else needs action.
              incomplete: fleet.costFailed,
            };
    const budgetRows = budgetNeeds.value;
    // One indexed read, filtered in SQL: `matching_count` is every failed session
    // in the window, so this states a fact. It used to be a sample — the newest
    // 25 rows, each status-checked by downloading its traces — which could only
    // ever say "N of the M latest sessions checked failed", and counted as a
    // failed source when too few of them came back (design 5A, QA ISSUE-005).
    const sessionState: NeedsInput["sessions"] = sessions.failed.isError
      ? { state: "failed" }
      : sessions.failed.isPending
        ? { state: "loading" }
        : {
            state: "ok",
            value: {
              failed: sessions.failed.matchingCount,
              agents: [
                ...new Set(
                  sessions.failed.rows.map((r) =>
                    r.agent_id ? fleet.displayName(r.agent_id) : copy.needs.unknownAgent,
                  ),
                ),
              ],
            },
          };
    return { requests, agents, budgets: budgetRows, sessions: sessionState };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `fleet` is a new object every render, so depending on it would re-run this on every render; every field read from it is listed individually below.
  }, [
    match,
    failedCold,
    waitingPending,
    chats.data,
    fleet.summary,
    fleet.error,
    fleet.ratingsPending,
    fleet.costFailed,
    fleet.displayName,
    budgetNeeds.value,
    sessions.failed.rows,
    sessions.failed.matchingCount,
    sessions.failed.isError,
    sessions.failed.isPending,
  ]);

  const needs = useMemo(() => mergeNeeds(input), [input]);
  const stamps = [waiting.lastChecked, budgetNeeds.stamp, sessions.failed.dataUpdatedAt].filter(
    (t) => t > 0,
  );
  return {
    needs,
    waiting,
    lastChecked: stamps.length ? Math.min(...stamps) : 0,
    retry: {
      requests: waiting.retry,
      // With a cost input down the agents source is incomplete: its Retry must refetch those too.
      agents: () => {
        fleet.retry();
        if (fleet.costFailed) fleet.retryCost();
      },
      budgets: budgetNeeds.retry,
      sessions: () => {
        void sessions.failed.refetch();
        void sessions.recent.refetch();
      },
    },
    sessions,
  };
}

interface HarnessLine {
  id: string;
  cost: number | null;
  /** More than half the turns unpriced: the cost reads "unpriced" (the Harnesses page rule). */
  unpriced: boolean;
}

export interface HarnessSummary {
  isPending: boolean;
  error: unknown;
  /** The org endpoint is absent (a bare 404): these are the viewer's own numbers (plans/feat-harness-org-view.md §4). */
  ownOnly: boolean;
  /** A coded 404: nothing visible at the landing scope. */
  notVisible: boolean;
  connected: number;
  activeDevs: number | null;
  harnesses: HarnessLine[];
  retry: () => void;
}

/** Coding harnesses over the range: the landing request (no scope, the server picks), else the viewer's own usage. */
export function useHarnessSummary(now: Date, me: Me, range: Range): HarnessSummary {
  const win = useMemo(() => rangeWindow(range, now), [range, now]);
  const usage = useUsage(me.sub, { ...win.params }, win.key, true);
  const absent = isEndpointAbsent(usage.error);
  const live = useLiveIndividual(me.sub, win, false, absent);
  if (absent) {
    const h = live.data?.harnesses.filter((x) => x.registered) ?? [];
    return {
      isPending: live.loading || (!live.data && !live.error),
      error: live.error,
      ownOnly: true,
      notVisible: false,
      connected: h.length,
      activeDevs: null,
      harnesses: h
        .map((x) => ({ id: x.harness, cost: x.cost_usd, unpriced: false }))
        // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- the proposed usage endpoint sends a null cost for unpriced turns; the type predates that (harness contract)
        .sort((a, b) => (b.cost ?? 0) - (a.cost ?? 0)),
      retry: () => {
        live.refetch();
      },
    };
  }
  // The landing request has no group_by=user, so its first page holds the whole answer (no cursor).
  const d = usage.data?.pages[0];
  const by = (d?.by_harness ?? []).filter((x) => x.registered_devs > 0 || x.turns > 0);
  return {
    isPending: usage.isPending,
    error: isNotVisible(usage.error) ? null : usage.error,
    ownOnly: false,
    notVisible: isNotVisible(usage.error),
    connected: by.length,
    activeDevs: d?.totals.active_devs ?? null,
    harnesses: by
      .map((x) => ({ id: x.harness, cost: x.cost_usd, unpriced: mostlyUnpriced(x) }))
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- the proposed usage endpoint sends a null cost for unpriced turns; the type predates that (harness contract)
      .sort((a, b) => (b.cost ?? 0) - (a.cost ?? 0)),
    retry: () => void usage.refetch(),
  };
}

/**
 * Token-optimisation savings for the header's range.
 *
 * Reuses TokenOps' own query and window so the two pages can never disagree about a figure, and so
 * the 90-day range works — the endpoint's `range` shorthand only understands 24h/7d/30d, while a
 * resolved window carries explicit timestamps.
 */
export function useOverviewSavings(now: Date, range: Range, enabled = true) {
  const win = useMemo(() => rangeWindow(range, now), [range, now]);
  const q = useSavings(win, {}, "total", enabled);
  const view = useMemo(() => (q.data ? summarizeOptimisation(q.data) : undefined), [q.data]);
  return { view, isPending: q.isPending, isError: q.isError };
}

/**
 * Tokens saved per agent over the range (Saved tokens by agent; Home's PR2d panel, back on the Overview 2026-10-19):
 * the savings read's `by_agent` rows, as many as the server returns, kept to agents in the viewer's directory so a
 * member never sees another owner's agent, ranked largest first. `capped`: the server returned as many rows as asked,
 * so agents past them may have savings too.
 */
export function useSavedByAgent(now: Date, range: Range, fleet: FleetHealth, enabled = true) {
  const win = useMemo(() => rangeWindow(range, now), [range, now]);
  const q = useSavings(win, {}, "agent", enabled, SAVINGS_ROWS);
  const rows = useMemo(() => {
    if (!q.data) return null;
    return q.data.by_agent
      .flatMap((r) => {
        const agent = fleet.byId.get(r.agent_id);
        return r.saved_tokens > 0 && agent
          ? [
              {
                id: r.agent_id,
                name: agent.display_name || agent.name,
                tokens: r.saved_tokens,
                basis: r.basis,
              },
            ]
          : [];
      })
      .sort((a, b) => b.tokens - a.tokens || a.name.localeCompare(b.name));
  }, [q.data, fleet]);
  return {
    rows,
    capped: (q.data?.by_agent.length ?? 0) >= SAVINGS_ROWS,
    isPending: q.isPending,
    error: q.error,
    retry: () => void q.refetch(),
  };
}
export type SavedByAgent = ReturnType<typeof useSavedByAgent>;
