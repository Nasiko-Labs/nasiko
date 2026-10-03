import { z } from 'zod'
import { flag, text } from '@/lib/search'

/**
 * URL state for the agent pages (plan §7). Junk values fall back instead of throwing, so a
 * stale link still opens the page.
 */
export const catalogSearchSchema = z.object({
  /** Untrimmed while typing (trimming here would eat the space between words); matching trims. */
  q: z.string().max(200).optional().catch(undefined),
  tag: text.optional().catch(undefined),
  harnesses: flag.optional().catch(undefined),
  yours: flag.optional().catch(undefined),
  /** The Overview's Fleet health count links here (overview design 15A, 16B); computed by `useFleetHealth` (eng R1). */
  health: z.enum(['healthy', 'watch', 'action', 'unknown']).optional().catch(undefined),
})
export type CatalogSearch = z.infer<typeof catalogSearchSchema>

export const MINE_TABS = [
  'all',
  'running',
  'deploying',
  'attention',
  'stopped',
  'harnesses',
] as const
export const mineSearchSchema = z.object({
  tab: z.enum(MINE_TABS).optional().catch(undefined),
  harnesses: flag.optional().catch(undefined),
  /** Superusers only: whose agents to list (a UUID). */
  owner: z.string().uuid().optional().catch(undefined),
})
export type MineSearch = z.infer<typeof mineSearchSchema>

export const DETAIL_TABS = [
  'overview',
  'activity',
  'versions',
  'builds',
  'mcp',
  'access',
  'settings',
] as const
export type DetailTab = (typeof DETAIL_TABS)[number]
/** Unknown tab values are kept as strings so the page can fall back to Overview with replace-history. */
export const detailSearchSchema = z.object({
  tab: z.string().trim().max(40).optional().catch(undefined),
})

/**
 * The delete result, carried to Your agents in router history state, not the URL: a crafted
 * link can't show a fake "Deleted …" notice, and long runtime-error lists aren't cut.
 */
export interface DeletedNote {
  name: string
  errors: string[]
  stopped: number
}

declare module '@tanstack/react-router' {
  interface HistoryState {
    agentDeleted?: DeletedNote
  }
}
