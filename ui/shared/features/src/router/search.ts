import { z } from 'zod/mini'
import { opt } from '@nasiko/ui/lib/search'

/**
 * URL state for the router page (plan §4.1): the open tab (Agents when absent) and the summary tiles' filter. A junk
 * value falls back to the default instead of throwing, so a stale link still opens the page.
 */
const ROUTER_SOURCES = ['attached', 'default', 'none'] as const
export type SourceFilter = (typeof ROUTER_SOURCES)[number]

/** Budgets shows only on a server with budgets (plans/feat-budget-burn.md F11). */
export const ROUTER_TABS = ['agents', 'configs', 'budgets', 'providers'] as const
export type RouterTab = (typeof ROUTER_TABS)[number]

export const routerSearchSchema = z.object({
  tab: opt(z.enum(['configs', 'budgets', 'providers'])),
  source: opt(z.enum(ROUTER_SOURCES)),
})
export type RouterSearch = z.infer<typeof routerSearchSchema>
