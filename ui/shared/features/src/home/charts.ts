// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/charts.ts. Local edits: Prettier formatting.
/**
 * The home's chart band (plans/feat-agent-centric.md PR2d, decisions #59-63). Pure: every figure comes from the rows
 * (ACL-scoped for every viewer) except spend over time, which reads per-agent series and, on Fleet, the fleet series
 * (superusers only: `spend-timeseries` isn't ACL-scoped).
 *
 * - By agent (spend, saved): the scope's largest values, the rest folded into one Other bar with its count.
 * - Over time: the top spenders' series stacked per UTC day; Other is the fleet's day minus theirs, on Fleet only.
 * - Cost × p95: the rows' dashboard figures, for TokenOps' own panel.
 * - Health: the rows per rating, harnesses and unrated agents counted as Not rated (the `?health=unknown` filter).
 */
import type { Rating } from "../overview/health";
import type { CostPerfRow } from "../tokenops/components/CostPerformance";
import { zeroFillTimeline } from "../tokenops/series";
import type { FinopsSpendTimeseries } from "../tokenops/types";
import { fmtShortDay } from "@nasiko/ui/lib";
import type { HomeRow } from "./rows";

export interface Bar {
  id: string;
  name: string;
  value: number;
}

export interface Bars {
  bars: Bar[];
  /** Everything past the top bars, folded together; null when nothing is left over. */
  other: { count: number; value: number } | null;
  /** The largest bar, so every bar shares one scale (Other is a figure, not a bar). */
  max: number;
}

/** Rows with a positive `key`, largest first (ties by name), the top `n` as bars and the rest as Other. */
export function topBars(rows: readonly HomeRow[], key: "spend" | "saved", n: number): Bars {
  const ranked = rows
    .flatMap((r) => {
      const v = r[key];
      return v !== null && v > 0 ? [{ id: r.id, name: r.displayName, value: v }] : [];
    })
    .sort((a, b) => b.value - a.value || a.name.localeCompare(b.name));
  const bars = ranked.slice(0, n);
  const rest = ranked.slice(n);
  const other = rest.length
    ? { count: rest.length, value: rest.reduce((s, b) => s + b.value, 0) }
    : null;
  return { bars, other, max: Math.max(0, ...bars.map((b) => b.value)) };
}

/** The rows whose spend series stack in Over time: the scope's top spenders in the window. */
export const topSpenders = (rows: readonly HomeRow[], n: number): HomeRow[] =>
  topBars(rows, "spend", n).bars.flatMap((b) => rows.filter((r) => r.id === b.id));

const DAY = 86_400_000;
const utcDay = (t: number) => {
  const d = new Date(t);
  return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
};

/** A series' spend per UTC day across the window (hourly buckets summed into their day; quiet days are zero). */
function perDay(
  series: Pick<FinopsSpendTimeseries, "bucket" | "points">,
  win: { start: Date; end: Date },
): Map<number, number> {
  const out = new Map<number, number>();
  for (const p of zeroFillTimeline(series.points, series.bucket, win.start, win.end)) {
    const day = utcDay(Date.parse(p.iso));
    out.set(day, (out.get(day) ?? 0) + p.spend);
  }
  return out;
}

/** One stacked day: `s0`…`s4` are the top spenders in order, `other` the rest of the fleet (Fleet only). */
export interface StackDay {
  iso: string;
  label: string;
  other: number;
  [series: `s${number}`]: number;
}

/**
 * The top spenders' series stacked per UTC day. With the fleet series (Fleet only), Other is each day's fleet spend less
 * theirs, never below zero (a series read a moment apart can disagree by a rounding).
 */
export function stackDays(
  top: readonly Pick<FinopsSpendTimeseries, "bucket" | "points">[],
  fleet: Pick<FinopsSpendTimeseries, "bucket" | "points"> | null,
  win: { start: Date; end: Date },
): StackDay[] {
  const tops = top.map((s) => perDay(s, win));
  const all = fleet ? perDay(fleet, win) : null;
  const out: StackDay[] = [];
  for (let t = utcDay(win.start.getTime()); t < win.end.getTime(); t += DAY) {
    const iso = new Date(t).toISOString();
    const day: StackDay = { iso, label: fmtShortDay(iso), other: 0 };
    let sum = 0;
    tops.forEach((m, i) => {
      const v = m.get(t) ?? 0;
      day[`s${i}`] = v;
      sum += v;
    });
    day.other = all ? Math.max(0, (all.get(t) ?? 0) - sum) : 0;
    out.push(day);
  }
  return out;
}

/** The rows TokenOps' Cost vs performance panel draws: agents with priced operations and a p95 in the window. */
export function costPerfRows(
  rows: readonly HomeRow[],
  perf: ReadonlyMap<string, AgentPerf> | null,
): CostPerfRow[] | undefined {
  if (!perf) return undefined;
  return rows.flatMap((r) => {
    const p = perf.get(r.id);
    return p && !r.harness
      ? [
          {
            id: r.id,
            name: r.displayName,
            kind: "agent" as const,
            operations: p.operations,
            costPerOp: p.costPerOp,
            p95: p.p95,
            containerHours: p.hours,
          },
        ]
      : [];
  });
}

/** One agent's window figures from the ACL-scoped dashboard (`view=agent`). */
export interface AgentPerf {
  operations: number;
  costPerOp: number;
  p95: number | null;
  hours: number | null;
}

export type HealthKey = Exclude<Rating, "unknown"> | "unknown";
/** Worst first, as the bar draws them. */
export const HEALTH_ORDER: readonly HealthKey[] = ["action", "watch", "healthy", "unknown"];

/** A row's place on the health bar: its rating, or Not rated (harnesses, undeployed agents, unknown inputs). */
export const healthOf = (r: HomeRow): HealthKey => r.health?.rating ?? "unknown";

export function healthCounts(rows: readonly HomeRow[]): Record<HealthKey, number> {
  const out: Record<HealthKey, number> = { action: 0, watch: 0, healthy: 0, unknown: 0 };
  for (const r of rows) out[healthOf(r)]++;
  return out;
}

/** The table's rows under a `?health=` filter (none: every row). */
export const filterByHealth = (rows: HomeRow[], health: HealthKey | undefined): HomeRow[] =>
  health ? rows.filter((r) => healthOf(r) === health) : rows;
