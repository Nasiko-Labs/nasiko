/** Code review evals reads and writes. Every key starts with ['evals', 'code-review']. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { apiData } from '@nasiko/data-access/client'
import {
  reviewRunSchema,
  reviewTaskSchema,
  reviewTasksSchema,
  runCreatedSchema,
  type ReferenceFinding,
  type ReviewRun,
  type ReviewTask,
  type RunCreated,
} from './types'

/** How often a running review is read again. */
export const CODE_REVIEW_POLL_MS = 3000

export const codeReviewKeys = {
  all: ['evals', 'code-review'] as const,
  tasks: ['evals', 'code-review', 'tasks'] as const,
  task: (id: string) => ['evals', 'code-review', 'task', id] as const,
  run: (id: string) => ['evals', 'code-review', 'run', id] as const,
}

const post = (body: unknown): RequestInit => ({
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
})

export function useReviewTasks() {
  return useQuery({
    queryKey: codeReviewKeys.tasks,
    queryFn: ({ signal }) =>
      apiData<ReviewTask[]>('/api/evals/code-review/tasks', { signal, schema: reviewTasksSchema }),
  })
}

export function useReviewTask(id: string | undefined) {
  return useQuery({
    queryKey: codeReviewKeys.task(id ?? ''),
    enabled: !!id,
    queryFn: ({ signal }) =>
      apiData<ReviewTask>(`/api/evals/code-review/tasks/${encodeURIComponent(id ?? '')}`, {
        signal,
        schema: reviewTaskSchema,
      }),
  })
}

/** Imports one pull request at a fixed base/head commit as a new (or refreshed) task. */
export function useImportPullRequest() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: { owner: string; repo: string; pr_number: number }) =>
      apiData<ReviewTask>('/api/evals/code-review/tasks', {
        ...post(body),
        schema: reviewTaskSchema,
      }),
    onSuccess: (task) => {
      qc.setQueryData(codeReviewKeys.task(task.id), task)
      void qc.invalidateQueries({ queryKey: codeReviewKeys.tasks })
    },
  })
}

/** Adds human-validated ground truth to a task; once non-empty, its runs stop being previews. */
export function useAddReferenceFindings(taskId: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (findings: ReferenceFinding[]) =>
      apiData<ReviewTask>(`/api/evals/code-review/tasks/${encodeURIComponent(taskId)}/reference-findings`, {
        ...post({ findings }),
        schema: reviewTaskSchema,
      }),
    onSuccess: (task) => qc.setQueryData(codeReviewKeys.task(task.id), task),
  })
}

export function useCreateReviewRun(taskId: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (model_ids: string[]) =>
      apiData<RunCreated>(`/api/evals/code-review/tasks/${encodeURIComponent(taskId)}/runs`, {
        ...post({ model_ids }),
        schema: runCreatedSchema,
      }),
    onSuccess: (run) => void qc.invalidateQueries({ queryKey: codeReviewKeys.run(run.id) }),
  })
}

const runBusy = (run: ReviewRun | undefined) => run?.status === 'running'

export function useReviewRun(id: string | undefined) {
  return useQuery({
    queryKey: codeReviewKeys.run(id ?? ''),
    enabled: !!id,
    queryFn: ({ signal }) =>
      apiData<ReviewRun>(`/api/evals/code-review/runs/${encodeURIComponent(id ?? '')}`, {
        signal,
        schema: reviewRunSchema,
      }),
    refetchInterval: (q) => (runBusy(q.state.data) ? CODE_REVIEW_POLL_MS : false),
  })
}
