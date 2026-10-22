// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/api.ts. Local edits: keys are scoped:
// the row series are `productKey(scope, 'home', 'series', …)`, the build records the deploy feature's scoped
// `agentBuildsKey(scope, id)`, and the `homeOrgScope` slot's queries run under `productKey(scope, <their key>, meSub)`
// (the shell's rule for a layer's slot query; the lab appended the viewer to the raw key); the needs counts read the
// Overview's failed-sessions page (`sessions.failed`, filtered in SQL since development 480650a69) instead of the
// checked sample it replaced; `ApiError` is `ProductApiError`; Prettier formatting.
/**
 * The home's reads (plans/feat-agent-centric.md PR2a; eng "Home execution table", in docs/home.md). Everything here is a
 * fleet-level read made once for all rows, never one per row:
 * - the directory (agents and harnesses, with owners);
 * - the ACL-scoped agent dashboard for the window (spend and requests by agent id, and how many calls had no price);
 * - the savings read's `by_agent` rows for the window (tokens saved, with basis), as many as the server returns;
 * - the Overview's ratings (`useFleetHealth`; the fleet-wide spend series only for a superuser) and its Needs you, whose
 *   reads also give the per-agent needs counts (action rows, the checked session sample, pending requests).
 *
 * A figure is shown only for the window it was read for: a window change's kept-previous data is "loading", never the
 * new window's number. A source that failed is unknown ("—"), never 0.
 *
 * PR2b adds one budgets read for all rows (a preview of the proposed R-L10 contract, polled every minute while the tab
 * is visible), and two reads per row on screen through a limiter of 4: the agent's spend series (superusers only: the
 * timeseries isn't ACL-scoped) and its build records (last change). A row read is dropped when its row scrolls away.
 */
import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { useSlots } from "@nasiko/app-core/edition/edition-context";
import type { BudgetsState, HomeLevel } from "@nasiko/app-core/edition/edition";
import { useAgentsDirectory, useUsers } from "../agents/api";
import { agentBuildsKey, fetchAgentBuilds } from "../deploy/api";
import { createLimiter } from "../observability/limiter";
import type { FinopsSpendTimeseries } from "../tokenops/types";
import type { ResolvedWindow } from "../tokenops/window";
import type { Scope as ProductScope } from "@nasiko/data-access";
import {
  ProductApiError as ApiError,
  apiData,
  productKey,
  useProductScope,
  withQuery,
} from "@nasiko/data-access/product";
import { isHarness } from "../agents/status";
import { usePendingRequests } from "../chat/api";
import { pendingRequestRows } from "../chat/waiting";
import { rangeWindow, useFleetHealth, useNeedsYou } from "../overview/api";
import { uniqueByName } from "../overview/health";
import type { Range } from "../overview/search";
import { useDashboard, useSavings } from "../tokenops/api";
import type { Me } from "@nasiko/data-access/product";
import type { AgentPerf } from "./charts";
import { buildRows, needsByAgent, type HomeAgent } from "./rows";
import { isUnitScope, unitOf, type Scope } from "./search";
import { ROW_READ_STALE_MS, ROW_READS, SAVINGS_ROWS } from "./tuning";

/** Where a source stands for the current window: its numbers are shown only when `ok` (or `stale`, kept and labelled). */
export type SourceStatus = "loading" | "ok" | "failed" | "stale";

const statusOf = (q: {
  data: unknown;
  isPlaceholderData: boolean;
  isError: boolean;
  isPending: boolean;
}): SourceStatus =>
  q.isPlaceholderData || (q.isPending && !q.isError)
    ? "loading"
    : q.isError
      ? q.data
        ? "stale"
        : "failed"
      : q.data
        ? "ok"
        : "loading";

export function useHome(now: Date, range: Range, me: Me, scope: Scope | null) {
  const superuser = me.is_superuser;
  const dir = useAgentsDirectory();
  const fleet = useFleetHealth(now, true, superuser);
  const users = useUsers(superuser);
  const win = rangeWindow(range, now);
  const any = (dir.data?.length ?? 0) > 0;
  const dash = useDashboard(win, {}, "agent", "current", any);
  const savings = useSavings(win, {}, "agent", any, SAVINGS_ROWS);
  // Needs you runs once a non-empty directory answered, a harness-only one included (its requests and sessions count
  // too); an empty fleet is the first run, which reads no sessions (design 7A).
  const ready = !!fleet.error || (!!fleet.summary && any);
  const needsYou = useNeedsYou(now, me, fleet, ready);
  const pending = usePendingRequests(me.sub, true);
  const budgets = useSlots().budgets.useHomeBudgets(any, me.sub);
  const org = useOrgLevel(scope, me.sub);

  const agents: HomeAgent[] = (dir.data ?? []).map((a) => ({
    id: a.id,
    name: a.name,
    displayName: a.display_name || a.name,
    ownerId: a.owner_id,
    harness: isHarness(a),
    status: a.status,
  }));

  const spendStatus = statusOf(dash);
  const spend =
    dash.data && spendStatus !== "loading" && spendStatus !== "failed"
      ? new Map(
          dash.data.agents.map((r) => [
            r.agent_id,
            { cost: r.total_cost, operations: r.operations },
          ]),
        )
      : null;
  // Cost × p95's figures (PR2d): the same ACL-scoped rows, only while they describe this window.
  const perf: Map<string, AgentPerf> | null =
    dash.data && spend
      ? new Map(
          dash.data.agents.map((r) => [
            r.agent_id,
            {
              operations: r.operations,
              costPerOp: r.avg_cost_per_operation,
              p95: r.avg_latency_p95_ms,
              hours: r.container_hours,
            },
          ]),
        )
      : null;
  const savedStatus = statusOf(savings);
  const saved =
    savings.data && savedStatus !== "loading" && savedStatus !== "failed"
      ? new Map(
          savings.data.by_agent.map((r) => [
            r.agent_id,
            { tokens: r.saved_tokens, basis: r.basis },
          ]),
        )
      : null;

  // Needs counts: fleet-level reads only (eng). Unknown while any source loads or failed, never 0.
  const { needs, sessions, waiting } = needsYou;
  // Every failed session of the last 7 days (up to the page's size), filtered in SQL: the raw agent names they name.
  const failedRaw = sessions.failed.rows.flatMap((r) => (r.agent_id ? [r.agent_id] : []));
  const requests = pendingRequestRows(superuser, waiting.match, pending.data?.items);
  const needsUnknown =
    !ready ||
    needs.loading ||
    needs.failed.length > 0 ||
    waiting.isPending ||
    waiting.failedCold ||
    pending.isPending ||
    (pending.isError && !pending.data) ||
    sessions.failed.isPending ||
    sessions.failed.isError;
  const needsMap = needsUnknown
    ? null
    : needsByAgent({
        action: needs.rows.flatMap((r) => (r.kind === "agent" ? [r.id] : [])),
        failedSessionAgents: failedRaw,
        idForName: uniqueByName(dir.byNameAll),
        requestAgents: requests.filter((r) => r.proven).map((r) => r.agentId),
      });

  // An org level's rows wait for its owners: never the whole fleet under a unit's name.
  const rows =
    scope && dir.data && (!isUnitScope(scope) || org.owners !== null)
      ? buildRows({
          agents,
          meSub: me.sub,
          scope,
          spend,
          saved,
          // Ratings settle together: a half-loaded set calls agents Healthy before their comparisons arrive.
          ratings: fleet.ratingsPending ? null : (fleet.summary?.byId ?? null),
          needs: needsMap,
          budgets: budgets.byAgent,
          owners: org.owners,
        })
      : null;

  return {
    dir,
    /** The directory failed before it ever answered (a failed refresh keeps the rows it had). */
    dirFailed: dir.isError && !dir.data,
    needsYou,
    rows,
    ratingsPending: fleet.ratingsPending,
    budgets,
    org,
    win,
    owner: (id: string) => users.data?.get(id) ?? org.names.get(id) ?? null,
    spendStatus,
    perf,
    /** The dashboard's failure, for the band's Cost × p95 panel (its retry is `retryMoney`). */
    dashError: dash.error,
    /** Calls in the window had no price: spend is a floor. */
    unpriced: (dash.data?.summary.unpriced_calls ?? 0) > 0 && spendStatus !== "loading",
    savedStatus,
    /** The server returned as many savings rows as asked: agents past them may have savings too. */
    savedCapped: (savings.data?.by_agent.length ?? 0) >= SAVINGS_ROWS,
    asOf: spendStatus === "ok" || spendStatus === "stale" ? dash.dataUpdatedAt : 0,
    retryMoney: () => {
      if (dash.isError) void dash.refetch();
      if (savings.isError) void savings.refetch();
    },
  };
}

// ─── PR2b: budgets, and the reads per row on screen ─────────────────────────────────────────────

/**
 * Budgets are an Enterprise feature (docs/requirements/budgets.md): the column's data comes from the `budgets` slot,
 * and an OSS build's answer is `off` without a request. The shapes stay the core's contract (`app/edition.ts`), so
 * the table, the phone rows and the header read them exactly as before.
 */
export type { BudgetsState, HomeBudgets } from "@nasiko/app-core/edition/edition";

/** Budget figures are drawn while they are current or kept from the last successful read. */
export const budgetsShown = (s: BudgetsState) => s === "ok" || s === "stale";

/** Row reads in flight across the page (eng "Home execution table": `ROW_READS`). */
const rowLimiter = createLimiter(ROW_READS);

const FINOPS = "/api/observability/finops";

/**
 * One agent's spend series for the window, for its sparkline. Superusers only: `/finops/spend-timeseries` is fleet-wide
 * on the server, not ACL-scoped (eng "Member money"). Mounted only while the row is on screen; leaving it cancels the
 * read, or drops it from the limiter's queue.
 *
 * The bounds are the page's frozen window, sent explicitly: a relative `range` read when a row scrolls in later would
 * describe a later window than the totals beside it. Each row is its own query on the server (one `trace_usage`
 * aggregate), so a failure isn't retried (the row keeps its total) and a batched per-agent series is a server ask.
 */
export function useRowSeries(id: string, win: Pick<ResolvedWindow, "start" | "end">) {
  return useQuery(seriesQuery(useProductScope(), id, win));
}

/**
 * The series read behind `useRowSeries`; `null` reads the whole fleet's (the chart band's Other, on Fleet only). The
 * band stacks the same queries the sparklines read, so a row on screen costs its series once.
 */
export function seriesQuery(
  scope: ProductScope,
  id: string | null,
  win: Pick<ResolvedWindow, "start" | "end">,
) {
  const bounds = { start_time: win.start.toISOString(), end_time: win.end.toISOString() };
  const path = withQuery(`${FINOPS}/spend-timeseries`, id ? { ...bounds, agent_id: id } : bounds);
  return queryOptions({
    queryKey: productKey(
      scope,
      "home",
      "series",
      id ?? "fleet",
      bounds.start_time,
      bounds.end_time,
    ),
    queryFn: ({ signal }) =>
      rowLimiter(signal, async () => {
        const body = await apiData<FinopsSpendTimeseries>(path, { signal });
        // A malformed body costs this row its line, never the table (it renders during the cell's render).
        // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- defensive read of server data (the lab's)
        if (!body || !Array.isArray(body.points))
          throw new ApiError(200, body, path, `GET ${path} → unexpected response (no points)`);
        return body;
      }),
    staleTime: ROW_READ_STALE_MS,
    retry: false,
    meta: { path: `${FINOPS}/spend-timeseries` },
  });
}

/**
 * One agent's build records (newest 20), for its last change; `{available:false}` without deploy rights. The deploy
 * feature's own key, so a build started, finished or invalidated anywhere in the app reaches the row.
 */
export function useRowBuilds(id: string) {
  return useQuery({
    queryKey: agentBuildsKey(useProductScope(), id),
    queryFn: ({ signal }) => rowLimiter(signal, () => fetchAgentBuilds(id, signal)),
    staleTime: ROW_READ_STALE_MS,
    retry: false,
    meta: { path: "/api/builds/agent/{id}" },
  });
}

// ─── PR2c: the org levels an edition layer adds (`homeOrgScope`) ─────────────────────────────────────────────

/**
 * Where an org level stands: `none` without a unit in the URL; `unknown` when the viewer can't pick that unit (not
 * listed, deleted, outside a manager's subtrees, a 404/403 or `{"available": false}` on its members, or a build without
 * the slot); `failed` when the units or members couldn't load at all; `stale` when a members refresh failed and the last
 * read stays (said so on the page).
 */
export type OrgLevelState = "none" | "loading" | "ok" | "stale" | "unknown" | "failed";

export interface OrgLevel {
  /** Levels the scope control offers after Yours and Fleet (empty without the slot, or for a member). */
  levels: readonly HomeLevel[];
  state: OrgLevelState;
  /** The chosen level, once the units answered. */
  level: HomeLevel | null;
  /** Its owners (members, subtree included), once read. */
  owners: ReadonlySet<string> | null;
  /** Its owners' names, for the owner groups (a manager reads no user list). */
  names: ReadonlyMap<string, string>;
  retry: () => void;
}

// Raw parts like a slot query's: `viewer` below scopes them with the product scope and the viewer.
const NO_QUERY_PARTS = ["home", "org", "none"] as const;
const NO_QUERY = {
  queryKey: NO_QUERY_PARTS,
  queryFn: () => Promise.resolve(null),
};
const NO_NAMES: ReadonlyMap<string, string> = new Map();
/** The members read says this viewer may not read the unit (gone, or out of their subtrees): never keep its rows. */
const denied = (e: unknown) => e instanceof ApiError && (e.status === 404 || e.status === 403);

function useOrgLevel(scope: Scope | null, meSub: string): OrgLevel {
  const slot = useSlots().homeOrgScope;
  const qc = useQueryClient();
  const product = useProductScope();
  // Scoped as the shell scopes a layer's slot query, and the viewer is in every key: a cached list never serves another
  // account or persona.
  const viewer = <Q extends { queryKey: readonly unknown[] }>(q: Q) => {
    const [feature = "home", ...parts] = q.queryKey;
    return { ...q, queryKey: productKey(product, String(feature), ...parts, meSub) };
  };
  const levelsQuery = viewer(slot?.levels.query ?? NO_QUERY);
  const levelsQ = useQuery({ ...levelsQuery, enabled: !!slot });
  const levels = slot && levelsQ.data !== undefined ? slot.levels.list(levelsQ.data) : [];
  const unitId = isUnitScope(scope) ? unitOf(scope) : null;
  const level = unitId ? (levels.find((l) => l.id === unitId) ?? null) : null;
  const ownersQ = useQuery({
    ...viewer(slot && unitId ? slot.owners.query(unitId) : NO_QUERY),
    enabled: !!slot && !!level,
  });
  const ids = slot && ownersQ.data != null ? slot.owners.ids(ownersQ.data) : null;
  const lost = denied(ownersQ.error) || (ownersQ.data != null && ids === null);
  // A unit the server no longer lets this viewer read: re-read the list, so the picker drops it too.
  // Keyed by its text: the key array is rebuilt every render, and the effect must run once per loss.
  const levelsKey = JSON.stringify(levelsQuery.queryKey);
  useEffect(() => {
    if (lost)
      void qc.invalidateQueries({ queryKey: JSON.parse(levelsKey) as unknown[], exact: true });
  }, [lost, qc, levelsKey]);
  const state: OrgLevelState = !unitId
    ? "none"
    : !slot
      ? "unknown"
      : levelsQ.isError && levelsQ.data === undefined
        ? "failed"
        : levelsQ.data === undefined
          ? "loading"
          : !level || lost
            ? "unknown"
            : ownersQ.isError
              ? ids
                ? "stale"
                : "failed"
              : ids
                ? "ok"
                : "loading";
  const shown = state === "ok" || state === "stale";
  return {
    levels,
    state,
    level,
    owners: shown ? ids : null,
    names: shown && slot && ownersQ.data != null ? slot.owners.names(ownersQ.data) : NO_NAMES,
    retry: () => {
      if (levelsQ.isError) void levelsQ.refetch();
      if (ownersQ.isError) void ownersQ.refetch();
    },
  };
}
