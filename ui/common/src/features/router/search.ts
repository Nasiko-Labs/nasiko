import { z } from 'zod'

/**
 * URL state for the router page (plan §4.1): the open tab (Agents when absent) and the summary tiles' filter. A junk
 * value falls back to the default instead of throwing, so a stale link still opens the page.
 */
const ROUTER_SOURCES = ['attached', 'default', 'none'] as const
export type SourceFilter = (typeof ROUTER_SOURCES)[number]

export const ROUTER_TABS = ['agents', 'configs', 'providers'] as const
export type RouterTab = (typeof ROUTER_TABS)[number]

export const routerSearchSchema = z.object({
  tab: z.enum(['configs', 'providers']).optional().catch(undefined),
  source: z.enum(ROUTER_SOURCES).optional().catch(undefined),
})
export type RouterSearch = z.infer<typeof routerSearchSchema>
