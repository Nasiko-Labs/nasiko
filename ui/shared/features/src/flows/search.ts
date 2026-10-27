/**
 * The Flows list's URL state (plans/feat-flows.md §3, F8): the shared window keys (7 days by default, as Sessions),
 * the search text and the Kind and Status filters. Junk falls back, so a stale link still opens. Here, not in a logic
 * file: the route's schema loads with the shell.
 */
import { z } from 'zod/mini'
import { sharedSearchSchema } from '@nasiko/app-core/shell/context'
import { fallback, opt, PRESETS, text } from '@nasiko/ui/lib/search'

export const KIND_FILTERS = ['all', 'orchestrated', 'direct', 'workflow'] as const
export type KindFilter = (typeof KIND_FILTERS)[number]
export const STATUS_FILTERS = ['all', 'running', 'paused', 'completed', 'failed'] as const
export type StatusFilter = (typeof STATUS_FILTERS)[number]

export const flowsSearchSchema = z.extend(sharedSearchSchema, {
  preset: fallback(z.enum(PRESETS), '7d'),
  q: opt(text),
  kind: fallback(z.enum(KIND_FILTERS), 'all'),
  status: fallback(z.enum(STATUS_FILTERS), 'all'),
  // Mock-only (read by the mock bootstrap from the URL): kept as written, so `?mock=flows-empty` and `flows-absent`
  // (and the trace variants) survive the route's search validation, as on /optimization.
  mock: opt(text),
})
export type FlowsSearch = z.infer<typeof flowsSearchSchema>

/** One flow's URL state (F25): the selected call (`Call.key`: `step:<id>` or `span:<hex>`), replaced, not pushed. */
export const flowSearchSchema = z.object({
  step: opt(z.string().check(z.trim(), z.minLength(1), z.maxLength(200))),
  // Mock-only, kept as written (`?mock=trace-503`, `trace-500` on a flow page).
  mock: opt(text),
})
export type FlowSearch = z.infer<typeof flowSearchSchema>
