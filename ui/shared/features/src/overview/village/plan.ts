/**
 * The free plan's limits (plans/feat-village-overview.md V17, V51; playground dec-078, dec-079): how many of each
 * thing a free workspace has. Two LLM routers, not four (Ans, 2026-10-16: "only 2 LLM Routers are enough"). Every
 * district has this many open plots; the village draws the first ones (broken things first, so a cap never hides a
 * fire) and says how many more exist on the district's sign. `?mock=village-unlimited` opens every plot a district
 * has. Pure.
 */
export const FREE_PLAN = {
  team: 4,
  mine: 2,
  bay: 10,
  harness: 1,
  tool: 10,
  mcp: 2,
  gate: 2,
} as const
export type PlanKind = keyof typeof FREE_PLAN

/** Plots of a kind that are open: the plan's limit, or every plot the district has when the plan is lifted. */
export function openPlots(kind: string, capacity: number, unlimited: boolean): number {
  const limit = (FREE_PLAN as Record<string, number>)[kind]
  return unlimited || limit === undefined ? capacity : Math.min(capacity, limit)
}
