/**
 * The Overview's one search param: the range its KPI tiles, Spend and Harnesses cover (7, 30 or 90 days). Needs you,
 * Fleet health and Recent sessions keep their own windows (their rules are defined on them). Replaced, not pushed. Only
 * the schema lives here: the route's `validateSearch` is in the shell chunk, so the window helpers stay in `api.ts`.
 */
import { z } from 'zod'

export const RANGES = ['7d', '30d', '90d'] as const
export type Range = (typeof RANGES)[number]

/** Optional, so the landing page's own URL stays a bare `/` (sign-in redirects, the sidebar link); 30 days when unset. */
export const overviewSearchSchema = z.object({
  range: z.enum(RANGES).optional().catch(undefined),
})
export const DEFAULT_RANGE: Range = '30d'
export type OverviewSearch = z.infer<typeof overviewSearchSchema>

export const RANGE_DAYS: Record<Range, number> = { '7d': 7, '30d': 30, '90d': 90 }
