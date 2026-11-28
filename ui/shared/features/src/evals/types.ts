/**
 * The evals routes (`/api/evals/*`, `oss/server/src/evals/api.rs`; docs/superpowers/specs/2026-11-23-evals-design.md).
 * The mock in `shared/mocks/src/evals.ts` answers them too, and a bare 404 means the server predates them. Every
 * answer is `{data}`; errors are `{error, code}`. The schemas check only the fields the page reads.
 *
 * Building a dataset and running one take minutes to hours: both answer at once with the new object, and the page
 * polls it until its status is final.
 */
import { z } from 'zod'

export interface EvalHarness {
  id: string
  sessions: number
  last_active_at: string | null
  /** Evals can be built from its sessions (Claude Code only, for now). */
  supported: boolean
}

export interface EvalSetup {
  harnesses: EvalHarness[]
  /** The viewer's GitHub account is connected: sessions' repositories are read through it. */
  github_connected: boolean
  /** The server keeps coding sessions' chat history, which tasks are built from. */
  chat_history: boolean
  /** The server builds datasets and runs evals. */
  runner_available: boolean
}

/** Why a session can't become a task. */
export type Ineligible =
  | 'no_chat_history'
  | 'no_edits'
  | 'no_repo'
  | 'not_github'
  | 'github_not_connected'
  | 'repo_not_accessible'
  | 'unsupported_harness'

export interface EvalSession {
  session_id: string
  /** The session's first prompt, as the server shortens it. */
  title: string
  started_at: string
  turns: number
  cost_usd: number
  models: string[]
  /** `owner/name`, when the session's repository is known. */
  repo: string | null
  eligible: boolean
  ineligible_reason: Ineligible | null
}

export type DatasetStatus = 'harvesting' | 'preparing' | 'ready' | 'failed'
export type TaskStatus = 'candidate' | 'reproducible' | 'verified' | 'rejected'

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
  /** Where the task came from: one of the ticked sessions, or one of the picked benchmarks. */
  session_id?: string | null | undefined
  benchmark_id?: string | null | undefined
  repo: string
  title: string
  /** Only `verified` tasks can run. */
  status: TaskStatus
  /** The next step: rewrite, build, screen, review (a person), done. */
  stage: string
  reject_reason: string | null
  /** Whether a person may approve or reject it: the gate found it reproducible, whatever the reviewers decided. */
  reviewable?: boolean | undefined
  /** What the original session did: the baseline every model is compared with. None for a benchmark's task. */
  original?:
    { model: string; cost_usd: number | null; latency_ms: number | null } | null | undefined
}

export interface EvalModel {
  /** One of the viewer's LLM configs. */
  id: string
  name: string
  provider: string
  model: string | null
  /** The sessions used this model: ticked by default, the fair comparison. */
  used: boolean
}

export interface EvalDataset {
  id: string
  status: DatasetStatus
  progress: { done: number; total: number }
  /** What happened to each picked session (`task`, `no_match`, `no_repo`, …). */
  outcomes: Record<string, string>
  error: string | null
  tasks: EvalTask[]
  models: EvalModel[]
}

export type RunStatus = 'queued' | 'running' | 'finalizing' | 'done' | 'failed' | 'cancelled'

/** One attempt: a cell of the comparison. Figures are null until the attempt (or the run, for cost) ends. */
export interface EvalResult {
  /** 1 for the first attempt; the table compares first attempts. */
  attempt?: number
  task_id: string
  model_id: string
  status: 'pending' | 'running' | 'done' | 'failed' | 'cancelled'
  /** 0–100: the share of the task's failing tests fixed, 0 if it broke others. */
  score: number | null
  resolved: boolean | null
  cost_usd: number | null
  latency_ms: number | null
  /** Why it was not resolved. */
  failure: string | null
  /** The model also rewrote or screened the task: its score is optimistic. */
  builder: boolean
}

export interface EvalRun {
  id: string
  dataset_id: string
  status: RunStatus
  models: { id: string; name: string }[]
  progress: { done: number; total: number }
  results: EvalResult[]
  error: string | null
  created_at: string
}

export interface EvalRunSummary {
  /** The dataset's harness. */
  harness: string
  id: string
  dataset_id: string
  status: RunStatus
  models: { id: string; name: string }[]
  created_at: string
}

const nullable = <T extends z.ZodType>(t: T) => t.nullable().optional()

const ineligibleSchema = z.enum([
  'no_chat_history',
  'no_edits',
  'no_repo',
  'not_github',
  'github_not_connected',
  'repo_not_accessible',
  'unsupported_harness',
])
const runStatusSchema = z.enum(['queued', 'running', 'finalizing', 'done', 'failed', 'cancelled'])

export const setupSchema = z.looseObject({
  harnesses: z.array(
    z.looseObject({
      id: z.string(),
      sessions: z.number(),
      last_active_at: z.string().nullable(),
      supported: z.boolean(),
    }),
  ),
  github_connected: z.boolean(),
  chat_history: z.boolean(),
  runner_available: z.boolean(),
})

export const sessionsSchema = z.looseObject({
  sessions: z.array(
    z.looseObject({
      session_id: z.string(),
      title: z.string(),
      started_at: z.string(),
      turns: z.number(),
      cost_usd: z.number(),
      models: z.array(z.string()),
      repo: nullable(z.string()),
      eligible: z.boolean(),
      ineligible_reason: nullable(ineligibleSchema),
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
  status: z.enum(['harvesting', 'preparing', 'ready', 'failed']),
  progress: z.looseObject({ done: z.number(), total: z.number() }),
  outcomes: z.record(z.string(), z.string()),
  error: nullable(z.string()),
  tasks: z.array(
    z.looseObject({
      id: z.string(),
      session_id: nullable(z.string()),
      benchmark_id: nullable(z.string()),
      repo: z.string(),
      title: z.string(),
      status: z.enum(['candidate', 'reproducible', 'verified', 'rejected']),
      stage: z.string(),
      reject_reason: nullable(z.string()),
      reviewable: z.boolean().optional(),
      original: nullable(
        z.looseObject({
          model: z.string(),
          cost_usd: nullable(z.number()),
          latency_ms: nullable(z.number()),
        }),
      ),
    }),
  ),
  models: z.array(
    z.looseObject({
      id: z.string(),
      name: z.string(),
      provider: z.string(),
      model: nullable(z.string()),
      used: z.boolean(),
    }),
  ),
})

const runModels = z.array(z.looseObject({ id: z.string(), name: z.string() }))

export const runSchema = z.looseObject({
  id: z.string(),
  dataset_id: z.string(),
  status: runStatusSchema,
  models: runModels,
  progress: z.looseObject({ done: z.number(), total: z.number() }),
  results: z.array(
    z.looseObject({
      task_id: z.string(),
      model_id: z.string(),
      status: z.enum(['pending', 'running', 'done', 'failed', 'cancelled']),
      score: nullable(z.number()),
      resolved: nullable(z.boolean()),
      cost_usd: nullable(z.number()),
      latency_ms: nullable(z.number()),
      failure: nullable(z.string()),
      builder: z.boolean(),
    }),
  ),
  error: nullable(z.string()),
  created_at: z.string(),
})

export const runsSchema = z.array(
  z.looseObject({
    id: z.string(),
    dataset_id: z.string(),
    harness: z.string(),
    status: runStatusSchema,
    models: runModels,
    created_at: z.string(),
  }),
)
