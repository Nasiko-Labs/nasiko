import { z } from 'zod/mini'
import { opt } from '@nasiko/ui/lib/search'

// Here, not in logic.ts: the route's search schema loads with the shell, and logic.ts would come with it.
export const RUN_STATUS_FILTERS = ['all', 'attention', 'running', 'success', 'failed'] as const
export type RunStatusFilter = (typeof RUN_STATUS_FILTERS)[number]
export const RUN_AGE_FILTERS = ['any', '1d', '7d', '30d'] as const
export type RunAgeFilter = (typeof RUN_AGE_FILTERS)[number]

// NAS-697 `WorkflowSort` / `DraftSort` (their `order_by`); `main` ignores `sort` (W-3).
export const WORKFLOW_SORTS = [
  'recent',
  'success_rate',
  'token_usage',
  'execution_count',
  'health',
] as const
export type WorkflowSort = (typeof WORKFLOW_SORTS)[number]
export const DRAFT_SORTS = ['all', 'last_updated', 'token_usage'] as const
export type DraftSort = (typeof DRAFT_SORTS)[number]

/** URL state for the workflow pages (plans/feat-workflows.md §1). Junk values fall back, so a stale link still opens. */
const q = opt(z.string().check(z.maxLength(200)))

export const deployedSearchSchema = z.object({
  q,
  sort: opt(z.enum(WORKFLOW_SORTS)),
})
export type DeployedSearch = z.infer<typeof deployedSearchSchema>

export const draftsSearchSchema = z.object({
  q,
  sort: opt(z.enum(DRAFT_SORTS)),
})
export type DraftsSearch = z.infer<typeof draftsSearchSchema>

export const runsSearchSchema = z.object({
  q,
  status: opt(z.enum(RUN_STATUS_FILTERS)),
  age: opt(z.enum(RUN_AGE_FILTERS)),
  /** The run to open and scroll to (an execution id): where Run lands (`run.ts`). */
  run: opt(z.uuid()),
})
export type RunsSearch = z.infer<typeof runsSearchSchema>

export const workflowSearchSchema = z.object({
  /** The run shown instead of the review (an execution id). */
  run: opt(z.uuid()),
})

declare module '@tanstack/react-router' {
  interface HistoryState {
    /** This entry is a run the workflow page pushed itself: its Back pops it (a deep link replaces instead). */
    workflowRunPushed?: boolean
  }
}
