import { z } from 'zod/mini'
import { flag, opt, text } from '@nasiko/ui/lib/search'

/**
 * URL state for the agent pages (plan §7). Junk values fall back instead of throwing, so a
 * stale link still opens the page.
 */
export const catalogSearchSchema = z.object({
  /** Untrimmed while typing (trimming here would eat the space between words); matching trims. */
  q: opt(z.string().check(z.maxLength(200))),
  tag: opt(text),
  harnesses: opt(flag),
  yours: opt(flag),
  /** The home's rating badges link here (plans/feat-agent-centric.md PR2a); computed by `useFleetHealth` (eng R1). */
  health: opt(z.enum(['healthy', 'watch', 'action', 'unknown'])),
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
  tab: opt(z.enum(MINE_TABS)),
  harnesses: opt(flag),
  /** Superusers only: whose agents to list (a UUID). */
  owner: opt(z.uuid()),
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
  // Today's Overview facts, since the hub took the Overview over (plans/feat-agent-centric.md PR1c).
  'details',
] as const
export type DetailTab = (typeof DETAIL_TABS)[number]
// The detail route's schema is the hub's (`features/hub/search.ts`): `tab` plus the hub's window, slice and marker.

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
