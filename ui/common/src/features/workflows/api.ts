/**
 * Workflow queries and mutations (plans/feat-workflows.md). Every read is under `['workflows', …]`. Lists are keyed by
 * sort, so a late answer for an old sort never overwrites the new one. Runs poll while they move.
 */
import { keepPreviousData, queryOptions, type QueryClient } from '@tanstack/react-query'
import type { ResolveBody } from '@/features/chat/api'
import type { HitlDto } from '@/features/chat/types'
import { apiData, apiFetch, withQuery } from '@/lib/api/client'
import { isExecActive } from './logic'
import type { DraftSort, WorkflowSort } from './search'
import { LIST_LIMIT, RUNS_LIMIT, RUN_POLL_MS } from './tuning'
import {
  executionListSchema,
  executionSchema,
  planSchema,
  runStartedSchema,
  workflowListSchema,
  workflowSchema,
  type Execution,
  type ExecutionRow,
  type GeneratedPlan,
  type RunStarted,
  type StepInput,
  type Workflow,
  type WorkflowRow,
} from './types'

const enc = encodeURIComponent
const W = (id: string) => `/api/maf/workflow/${enc(id)}`

export const workflowKeys = {
  all: ['workflows'] as const,
  lists: ['workflows', 'list'] as const,
  list: (mode: 'deployed' | 'drafts', sort: string) => ['workflows', 'list', mode, sort] as const,
  detail: (id: string) => ['workflows', 'detail', id] as const,
  runs: ['workflows', 'runs'] as const,
  runList: ['workflows', 'runs', 'list'] as const,
  run: (id: string) => ['workflows', 'runs', id] as const,
  /** The Run button's mutation (`run.ts`), read by the run page while it starts. */
  start: ['workflows', 'start'] as const,
}

const json = (method: string, body?: unknown): RequestInit => ({
  method,
  headers: { 'Content-Type': 'application/json' },
  body: body === undefined ? undefined : JSON.stringify(body),
})

const rows = async (path: string, signal: AbortSignal) =>
  (await apiData<{ data: WorkflowRow[] }>(path, { signal, schema: workflowListSchema })).data

/** Deployed (`GET /maf/workflows`, active rows only) or drafts (NAS-697; W-1). The first 100 either way. */
export const workflowListQuery = (
  p: { mode: 'deployed'; sort: WorkflowSort } | { mode: 'drafts'; sort: DraftSort },
) =>
  queryOptions({
    queryKey: workflowKeys.list(p.mode, p.sort),
    queryFn: ({ signal }) =>
      rows(
        withQuery(p.mode === 'deployed' ? '/api/maf/workflows' : '/api/maf/workflow/drafts', {
          limit: LIST_LIMIT,
          offset: 0,
          sort: p.sort,
        }),
        signal,
      ),
    placeholderData: keepPreviousData,
  })

export const workflowQuery = (id: string) =>
  queryOptions({
    queryKey: workflowKeys.detail(id),
    queryFn: ({ signal }) => apiData<Workflow>(W(id), { signal, schema: workflowSchema }),
  })

/** One run; polls while it is still moving (paused included). */
export const executionQuery = (id: string) =>
  queryOptions({
    queryKey: workflowKeys.run(id),
    queryFn: ({ signal }) =>
      apiData<Execution>(`/api/maf/execution/${enc(id)}`, { signal, schema: executionSchema }),
    refetchInterval: (q) => (isExecActive(q.state.data?.status) ? RUN_POLL_MS : false),
  })

/** The newest 50 runs of every workflow, deleted ones included; polls while any of them moves. */
export const executionsQuery = queryOptions({
  queryKey: workflowKeys.runList,
  queryFn: async ({ signal }) =>
    (
      await apiData<{ data: ExecutionRow[] }>(
        withQuery('/api/maf/executions', { limit: RUNS_LIMIT, offset: 0 }),
        { signal, schema: executionListSchema },
      )
    ).data,
  refetchInterval: (q) => (q.state.data?.some((r) => isExecActive(r.status)) ? RUN_POLL_MS : false),
})

export const createWorkflow = (body: { name: string; description?: string; steps: StepInput[] }) =>
  apiData<Workflow>('/api/maf/workflows', { ...json('POST', body), schema: workflowSchema })

/** Absent = keep; `description: null` clears; `steps: []` is a 400. */
export const updateWorkflow = (
  id: string,
  body: { name?: string; description?: string | null; steps?: StepInput[] },
) => apiData<Workflow>(W(id), { ...json('PUT', body), schema: workflowSchema })

/** NAS-697: a bare-sentence draft, named after the instruction's first 60 characters (a 404 = the draft is gone). */
export const saveDraft = (body: { instruction: string; draft_id?: string }) =>
  apiData<Workflow>('/api/maf/workflow/draft', { ...json('POST', body), schema: workflowSchema })

/** NAS-697: a status flip only (same id, no re-routing). */
export const promoteWorkflow = (id: string) =>
  apiData<Workflow>(`${W(id)}/promote`, { ...json('POST'), schema: workflowSchema })

/** A soft delete; answers `data: null`, so not `apiData`. */
export const deleteWorkflow = (id: string) => apiFetch<unknown>(W(id), { method: 'DELETE' })

/** 202: queues a run of the stored row. */
export const runWorkflow = (id: string) =>
  apiData<RunStarted>(`${W(id)}/run`, { ...json('POST'), schema: runStartedSchema })

/** The planner's proposal (never stored); callers branch on 503 / 400 / 422 (`planFailure`). */
export const generateWorkflow = (description: string) =>
  apiData<GeneratedPlan>('/api/maf/generate', {
    ...json('POST', { description }),
    schema: planSchema,
    // An LLM call: longer than the default deadline.
    timeout: 120_000,
  })

/** A paused step's request (`router/hitl.rs`): answered in place, and the resume continues the run server-side. */
export const resolveHitl = (id: string, body: ResolveBody) =>
  apiFetch<HitlDto>(`/api/hitl/${enc(id)}/resolve`, json('POST', body))
export const cancelHitl = (id: string) =>
  apiFetch<HitlDto>(`/api/hitl/${enc(id)}/cancel`, json('POST'))

/** Gone from every cached list, without a refetch. */
export function dropFromLists(client: QueryClient, id: string) {
  client.setQueriesData<WorkflowRow[]>({ queryKey: workflowKeys.lists }, (list) =>
    list?.filter((r) => r.id !== id),
  )
}
