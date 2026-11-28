/**
 * `GET /api/evals/code-review/universe` (`oss/server/src/code_review/api.rs::get_universe`): every
 * task and attempt the caller has run, flattened for a cross-task view. The caller's own data
 * only — same scoping as every other code review evals route. See
 * `oss/docs/proposals/evals/CODE_REVIEW_EVALS.md`'s "Eval Universe" section for what this is and
 * what it deliberately does not do (no clustering algorithm, no pagination, single user).
 */
import { z } from 'zod'
import { metricsSchema } from '../code-review/types'

const nullable = <T extends z.ZodType>(t: T) => t.nullable().optional()

export const taskSummarySchema = z.looseObject({
  id: z.string(),
  owner: z.string(),
  repo: z.string(),
  pr_number: z.number(),
  pr_title: z.string(),
  graded: z.boolean(),
  created_at: z.string(),
})
export type TaskSummary = z.infer<typeof taskSummarySchema>

export const universeAttemptSchema = z.looseObject({
  attempt_id: z.string(),
  run_id: z.string(),
  task_id: z.string(),
  model_id: z.string(),
  model_name: z.string(),
  provider: z.string(),
  model: nullable(z.string()),
  status: z.string(),
  metrics: nullable(metricsSchema),
  latency_ms: nullable(z.number()),
  cost_usd: nullable(z.number()),
  input_tokens: nullable(z.number()),
  output_tokens: nullable(z.number()),
  worst_price_source: nullable(z.string()),
  error: nullable(z.string()),
})
export type UniverseAttempt = z.infer<typeof universeAttemptSchema>

export const universeViewSchema = z.looseObject({
  tasks: z.array(taskSummarySchema),
  attempts: z.array(universeAttemptSchema),
})
export type UniverseView = z.infer<typeof universeViewSchema>
