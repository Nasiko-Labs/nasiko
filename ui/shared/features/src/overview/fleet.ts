// Ported from nasiko-ui-lab@1b99bdda common/src/features/overview/api.ts (`useFleetHealth`, `FleetHealth` and their
// helpers only). Local edits: split out of overview/api.ts into this module so the Agents catalog's `?health=` filter
// can use it before the Overview page is ported (overview/api.ts re-exports it from here); imports are the ported
// modules; `displayName` is declared as a function property (the Overview passes it on unbound); Prettier formatting.
/**
 * The fleet ratings (plans/feat-overview.md §5, eng review R1): `useFleetHealth()` is the one source of them, called
 * by the Overview and by the Agents catalog while `?health=` is set, so both apply the same rules to the same directory
 * rows. It reuses existing query keys, so TokenOps and the Agents pages open warm.
 */
import { useCallback, useMemo } from "react";
import { useAgentsDirectory } from "../agents/api";
import { displayStatus, isHarness } from "../agents/status";
import type { Agent } from "../agents/types";
import { useBudgetStatus, useBudgets } from "../router/api";
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
 */
export function useFleetHealth(now: Date, enabled = true): FleetHealth {
  const dir = useAgentsDirectory(enabled);
  const win7 = useMemo(() => resolveWindow({ preset: "7d" }, now), [now]);
  const win30 = useMemo(() => resolveWindow({ preset: "30d" }, now), [now]);
  const agents = useMemo(
    () => (enabled ? (dir.data ?? []).filter((a) => !isHarness(a)) : []),
    [enabled, dir.data],
  );
  // First run (no agents but harnesses) fetches nothing else (design review 7A).
  const on = enabled && agents.length > 0;
  const cur = useDashboard(win7, {}, "agent", "current", on);
  const prev = useDashboard(win7, {}, "agent", "previous", on);
  const d30 = useDashboard(win30, {}, "agent", "current", on);
  const ts = useTimeseries(win30, {}, on);
  // Budgets hidden: no server support for /api/budgets yet (R-L10). Restore the commented lines when it lands.
  // const budgets = useBudgets(on)
  // const budgetsAbsent = isEndpointAbsent(budgets.error)
  const budgets = useBudgets(false);
  const budgetsAbsent = true;
  // Only once the list answered: on a server without budgets (bare 404) no status request goes out.
  const budgetStatus = useBudgetStatus(on && budgets.isSuccess);
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
    on &&
    (cur.isError ||
      prev.isError ||
      d30.isError ||
      ts.isError ||
      (budgets.isError && !budgetsAbsent) ||
      budgetStatus.isError);

  const summary = useMemo(() => {
    if (!enabled || !dir.data) return null;
    const t = now.getTime();
    const current = cur.data && !costFailed ? usage(cur.data.agents) : null;
    const previous = prev.data && !prev.isError ? usage(prev.data.agents) : null;
    const perOp30 = new Map(
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- defensive read of server data (the lab's)
      (d30.data?.agents ?? []).map((r) => [r.agent_id, r.avg_cost_per_operation ?? null]),
    );
    const days =
      ts.data?.bucket === "day"
        ? zeroFillTimeline(ts.data.points, "day", win30.start, win30.end).map((p) => ({
            date: p.iso.slice(0, 10),
            spend: p.spend,
            topAgent: p.topAgent,
          }))
        : [];
    const spikes = spikeDrivers(days, uniqueByName(dir.byNameAll), t);
    const budgetByAgent = new Map<string, { stopped: boolean; exceeded: boolean }>();
    const statusById = new Map((budgetStatus.data?.data ?? []).map((st) => [st.budget_id, st]));
    for (const b of budgets.data ?? []) {
      const st = statusById.get(b.id);
      if (b.agent_id && st)
        budgetByAgent.set(b.agent_id, { stopped: st.stopped, exceeded: st.state === "exceeded" });
    }
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
    prev.data,
    prev.isError,
    d30.data,
    ts.data,
    win30,
    budgets.data,
    budgetStatus.data,
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
  const budgetsSettled =
    // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- the lab's guard, kept as written
    budgetsAbsent || budgets.isError || budgetStatus.isSuccess || budgetStatus.isError;
  const ratingsPending =
    on && (cur.isPending || prev.isPending || d30.isPending || ts.isPending || !budgetsSettled);
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
      for (const q of [cur, prev, d30, ts, budgets, budgetStatus]) if (q.isError) void q.refetch();
    },
  };
}
