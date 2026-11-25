/** Evals reads and writes (docs/superpowers/specs/2026-11-23-evals-design.md). Every key starts with ['evals']. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { apiData } from '@nasiko/data-access/client'
import { isEndpointAbsent } from '@nasiko/data-access/detect'
import {
  benchmarksSchema,
  datasetSchema,
  runSchema,
  runsSchema,
  sessionsSchema,
  setupSchema,
  type EvalBenchmark,
  type EvalDataset,
  type EvalRun,
  type EvalRunSummary,
  type EvalSession,
  type EvalSetup,
} from './types'

/** How often a dataset being built, or a run going, is read again. */
export const EVALS_POLL_MS = 3000

export const evalsKeys = {
  all: ['evals'] as const,
  setup: ['evals', 'harnesses'] as const,
  sessions: (harness: string) => ['evals', 'sessions', harness] as const,
  benchmarks: ['evals', 'benchmarks'] as const,
  dataset: (id: string) => ['evals', 'dataset', id] as const,
  run: (id: string) => ['evals', 'run', id] as const,
  runs: ['evals', 'runs'] as const,
}

const post = (body: unknown): RequestInit => ({
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
})

/** What building evals needs: GitHub, chat history, a server that runs them. */
export function useEvalSetup() {
  return useQuery({
    queryKey: evalsKeys.setup,
    queryFn: ({ signal }) =>
      apiData<EvalSetup>('/api/evals/harnesses', { signal, schema: setupSchema }),
  })
}

/** The harness's sessions; a coded 404 `harness_not_connected` when it isn't connected. */
export function useEvalSessions(harness: string) {
  return useQuery({
    queryKey: evalsKeys.sessions(harness),
    queryFn: ({ signal }) =>
      apiData<{ sessions: EvalSession[] }>(
        `/api/evals/harnesses/${encodeURIComponent(harness)}/sessions`,
        { signal, schema: sessionsSchema },
      ),
    select: (d) => d.sessions,
  })
}

/**
 * The pre-defined SWE benchmarks, read only when the harness has no sessions to build from. A server without the route
 * offers none.
 */
export function useEvalBenchmarks(enabled: boolean) {
  return useQuery({
    queryKey: evalsKeys.benchmarks,
    queryFn: async ({ signal }) => {
      try {
        return await apiData<{ benchmarks: EvalBenchmark[] }>('/api/evals/benchmarks', {
          signal,
          schema: benchmarksSchema,
        })
      } catch (e) {
        if (isEndpointAbsent(e)) return { benchmarks: [] }
        throw e
      }
    },
    select: (d) => d.benchmarks,
    enabled,
  })
}

const datasetBusy = (d: EvalDataset | undefined) =>
  d?.status === 'harvesting' || d?.status === 'preparing'

/** A dataset, read again while it is being built. */
export function useEvalDataset(id: string | undefined) {
  return useQuery({
    queryKey: evalsKeys.dataset(id ?? ''),
    enabled: !!id,
    queryFn: ({ signal }) =>
      apiData<EvalDataset>(`/api/evals/datasets/${encodeURIComponent(id ?? '')}`, {
        signal,
        schema: datasetSchema,
      }),
    refetchInterval: (q) => (datasetBusy(q.state.data) ? EVALS_POLL_MS : false),
  })
}

/** A dataset from the ticked sessions, or from the picked benchmarks (one of the two lists, never both). */
export interface DatasetSource {
  harness: string
  session_ids?: string[]
  benchmark_ids?: string[]
}

export function useGenerateDataset() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (v: DatasetSource) =>
      apiData<EvalDataset>('/api/evals/datasets', { ...post(v), schema: datasetSchema }),
    onSuccess: (d) => qc.setQueryData(evalsKeys.dataset(d.id), d),
  })
}

const runBusy = (r: EvalRun | undefined) =>
  r?.status === 'queued' || r?.status === 'running' || r?.status === 'finalizing'

/** A run, read again until it ends. */
export function useEvalRun(id: string | undefined) {
  return useQuery({
    queryKey: evalsKeys.run(id ?? ''),
    enabled: !!id,
    queryFn: ({ signal }) =>
      apiData<EvalRun>(`/api/evals/runs/${encodeURIComponent(id ?? '')}`, {
        signal,
        schema: runSchema,
      }),
    refetchInterval: (q) => (runBusy(q.state.data) ? EVALS_POLL_MS : false),
  })
}

export function useRunEvals() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({
      datasetId,
      ...body
    }: {
      datasetId: string
      task_ids: string[]
      model_ids: string[]
    }) =>
      apiData<EvalRun>(`/api/evals/datasets/${encodeURIComponent(datasetId)}/runs`, {
        ...post(body),
        schema: runSchema,
      }),
    onSuccess: (r) => {
      qc.setQueryData(evalsKeys.run(r.id), r)
      void qc.invalidateQueries({ queryKey: evalsKeys.runs })
    },
  })
}

/** The viewer's saved runs, newest first. */
export function useEvalRuns() {
  return useQuery({
    queryKey: evalsKeys.runs,
    queryFn: ({ signal }) =>
      apiData<EvalRunSummary[]>('/api/evals/runs', { signal, schema: runsSchema }),
  })
}
