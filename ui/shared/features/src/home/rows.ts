// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/rows.ts. Local edits: optional props
// admit `undefined` (exactOptionalPropertyTypes); Prettier formatting.
/**
 * The home's rows and totals (plans/feat-agent-centric.md PR2; design "Home"; eng "Fleet roll-up honesty", "Home execution
 * table"). Pure.
 *
 * - One row per visible agent or coding harness. Yours keeps the viewer's own; Fleet keeps everything they can see.
 * - Money comes only from the ACL-scoped dashboard (`view=agent`) for the window, by agent id: a loaded dashboard without
 *   the agent's row is a real zero; an unloaded or failed one is unknown ("—"), never 0. Harness money is an estimate
 *   (API list price) and keeps that basis.
 * - Saved tokens come from the savings read's `by_agent` rows, with each row's basis; no row is "not reported".
 * - Health is the Overview's rating (`overview/health.ts`); harnesses and agents it can't rate have none.
 * - Needs is a count from fleet-level reads only: the agent needing action, its share of the failed sessions in the
 *   checked sample, and its pending requests. Unknown while those reads load.
 * - Totals are the sum of the rows shown, so a member never sees a total that doesn't match their rows.
 * - Budget use (PR2b) is the agent budget's status, a preview of the proposed R-L10 contract: present only for agents
 *   with a budget the viewer can read. It never sorts, so a background refresh never moves a row.
 */
import type { AgentHealth } from "../overview/health";
import type { SpendTimeseriesPoint } from "../tokenops/types";
import type { Crossing } from "../tokenops/forecast";
import { zeroFillTimeline } from "../tokenops/series";
import { copy } from "./copy";
import type { Scope } from "./search";

export interface HomeAgent {
  id: string;
  name: string;
  displayName: string;
  ownerId: string;
  harness: boolean;
  status: string;
}

export interface HomeRow extends HomeAgent {
  mine: boolean;
  health: AgentHealth | null;
  /** Pending items for this agent; null while the needs reads load. */
  needs: number | null;
  /** Window spend (USD); null when the dashboard hasn't answered. */
  spend: number | null;
  requests: number | null;
  /** Tokens saved in the window; null when the savings read has no row for the agent. */
  saved: number | null;
  savedBasis: string | null;
  /** This agent's budget for the month; null when it has none the viewer can read (or budgets are off). */
  budget: RowBudget | null;
}

/** One budget's use this period (the status read), and when it reaches its limit at this pace (X5). */
export interface RowBudget {
  used: number;
  limit: number;
  /** When the period resets (ISO), for the month name. */
  resetsAt: string;
  /** Calls with no price this period: counted as $0, so use is a floor. */
  unpriced: boolean;
  crossing: Crossing | null;
}

export interface HomeInput {
  agents: readonly HomeAgent[];
  meSub: string;
  scope: Scope;
  spend: ReadonlyMap<string, { cost: number; operations: number }> | null;
  saved: ReadonlyMap<string, { tokens: number; basis: string }> | null;
  ratings: ReadonlyMap<string, AgentHealth> | null;
  needs: ReadonlyMap<string, number> | null;
  budgets?: ReadonlyMap<string, RowBudget> | null | undefined;
  /** An org level's owners (PR2c): the rows they own, the viewer's own included only when the viewer is one. */
  owners?: ReadonlySet<string> | null | undefined;
}

export function buildRows(i: HomeInput): HomeRow[] {
  return i.agents
    .filter((a) =>
      i.scope === "fleet"
        ? true
        : i.scope === "yours"
          ? a.ownerId === i.meSub
          : !!i.owners?.has(a.ownerId),
    )
    .map((a) => {
      const money = i.spend ? (i.spend.get(a.id) ?? { cost: 0, operations: 0 }) : null;
      const saved = i.saved?.get(a.id);
      return {
        ...a,
        mine: a.ownerId === i.meSub,
        health: i.ratings?.get(a.id) ?? null,
        needs: i.needs ? (i.needs.get(a.id) ?? 0) : null,
        spend: money ? money.cost : null,
        requests: money ? money.operations : null,
        saved: saved ? saved.tokens : null,
        savedBasis: saved ? saved.basis : null,
        budget: i.budgets?.get(a.id) ?? null,
      };
    });
}

export type SortKey = "needs" | "spend" | "saved" | "name";
export interface Sort {
  key: SortKey;
  desc: boolean;
}
export const DEFAULT_SORT: Sort = { key: "needs", desc: true };

/** Unknown values sort last in either direction; ties fall back to spend, then name. */
export function sortRows(rows: readonly HomeRow[], sort: Sort): HomeRow[] {
  const value = (r: HomeRow): number | string | null =>
    sort.key === "name" ? r.displayName.toLowerCase() : r[sort.key];
  const dir = sort.desc ? -1 : 1;
  return [...rows].sort((a, b) => {
    const x = value(a);
    const y = value(b);
    if (x !== y) {
      if (x === null) return 1;
      if (y === null) return -1;
      return (x < y ? -1 : 1) * dir;
    }
    const spend = (b.spend ?? -1) - (a.spend ?? -1);
    return spend || a.displayName.localeCompare(b.displayName);
  });
}

export interface Totals {
  agents: number;
  harnesses: number;
  /** Sum of the rows' spend; null while the dashboard hasn't answered. */
  spend: number | null;
  /** Some of it is harness estimates (API list price): the strip says so. */
  spendHasEstimates: boolean;
  saved: number | null;
  /** The basis of the saved figures summed: one basis, or `mixed` when they differ (never shown as all measured). */
  savedBasis: string | null;
  /** Rows with something that needs the viewer (null while the needs reads load). */
  needing: number | null;
}

export function totals(rows: readonly HomeRow[]): Totals {
  const known = (k: "spend" | "saved") =>
    rows.some((r) => r[k] !== null) ? rows.reduce((n, r) => n + (r[k] ?? 0), 0) : null;
  return {
    agents: rows.filter((r) => !r.harness).length,
    harnesses: rows.filter((r) => r.harness).length,
    spend: rows.every((r) => r.spend !== null) && rows.length ? known("spend") : null,
    spendHasEstimates: rows.some((r) => r.harness && (r.spend ?? 0) > 0),
    saved: known("saved"),
    savedBasis: (() => {
      const bases = new Set(rows.flatMap((r) => (r.savedBasis ? [r.savedBasis] : [])));
      return bases.size > 1 ? "mixed" : ([...bases][0] ?? null);
    })(),
    needing: rows.every((r) => r.needs !== null)
      ? rows.filter((r) => (r.needs ?? 0) > 0).length
      : null,
  };
}

/**
 * Pending items per agent from fleet-level reads: the agent's own action row, the failed sessions in the checked sample
 * naming it (by raw name, only when one visible agent has it), and its pending requests.
 */
export function needsByAgent(i: {
  action: readonly string[];
  failedSessionAgents: readonly string[];
  idForName: (raw: string) => string | undefined;
  requestAgents: readonly (string | null | undefined)[];
}): Map<string, number> {
  const out = new Map<string, number>();
  const add = (id: string | null | undefined) => {
    if (id) out.set(id, (out.get(id) ?? 0) + 1);
  };
  for (const id of i.action) add(id);
  for (const raw of i.failedSessionAgents) add(i.idForName(raw));
  for (const id of i.requestAgents) add(id);
  return out;
}

// ─── PR2b: owner groups and sparklines ─────────────────────────────────────────────

export interface RowGroup {
  key: string;
  label: string;
  rows: HomeRow[];
  /** Sum of the group's spend; null while any of it is unknown. */
  spend: number | null;
}

/** The group every owner the viewer can't name falls into (members read no user list). */
const OTHER_OWNERS = "other-owners";
const MINE = "mine";

/**
 * Fleet's owner groups in display order, or null for Yours' one list: the viewer's own first, then each named owner by
 * name; owners the viewer can't name share one group, last. Rows keep their order inside a group.
 */
export function rowGroups(
  rows: readonly HomeRow[],
  grouped: boolean,
  ownerName: (id: string) => string | null,
): RowGroup[] | null {
  if (!grouped) return null;
  const groups = new Map<string, RowGroup>();
  for (const r of rows) {
    const name = r.mine ? copy.groups.you : ownerName(r.ownerId);
    const key = r.mine ? MINE : name ? `owner:${r.ownerId}` : OTHER_OWNERS;
    const g = groups.get(key) ?? { key, label: name ?? copy.groups.others, rows: [], spend: 0 };
    g.rows.push(r);
    g.spend = g.spend === null || r.spend === null ? null : g.spend + r.spend;
    groups.set(key, g);
  }
  const rank = (g: RowGroup) => (g.key === MINE ? 0 : g.key === OTHER_OWNERS ? 2 : 1);
  return [...groups.values()].sort((a, b) => rank(a) - rank(b) || a.label.localeCompare(b.label));
}

/**
 * A row's spend per bucket across the window, zero-filled by TokenOps' own timeline (`series.ts`: the series is
 * sparse, an omitted bucket is a real zero, bucket starts are floored to the UTC hour or day).
 */
export const sparkPoints = (
  series: { bucket: "hour" | "day"; points: SpendTimeseriesPoint[] },
  win: { start: Date; end: Date },
): number[] =>
  zeroFillTimeline(series.points, series.bucket, win.start, win.end).map((p) => p.spend);

/**
 * The budget period's month ("Oct"): the period that resets at `resetsAt` is the month before it (UTC). Empty for a
 * time that doesn't parse.
 */
export function budgetMonth(iso: string): string {
  const t = Date.parse(iso);
  return Number.isNaN(t)
    ? ""
    : new Date(t - 1).toLocaleString("en-US", { month: "short", timeZone: "UTC" });
}

/** A row's owner line: "You", the owner's name, or "Another user" when the viewer can't name them. */
export const ownerLine = (r: HomeRow, ownerName: (id: string) => string | null) =>
  r.mine ? copy.table.you : (ownerName(r.ownerId) ?? copy.table.anotherUser);
