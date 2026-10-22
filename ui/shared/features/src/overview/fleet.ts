// Ported from nasiko-ui-lab@1b99bdda common/src/features/overview/api.ts (`useFleetHealth`, `FleetHealth` and their
// helpers only). Local edits: split out of overview/api.ts into this module so the Agents catalog's `?health=` filter
// can use it before the Overview page is ported (overview/api.ts re-exports it from here); imports are the ported
// modules; `displayName` is declared as a function property (the Overview passes it on unbound); development
// (be271b564, 1c28b0bb9): `fleetSeries` (members never read the fleet-wide spend series) and the budgets slot's
// `useOverviewBudgets` for the budget reasons; Prettier formatting.
/**
 * The fleet ratings (plans/feat-overview.md §5, eng review R1): `useFleetHealth()` is the one source of them, called
 * by the Overview and by the Agents catalog while `?health=` is set, so both apply the same rules to the same directory
 * rows. It reuses existing query keys, so TokenOps and the Agents pages open warm.
 */
import { useCallback, useMemo } from "react";
import { useAgentsDirectory } from "../agents/api";
import { displayStatus, isHarness } from "../agents/status";
import type { Agent } from "../agents/types";
import { useSlots } from "@nasiko/app-core/edition/edition-context";
import { useDashboard, useTimeseries } from "../tokenops/api";
import { zeroFillTimeline } from "../tokenops/series";
import type { AgentFinopsRow } from "../tokenops/types";
import { resolveWindow } from "../tokenops/window";
import {
  RATED_STATUSES,
  rateAgent,
  spikeDrivers,
  summarizeFleet,
  uniqueByName,
  type FleetHealthSummary,
  type UsageWindow,
} from "./health";

/** Agents the ratings cover: every visible agent except coding harnesses (rated on the Harnesses card). */
const RATED = new Set<string>(RATED_STATUSES);

const usage = (rows: readonly AgentFinopsRow[] | undefined) => {
  const m = new Map<string, UsageWindow>();
  for (const r of rows ?? [])
    m.set(r.agent_id, {
      cost: r.total_cost,
      operations: r.operations,
      p95: r.avg_latency_p95_ms ?? null,
    });
  return m;
};
/**
 * The preceding window, read off `compare=previous` instead of a second request (ported from nasiko-cloud-rs
 * development, 480650a69). `null` when the response carried no comparison, so "no previous data" stays
 * distinguishable from "every agent spent nothing".
 */
const previousUsage = (rows: readonly AgentFinopsRow[] | undefined) => {
  if (!rows?.some((r) => r.previous)) return null;
  const m = new Map<string, UsageWindow>();
  for (const r of rows)
    m.set(r.agent_id, {
      cost: r.previous?.total_cost ?? 0,
      operations: r.previous?.operations ?? 0,
      // The comparison window only carries what a delta needs; p95 is not part
      // of it, and inventing one would rate latency against a fabricated number.
      p95: null,
    });
  return m;
};
const ZERO: UsageWindow = { cost: 0, operations: 0, p95: null };

export interface FleetHealth {
  /** The directory hasn't answered yet: nothing can be rated. */
  isPending: boolean;
  /** The directory failed: the card shows its error. */
  error: unknown;
  summary: FleetHealthSummary | null;
  /** Some rating inputs are still loading (dashboards, spend series, budgets): Needs you waits (design review 5A). */
  ratingsPending: boolean;
  /** A cost input failed (7-day or previous-week or 30-day dashboard, spend series, budgets): every agent's cost
   *  rating is Unknown, never Healthy, and the card says so (/ship adversarial). */
  costFailed: boolean;
  /** Visible agents other than harnesses; zero means first run (design review 7A). */
  agentCount: number;
  /** Of those, how many are deployed (running, deploying or crashed/failed): only these can be rated. */
  deployedCount: number;
  /** The directory's indexes, so Needs you labels rows without a second directory observer. */
  byId: ReadonlyMap<string, Agent>;
  /** A raw agent name → its display name, only when exactly one visible agent has that name (else the raw name). */
  displayName: (raw: string) => string;
  /** Each agent's p95 latency over the last 7 days (the window the ratings use); absent when it had no calls. */
  p95: ReadonlyMap<string, number>;
  retry: () => void;
  retryCost: () => void;
}

/**
 * Ratings for every visible agent. `enabled: false` (the Agents catalog without `?health=`) fetches and computes
 * nothing. There are no per-agent `/deployment` reads: the recent-crash rule was dropped (/ship review).
 * `fleetSeries: false` (anyone but a superuser) never reads the fleet-wide spend series, which isn't ACL-scoped
 * (plans/feat-agent-centric.md eng "Health input contract"): there is no spike check then, only the ACL-scoped dashboards.
 */
export function useFleetHealth(now: Date, enabled = true, fleetSeries = true): FleetHealth {
  const dir = useAgentsDirectory(enabled);
  const win7 = useMemo(() => resolveWindow({ preset: "7d" }, now), [now]);
  const win30 = useMemo(() => resolveWindow({ preset: "30d" }, now), [now]);
  const agents = useMemo(
    () => (enabled ? (dir.data ?? []).filter((a) => !isHarness(a)) : []),
    [enabled, dir.data],
  );
  // First run (no agents but harnesses) fetches nothing else (design review 7A).
  const on = enabled && agents.length > 0;
  // One dashboard, not two: `compare=previous` carries each agent's preceding
  // window in the same response (the server already queried it for the KPI
  // strip and used to discard the per-agent detail).
  const cur = useDashboard(win7, {}, "agent", "current", on, true);
  const d30 = useDashboard(win30, {}, "agent", "current", on);
  const ts = useTimeseries(win30, {}, on && fleetSeries);
  // Budgets are Enterprise only (docs/requirements/budgets.md): a stopped budget is an agent's reason where the
  // edition has them. An OSS build answers settled-and-empty without a request, so ratings never wait on it.
  const budgetInputs = useSlots().budgets.useOverviewBudgets(on);
  const deployedCount = useMemo(
    () => agents.filter((a) => RATED.has(displayStatus(a.status, false))).length,
    [agents],
  );
  const { byId, byNameAll } = dir;
  const displayName = useCallback(
    (raw: string) => {
      const id = uniqueByName(byNameAll)(raw);
      return (id ? byId.get(id)?.display_name : undefined) || raw;
    },
    [byId, byNameAll],
  );
  // Any failed cost input leaves ratings incomplete: rate cost as unknown rather than silently healthy.
  const costFailed =
    on && (cur.isError || d30.isError || (fleetSeries && ts.isError) || budgetInputs.failed);

  const summary = useMemo(() => {
    if (!enabled || !dir.data) return null;
    const t = now.getTime();
    const current = cur.data && !costFailed ? usage(cur.data.agents) : null;
    // Same response as `current`, so it can no longer fail or arrive separately.
    const previous = cur.data ? previousUsage(cur.data.agents) : null;
    const perOp30 = new Map(
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- defensive read of server data (the lab's)
      (d30.data?.agents ?? []).map((r) => [r.agent_id, r.avg_cost_per_operation ?? null]),
    );
    const days =
      // Gate the data, not just the fetch: a disabled query still hands out a cached fleet series (TokenOps fills the same
      // key), which a member must never be rated on (review: security, Codex P1).
      fleetSeries && ts.data?.bucket === "day"
        ? zeroFillTimeline(ts.data.points, "day", win30.start, win30.end).map((p) => ({
            date: p.iso.slice(0, 10),
            spend: p.spend,
            topAgent: p.topAgent,
          }))
        : [];
    const spikes = spikeDrivers(days, uniqueByName(dir.byNameAll), t);
    const budgetByAgent = budgetInputs.byAgent;
    return summarizeFleet(
      agents.map((a) =>
        rateAgent(
          {
            id: a.id,
            name: a.display_name || a.name,
            display: displayStatus(a.status, false),
            raw: a.status,
            createdAt: a.created_at,
            updatedAt: a.updated_at,
            // A loaded window without the agent's row is a real zero; a missing or failed window is unknown.
            current: current ? (current.get(a.id) ?? ZERO) : null,
            previous: previous ? (previous.get(a.id) ?? ZERO) : null,
            costPerOp30d: perOp30.get(a.id) ?? null,
            budget: budgetByAgent.get(a.id),
            spike: spikes.get(a.id),
          },
          t,
        ),
      ),
    );
  }, [
    enabled,
    dir.data,
    dir.byNameAll,
    agents,
    now,
    cur.data,
    costFailed,
    d30.data,
    ts.data,
    fleetSeries,
    win30,
    budgetInputs.byAgent,
  ]);

  const p95 = useMemo(
    () =>
      new Map(
        (cur.data?.agents ?? []).flatMap((r) =>
          r.avg_latency_p95_ms == null ? [] : [[r.agent_id, r.avg_latency_p95_ms] as const],
        ),
      ),
    [cur.data],
  );
  const ratingsPending =
    on &&
    (cur.isPending || d30.isPending || (fleetSeries && ts.isPending) || !budgetInputs.settled);
  return {
    isPending: dir.isPending || (on && cur.isPending),
    error: dir.error,
    summary,
    ratingsPending,
    costFailed,
    agentCount: agents.length,
    deployedCount,
    byId: dir.byId,
    displayName,
    p95,
    retry: () => void dir.refetch(),
    retryCost: () => {
      // Never the fleet series for a member: Retry must not send the request the gate withholds.
      for (const q of [cur, d30, ...(fleetSeries ? [ts] : [])]) if (q.isError) void q.refetch();
      budgetInputs.retry();
    },
  };
}
