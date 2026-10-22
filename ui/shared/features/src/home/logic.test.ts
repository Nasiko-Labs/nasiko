// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/logic.test.ts. Local edits: Prettier formatting.
/**
 * The home's pure rules (plans/feat-agent-centric.md PR2a; eng `resolveScope` truth table, "Fleet roll-up honesty").
 */
import { describe, expect, it } from "vitest";
import { homeNarrative } from "../narrative/home";
import { homeScopeFor } from "../onboarding/logic";
import { PERSONAS } from "../onboarding/types";
import type { AgentHealth } from "../overview/health";
import {
  budgetMonth,
  buildRows,
  DEFAULT_SORT,
  needsByAgent,
  rowGroups,
  sortRows,
  sparkPoints,
  totals,
  type HomeAgent,
  type HomeRow,
} from "./rows";
import { copy } from "./copy";
import { rememberScope, resolveScope, storedScope } from "./scope";
import { homeSearchSchema } from "./search";

describe("scope", () => {
  it("maps all nine personas, and no persona by role", () => {
    const fleet = PERSONAS.filter((p) => homeScopeFor(p, false) === "fleet");
    expect(fleet).toEqual([
      "platform_engineer",
      "finance",
      "engineering_manager",
      "sre",
      "leadership",
    ]);
    expect(homeScopeFor("developer", true)).toBe("yours");
    expect(homeScopeFor(null, false)).toBe("yours");
    expect(homeScopeFor(null, true)).toBe("fleet");
  });

  it.each([
    // url, stored, persona, interacted → scope, replace
    ["yours", "fleet", "fleet", false, "yours", null],
    [undefined, "yours", "fleet", false, "yours", null],
    [undefined, null, null, false, null, null],
    [undefined, null, "yours", false, "yours", null],
    [undefined, null, "fleet", false, "fleet", "fleet"],
    [undefined, null, "fleet", true, "fleet", null],
  ] as const)(
    "url %s, stored %s, persona %s, interacted %s → %s (replace %s)",
    (url, stored, persona, interacted, scope, replace) => {
      expect(resolveScope({ url, stored, persona, interacted })).toEqual({ scope, replace });
    },
  );

  it("remembers the last choice per user", () => {
    expect(storedScope("u1")).toBeNull();
    rememberScope("u1", "fleet");
    expect(storedScope("u1")).toBe("fleet");
    expect(storedScope("u2")).toBeNull();
  });
});

const agent = (id: string, over: Partial<HomeAgent> = {}): HomeAgent => ({
  id,
  name: id,
  displayName: id.toUpperCase(),
  ownerId: "me",
  harness: false,
  status: "running",
  ...over,
});
const rated = (id: string, rating: AgentHealth["rating"]): AgentHealth => ({
  id,
  name: id,
  rating,
  reasons: [],
});

describe("rows", () => {
  const agents = [agent("a"), agent("b", { ownerId: "other" }), agent("h", { harness: true })];
  const base = {
    agents,
    meSub: "me",
    spend: new Map([["a", { cost: 5, operations: 10 }]]),
    saved: new Map([["a", { tokens: 100, basis: "measured" }]]),
    ratings: new Map([["a", rated("a", "healthy")]]),
    needs: new Map([["b", 2]]),
  };

  it("Yours keeps what the viewer owns; Fleet everything visible", () => {
    expect(buildRows({ ...base, scope: "yours" }).map((r) => r.id)).toEqual(["a", "h"]);
    expect(buildRows({ ...base, scope: "fleet" }).map((r) => r.id)).toEqual(["a", "b", "h"]);
  });

  it("a loaded dashboard without a row is a real zero; an unloaded one is unknown", () => {
    const loaded = buildRows({ ...base, scope: "fleet" });
    expect(loaded.find((r) => r.id === "b")?.spend).toBe(0);
    const pending = buildRows({ ...base, scope: "fleet", spend: null });
    expect(pending.every((r) => r.spend === null)).toBe(true);
    expect(totals(pending).spend).toBeNull();
  });

  it('saved tokens with no row are "not reported", never 0', () => {
    const rows = buildRows({ ...base, scope: "fleet" });
    expect(rows.find((r) => r.id === "b")?.saved).toBeNull();
    expect(totals(rows).saved).toBe(100);
  });

  it("sorts by needs, then spend; unknowns last either way", () => {
    const rows = buildRows({
      ...base,
      scope: "fleet",
      spend: new Map([
        ["a", { cost: 5, operations: 1 }],
        ["h", { cost: 9, operations: 1 }],
      ]),
    });
    expect(sortRows(rows, DEFAULT_SORT).map((r) => r.id)).toEqual(["b", "h", "a"]);
    expect(sortRows(rows, { key: "saved", desc: false }).map((r) => r.id)).toEqual(["a", "h", "b"]);
  });

  it("totals are the sum of the rows shown, and flag harness estimates", () => {
    const rows = buildRows({
      ...base,
      scope: "yours",
      spend: new Map([
        ["a", { cost: 5, operations: 1 }],
        ["b", { cost: 100, operations: 1 }],
        ["h", { cost: 2, operations: 1 }],
      ]),
    });
    expect(totals(rows)).toMatchObject({
      agents: 1,
      harnesses: 1,
      spend: 7,
      spendHasEstimates: true,
      needing: 0,
    });
  });

  it("needs counts come from action rows, failed sessions (unique names) and requests", () => {
    const m = needsByAgent({
      action: ["a"],
      failedSessionAgents: ["raw-b", "shared"],
      idForName: (raw) => (raw === "raw-b" ? "b" : undefined),
      requestAgents: ["a", null, "b"],
    });
    expect([...m]).toEqual([
      ["a", 2],
      ["b", 2],
    ]);
  });
});

describe("the sentence", () => {
  it("names the rows, the spend and what needs the viewer", () => {
    expect(
      homeNarrative({
        scope: "fleet",
        agents: 12,
        harnesses: 4,
        spend: 3840,
        days: 30,
        needing: 3,
      }),
    ).toBe(
      "12 agents and 4 harnesses you can see spent $3,840.00 in the last 30 days; 3 need you.",
    );
    expect(
      homeNarrative({ scope: "yours", agents: 1, harnesses: 0, spend: 0, days: 7, needing: 0 }),
    ).toBe("Your 1 agent spent nothing in the last 7 days.");
    expect(
      homeNarrative({
        scope: "yours",
        agents: 2,
        harnesses: 0,
        spend: null,
        days: 7,
        needing: null,
      }),
    ).toBeNull();
  });
});

describe("owner groups, sparklines, budget month (PR2b)", () => {
  const row = (over: Partial<HomeRow>): HomeRow => ({
    id: over.id ?? "a",
    name: "a",
    displayName: over.id ?? "a",
    ownerId: "o",
    harness: false,
    status: "running",
    mine: false,
    health: null,
    needs: 0,
    spend: 1,
    requests: 1,
    saved: null,
    savedBasis: null,
    budget: null,
    ...over,
  });

  it("orders groups: yours, then named owners by name, then the owners you can’t name", () => {
    const groups = rowGroups(
      [
        row({ id: "1", ownerId: "z" }),
        row({ id: "2", ownerId: "me", mine: true }),
        row({ id: "3", ownerId: "b" }),
        row({ id: "4", ownerId: "a" }),
        row({ id: "5", ownerId: "y" }),
      ],
      true,
      (id) => ({ a: "Ana", b: "Bea" })[id] ?? null,
    )!;
    expect(groups.map((g) => g.label)).toEqual([copy.groups.you, "Ana", "Bea", copy.groups.others]);
    expect(groups.at(-1)!.rows.map((r) => r.id)).toEqual(["1", "5"]);
    expect(rowGroups([row({})], false, () => null)).toBeNull();
  });

  it("a group’s spend is unknown while any of its rows is", () => {
    const [g] = rowGroups(
      [row({ id: "1", spend: 2 }), row({ id: "2", spend: null })],
      true,
      () => "X",
    )!;
    expect(g!.spend).toBeNull();
    const [h] = rowGroups(
      [row({ id: "1", spend: 2 }), row({ id: "2", spend: 3 })],
      true,
      () => "X",
    )!;
    expect(h!.spend).toBe(5);
  });

  it("zero-fills sparse buckets across the window", () => {
    const point = (bucket_start: string, spend_usd: number) => ({
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
    const win = { start: new Date("2026-03-01T00:00:00Z"), end: new Date("2026-03-05T00:00:00Z") };
    expect(sparkPoints({ bucket: "day", points: [point("2026-03-03T00:00:00Z", 4)] }, win)).toEqual(
      [0, 0, 4, 0],
    );
    expect(sparkPoints({ bucket: "day", points: [] }, win)).toEqual([0, 0, 0, 0]);
  });

  it("names the period’s month (the one before the reset), across a year boundary", () => {
    expect(budgetMonth("2027-01-01T00:00:00Z")).toBe("Dec");
    expect(budgetMonth("2026-11-01T00:00:00Z")).toBe("Oct");
    expect(budgetMonth("not a date")).toBe("");
  });
});

describe("org levels in the core (PR2c)", () => {
  it("the scope schema takes yours, fleet and unit:<uuid>, and drops junk", () => {
    const scope = (v: unknown) => homeSearchSchema.parse({ scope: v }).scope;
    expect(scope("yours")).toBe("yours");
    expect(scope("unit:5eed0014-0000-4000-8000-000000000001")).toBe(
      "unit:5eed0014-0000-4000-8000-000000000001",
    );
    expect(scope("unit:")).toBeUndefined();
    expect(scope("unit:<script>")).toBeUndefined();
    expect(scope("team")).toBeUndefined();
    expect(scope("unit:not-a-uuid")).toBeUndefined();
  });

  it("a unit is never remembered as the default scope", () => {
    rememberScope("u-unit", "fleet");
    rememberScope("u-unit", "unit:abc");
    expect(storedScope("u-unit")).toBe("fleet");
  });

  it("a unit keeps the rows its owners own, the viewer’s only when the viewer is one", () => {
    const agents: HomeAgent[] = ["me", "a", "b"].map((owner, i) => ({
      id: `id${i}`,
      name: `n${i}`,
      displayName: `n${i}`,
      ownerId: owner,
      harness: false,
      status: "running",
    }));
    const base = { agents, meSub: "me", spend: null, saved: null, ratings: null, needs: null };
    const ids = (owners: string[]) =>
      buildRows({ ...base, scope: "unit:x", owners: new Set(owners) }).map((r) => r.ownerId);
    expect(ids(["a"])).toEqual(["a"]);
    expect(ids(["a", "me"])).toEqual(["me", "a"]);
    expect(buildRows({ ...base, scope: "unit:x", owners: null })).toEqual([]);
  });

  it("the sentence names the unit", () => {
    expect(
      homeNarrative({
        scope: "unit",
        unitName: "Platform",
        agents: 3,
        harnesses: 1,
        spend: 12,
        days: 30,
        needing: 0,
      }),
    ).toBe("3 agents and 1 harness owned by Platform members spent $12.00 in the last 30 days.");
  });
});
