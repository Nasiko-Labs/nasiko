/**
 * The Overview's search params, all shareable view state, all replaced, never pushed:
 * - `range`: the window the home reads its money for (7, 30 or 90 days), and what the village's volumes cover.
 * - `view`: what the page shows under its header: the cards (absent: the viewer's last pick, else the cards), or the
 *   estate's canvas as the village or the map; `list` (the List view, gone: its list is the Map's System card) still
 *   parses, and opens the map.
 * - `scope`: Org (admins) or Mine. Members only ever see Mine, so their links leave it out.
 * - `sel`: what the canvas has selected (`node:<id>`, `problem:<id>`, `flow:<id>`, `chain:<id>`, `opp:<id>`,
 *   `panel:<card>[:<kind>]`, `tile:<id>:<day>`, `sess:<id>:<index>`, `place:<id>`), so a link opens the same panel.
 * - `at`: a past day in the History slider (absent: live).
 * - `team`, `owner`, `kind`, `tier`: the canvas's filters.
 * Needs you keeps its own windows (its rules are defined on them). Loaded with the shell (the home's schema uses it), so
 * no query code here.
 */
import { z } from 'zod/mini'
import { isoDate, opt } from '@nasiko/ui/lib/search'

export const RANGES = ['7d', '30d', '90d'] as const
export type Range = (typeof RANGES)[number]

const ref = z.string().check(z.minLength(1), z.maxLength(200))

/** Optional, so the landing page's own URL stays a bare `/` (sign-in redirects, the sidebar link); 30 days when unset. */
export const overviewSearchSchema = z.object({
  range: opt(z.enum(RANGES)),
  view: opt(z.enum(['village', 'map', 'list'])),
  scope: opt(z.enum(['org', 'mine'])),
  sel: opt(ref),
  at: opt(isoDate),
  team: opt(ref),
  owner: opt(ref),
  kind: opt(z.enum(['errors', 'slow', 'cost'])),
  tier: opt(z.enum(['broken', 'worse'])),
  // Mock-only (read by the mock from the URL): kept as written, so `?mock=village-unlimited` survives a selection.
  mock: opt(ref),
})
export type OverviewSearch = z.infer<typeof overviewSearchSchema>

/** 30 days when `?range=` is unset (the home's window, `home/search.ts`, keeps the Overview's key). */
export const DEFAULT_RANGE: Range = '30d'

export const RANGE_DAYS: Record<Range, number> = { '7d': 7, '30d': 30, '90d': 90 }
