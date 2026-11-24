/** Evals reads and writes (docs/superpowers/specs/2026-11-23-evals-design.md). Every key starts with ['evals']. */
import { useMutation, useQuery } from '@tanstack/react-query'
import { apiData } from '@nasiko/data-access/client'
import {
  datasetSchema,
  harnessesSchema,
  runSchema,
  sessionsSchema,
  type EvalDataset,
  type EvalHarness,
  type EvalResult,
  type EvalSession,
} from './types'

export const evalsKeys = {
  harnesses: ['evals', 'harnesses'] as const,
  sessions: (harness: string) => ['evals', 'sessions', harness] as const,
}

const post = (body: unknown): RequestInit => ({
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
})

/** The viewer's connected coding harnesses; empty when none is connected. */
export function useEvalHarnesses() {
  return useQuery({
    queryKey: evalsKeys.harnesses,
    queryFn: ({ signal }) =>
      apiData<{ harnesses: EvalHarness[] }>('/api/evals/harnesses', {
        signal,
        schema: harnessesSchema,
      }),
    select: (d) => d.harnesses,
  })
}

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

export function useGenerateDataset() {
  return useMutation({
    mutationFn: (v: { harness: string; session_ids: string[] }) =>
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
