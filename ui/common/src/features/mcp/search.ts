import { z } from 'zod'

// Here, not in logic.ts: the route's search schema loads with the shell, and logic.ts would come with it.
/** `?view=`: the legacy module nav's scopes. Ownership scopes are custom servers only. */
export const VIEWS = ['yours', 'shared', 'toolkits'] as const
export type View = (typeof VIEWS)[number]
export const TABS = ['all', 'available', 'connected'] as const
export type Tab = (typeof TABS)[number]

/** URL state for the MCP pages (plans/feat-mcp.md §2, §5). Junk values fall back, so a stale link still opens. */
export const catalogSearchSchema = z.object({
  /** Untrimmed while typing; matching trims. */
  q: z.string().max(200).optional().catch(undefined),
  view: z.enum(VIEWS).optional().catch(undefined),
  tab: z.enum(TABS).optional().catch(undefined),
})
export type CatalogSearch = z.infer<typeof catalogSearchSchema>

export type DetailTab = 'overview' | 'agents' | 'access' | 'logs' | 'settings'
/** Unknown tabs stay strings so the page can fall back to Overview with replace (as Agents does). */
export const detailSearchSchema = z.object({
  tab: z.string().trim().max(40).optional().catch(undefined),
  /** The agent whose access the Agents tab shows. */
  agent: z.string().uuid().optional().catch(undefined),
})
export type DetailSearch = z.infer<typeof detailSearchSchema>
