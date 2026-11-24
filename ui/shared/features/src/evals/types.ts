/**
 * The proposed evals routes (docs/superpowers/specs/2026-11-23-evals-design.md). No server has them yet: the mock in
 * `shared/mocks/src/evals.ts` answers them, and a bare 404 means the server predates them. Every answer is `{data}`;
 * errors are `{error, code}`. The schemas check only the fields the page reads.
 */
import { z } from 'zod'

export interface EvalSession {
  session_id: string
  /** The session's first prompt, as the server shortens it. */
  title: string
  started_at: string
  turns: number
  cost_usd: number
  models: string[]
}

/** A pre-defined SWE benchmark: what a dataset starts from when the harness has no sessions. */
export interface EvalBenchmark {
  id: string
  name: string
  description: string
  /** How many tasks it brings to a dataset. */
  tasks: number
}

export interface EvalTask {
  id: string
  title: string
  /** Where the task came from: one of the ticked sessions, or one of the picked benchmarks. */
  session_id?: string
  benchmark_id?: string
}

export interface EvalModel {
  id: string
  name: string
  provider: string
  /** The ticked sessions used this model (never for a benchmark dataset). */
  used: boolean
}

export interface EvalDataset {
  id: string
  tasks: EvalTask[]
  models: EvalModel[]
}

export interface EvalResult {
  task_id: string
  model_id: string
  /** 0–100. */
  score: number
  cost_usd: number
  latency_ms: number
}

export const sessionsSchema = z.looseObject({
  sessions: z.array(
    z.looseObject({
      session_id: z.string(),
      title: z.string(),
      started_at: z.string(),
      turns: z.number(),
      cost_usd: z.number(),
      models: z.array(z.string()),
    }),
  ),
})

export const benchmarksSchema = z.looseObject({
  benchmarks: z.array(
    z.looseObject({
      id: z.string(),
      name: z.string(),
      description: z.string(),
      tasks: z.number(),
    }),
  ),
})

export const datasetSchema = z.looseObject({
  id: z.string(),
  tasks: z.array(z.looseObject({ id: z.string(), title: z.string() })),
  models: z.array(
    z.looseObject({ id: z.string(), name: z.string(), provider: z.string(), used: z.boolean() }),
  ),
})

export const runSchema = z.looseObject({
  results: z.array(
    z.looseObject({
      task_id: z.string(),
      model_id: z.string(),
      score: z.number(),
      cost_usd: z.number(),
      latency_ms: z.number(),
    }),
  ),
})
