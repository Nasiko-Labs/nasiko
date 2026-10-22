// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/charts.test.ts. Local edits: Prettier formatting.
/**
 * The chart band's pure rules (plans/feat-agent-centric.md PR2d, decisions #60-62): ranked bars with Other, the stacked
 * days and their Other, Cost × p95's rows, and the health counts and filter.
 */
import { describe, expect, it } from "vitest";
import type { SpendTimeseriesPoint } from "../tokenops/types";
import {
  costPerfRows,
  filterByHealth,
  healthCounts,
  stackDays,
  topBars,
  topSpenders,
  type AgentPerf,
} from "./charts";
import type { HomeRow } from "./rows";

const row = (id: string, o: Partial<HomeRow> = {}): HomeRow => ({
  id,
  name: id,
  displayName: id.toUpperCase(),
  ownerId: "me",
  harness: false,
  status: "running",
  mine: true,
  health: null,
  needs: 0,
  spend: 0,
  requests: 0,
  saved: null,
  savedBasis: null,
  budget: null,
  ...o,
});

const rated = (id: string, rating: "healthy" | "watch" | "action" | "unknown") =>
  row(id, { health: { id, name: id, rating, reasons: [] } });

describe("topBars", () => {
  it("ranks positive values, folds the rest into Other, and scales to the largest bar", () => {
    const rows = [
      row("a", { spend: 1 }),
      row("b", { spend: 5 }),
      row("c", { spend: 3 }),
      row("d", { spend: 2 }),
      row("z", { spend: 0 }),
      row("n", { spend: null }),
    ];
    const out = topBars(rows, "spend", 2);
    expect(out.bars.map((b) => [b.id, b.value])).toEqual([
      ["b", 5],
      ["c", 3],
    ]);
    // Zero and unknown spend are neither bars nor Other.
    expect(out.other).toEqual({ count: 2, value: 3 });
    // Other is a figure, not a bar: it never sets the scale, even when it's the largest.
    expect(topBars([...rows, row("e", { spend: 4 })], "spend", 1)).toMatchObject({
      max: 5,
      other: { count: 4, value: 10 },
    });
  });

  it("has no Other when everything fits, and breaks ties by name", () => {
    const out = topBars([row("b", { saved: 2 }), row("a", { saved: 2 })], "saved", 5);
    expect(out.bars.map((b) => b.name)).toEqual(["A", "B"]);
    expect(out.other).toBeNull();
    expect(topBars([], "saved", 5)).toEqual({ bars: [], other: null, max: 0 });
  });

  it("topSpenders keeps the bars’ order", () => {
    const rows = [row("a", { spend: 1 }), row("b", { spend: 9 }), row("c", { spend: 4 })];
    expect(topSpenders(rows, 2).map((r) => r.id)).toEqual(["b", "c"]);
  });
});

describe("stackDays", () => {
  const win = { start: new Date("2026-10-01T06:00:00Z"), end: new Date("2026-10-04T06:00:00Z") };
  const pt = (bucket_start: string, spend_usd: number): SpendTimeseriesPoint => ({
    bucket_start,
    spend_usd,
    operations: 1,
    tool_calls: 0,
    top_agent_name: null,
    top_agent_spend_usd: null,
    p50_latency_ms: null,
    p95_latency_ms: null,
    p99_latency_ms: null,
  });

  it("sums hourly buckets into UTC days, zero on quiet days, one entry per day of the window", () => {
    const a = {
      bucket: "hour" as const,
      points: [pt("2026-10-01T07:00:00Z", 1), pt("2026-10-01T22:00:00Z", 2)],
    };
    const days = stackDays([a], null, win);
    expect(days.map((d) => d.iso.slice(0, 10))).toEqual([
      "2026-10-01",
      "2026-10-02",
      "2026-10-03",
      "2026-10-04",
    ]);
    expect(days.map((d) => d.s0)).toEqual([3, 0, 0, 0]);
    // Without the fleet series (Yours, a unit) there is no Other.
    expect(days.every((d) => d.other === 0)).toBe(true);
  });

  it("Other is the fleet’s day less the top spenders’, never below zero", () => {
    const day = (spend: number, date: string) => ({
      bucket: "day" as const,
      points: [pt(`${date}T00:00:00Z`, spend)],
    });
    const a = day(2, "2026-10-02");
    const b = day(1, "2026-10-02");
    const fleet = {
      bucket: "day" as const,
      points: [pt("2026-10-02T00:00:00Z", 10), pt("2026-10-03T00:00:00Z", 0)],
    };
    const days = stackDays([a, b], fleet, win);
    const oct2 = days.find((d) => d.iso.startsWith("2026-10-02"));
    expect(oct2).toMatchObject({ s0: 2, s1: 1, other: 7 });
    // A fleet read a moment apart can trail its agents': Other floors at zero.
    expect(stackDays([day(5, "2026-10-02")], day(4.99, "2026-10-02"), win)[1]?.other).toBe(0);
  });
});

describe("costPerfRows", () => {
  const perf = new Map<string, AgentPerf>([
    ["a", { operations: 10, costPerOp: 0.01, p95: 1200, hours: 2 }],
    ["h", { operations: 5, costPerOp: 0.02, p95: 900, hours: null }],
  ]);

  it("maps the rows’ dashboard figures for agents, never harnesses; unknown while the dashboard loads", () => {
    const rows = [row("a"), row("h", { harness: true }), row("x")];
    expect(costPerfRows(rows, perf)).toEqual([
      {
        id: "a",
        name: "A",
        kind: "agent",
        operations: 10,
        costPerOp: 0.01,
        p95: 1200,
        containerHours: 2,
      },
    ]);
    expect(costPerfRows(rows, null)).toBeUndefined();
  });
});

describe("health", () => {
  const rows = [
    rated("a", "action"),
    rated("b", "healthy"),
    rated("c", "healthy"),
    rated("u", "unknown"),
    row("h", { harness: true }),
  ];

  it("counts each rating, unrated rows and harnesses as Not rated", () => {
    expect(healthCounts(rows)).toEqual({ action: 1, watch: 0, healthy: 2, unknown: 2 });
  });

  it("filters to one rating, or keeps every row", () => {
    expect(filterByHealth(rows, "healthy").map((r) => r.id)).toEqual(["b", "c"]);
    expect(filterByHealth(rows, "unknown").map((r) => r.id)).toEqual(["u", "h"]);
    expect(filterByHealth(rows, undefined)).toBe(rows);
  });
});
