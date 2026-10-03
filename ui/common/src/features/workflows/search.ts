import { z } from 'zod'

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
const q = z.string().max(200).optional().catch(undefined)

export const deployedSearchSchema = z.object({
  q,
  sort: z.enum(WORKFLOW_SORTS).optional().catch(undefined),
})
export type DeployedSearch = z.infer<typeof deployedSearchSchema>

export const draftsSearchSchema = z.object({
  q,
  sort: z.enum(DRAFT_SORTS).optional().catch(undefined),
})
export type DraftsSearch = z.infer<typeof draftsSearchSchema>

export const runsSearchSchema = z.object({
  q,
  status: z.enum(RUN_STATUS_FILTERS).optional().catch(undefined),
  age: z.enum(RUN_AGE_FILTERS).optional().catch(undefined),
  /** The run to open and scroll to (an execution id): where Run lands (`run.ts`). */
  run: z.string().uuid().optional().catch(undefined),
})
export type RunsSearch = z.infer<typeof runsSearchSchema>

export const workflowSearchSchema = z.object({
  /** The run shown instead of the review (an execution id). */
  run: z.string().uuid().optional().catch(undefined),
})

declare module '@tanstack/react-router' {
  interface HistoryState {
    /** This entry is a run the workflow page pushed itself: its Back pops it (a deep link replaces instead). */
    workflowRunPushed?: boolean
  }
}
