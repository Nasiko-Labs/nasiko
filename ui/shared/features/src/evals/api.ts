/** Evals reads and writes (docs/superpowers/specs/2026-11-23-evals-design.md). Every key starts with ['evals']. */
import { useMutation, useQuery } from '@tanstack/react-query'
import { apiData } from '@nasiko/data-access/client'
import {
  benchmarksSchema,
  datasetSchema,
  runSchema,
  sessionsSchema,
  type EvalBenchmark,
  type EvalDataset,
  type EvalResult,
  type EvalSession,
} from './types'

export const evalsKeys = {
  sessions: (harness: string) => ['evals', 'sessions', harness] as const,
  benchmarks: ['evals', 'benchmarks'] as const,
}

const post = (body: unknown): RequestInit => ({
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
})

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

/** The pre-defined SWE benchmarks, read only when the harness has no sessions to build from. */
export function useEvalBenchmarks(enabled: boolean) {
  return useQuery({
    queryKey: evalsKeys.benchmarks,
    queryFn: ({ signal }) =>
      apiData<{ benchmarks: EvalBenchmark[] }>('/api/evals/benchmarks', {
        signal,
        schema: benchmarksSchema,
      }),
    select: (d) => d.benchmarks,
    enabled,
  })
}

/** A dataset from the ticked sessions, or from the picked benchmarks (one of the two lists, never both). */
export interface DatasetSource {
  harness: string
  session_ids?: string[]
  benchmark_ids?: string[]
}

export function useGenerateDataset() {
  return useMutation({
    mutationFn: (v: DatasetSource) =>
      apiData<EvalDataset>('/api/evals/datasets', { ...post(v), schema: datasetSchema }),
  })
}

export function useRunEvals() {
  return useMutation({
    mutationFn: ({
      datasetId,
      ...body
    }: {
      datasetId: string
      task_ids: string[]
      model_ids: string[]
    }) =>
      apiData<{ results: EvalResult[] }>(
        `/api/evals/datasets/${encodeURIComponent(datasetId)}/runs`,
        { ...post(body), schema: runSchema },
      ),
  })
}
