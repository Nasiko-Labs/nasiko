/**
 * /optimization search keys (plans/feat-optimization-page.md R7A, C5): TokenOps' window keys, plus `slice`, the chart
 * bar a click picked (ISO 8601 `start/end`; `lead.ts` `parseSlice` ignores one outside the window; not `from`/`to`:
 * those are the custom window's dates), and By agent's sort and Show all.
 */
import { z } from 'zod/mini'
import { sharedSearchSchema } from '@nasiko/app-core/shell/context'
import { fallback, flag, opt, text } from '@nasiko/ui/lib/search'

/**
 * By agent's sort keys. Here, not in breakdown.ts: the route's schema loads with the app shell, so this module imports
 * nothing of the page (review: the shell budget).
 */
const AGENT_SORTS = ['without', 'sent', 'saved', 'requests', 'name'] as const
export type AgentSort = (typeof AGENT_SORTS)[number]

export const optimizationSearchSchema = z.extend(sharedSearchSchema, {
  slice: opt(text),
  /** By agent's column (R7B): history volume first; a header click replaces history. */
  sort: fallback(z.enum(AGENT_SORTS), 'without'),
  /** By agent's "Show all N". */
  all: opt(flag),
  // Mock-only (read by the mock bootstrap from the URL): kept as written, so `?mock=optimization-classic` survives
  // the router's search validation instead of being dropped as an unknown TokenOps variant.
  mock: opt(text),
})

export type OptimizationSearch = z.infer<typeof optimizationSearchSchema>
