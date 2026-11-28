/**
 * The code review evals routes (`/api/evals/code-review/*`, `oss/server/src/code_review/api.rs`;
 * `oss/docs/proposals/evals/CODE_REVIEW_EVALS.md`). Every answer is `{data}`; errors are
 * `{error, code}`. The schemas check only the fields this page reads.
 *
 * This reviews one imported pull request at a time, not a dataset of many — see the proposal doc
 * for why (a thin, fully-real slice over a wide, partly-simulated one).
 */
import { z } from 'zod'

export const severitySchema = z.enum(['critical', 'high', 'medium', 'low'])
export const categorySchema = z.enum(['bug', 'security', 'quality'])
export type Severity = z.infer<typeof severitySchema>
export type Category = z.infer<typeof categorySchema>

const nullable = <T extends z.ZodType>(t: T) => t.nullable().optional()

export const referenceFindingSchema = z.object({
  file: z.string(),
  line: z.number(),
  severity: severitySchema,
  category: categorySchema,
  description: z.string(),
})
export type ReferenceFinding = z.infer<typeof referenceFindingSchema>

export const modelFindingSchema = z.object({
  file: z.string(),
  line: z.number(),
  severity: severitySchema,
  category: categorySchema,
  description: z.string(),
  confidence: z.number(),
})
export type ModelFinding = z.infer<typeof modelFindingSchema>

export const metricsSchema = z.object({
  true_positives: z.number(),
  false_positives: z.number(),
  false_negatives: z.number(),
  precision: z.number(),
  recall: z.number(),
  severity_accuracy: nullable(z.number()),
})
export type Metrics = z.infer<typeof metricsSchema>

export const reviewTaskSchema = z.looseObject({
  id: z.string(),
  owner: z.string(),
  repo: z.string(),
  pr_number: z.number(),
  pr_title: z.string(),
  base_sha: z.string(),
  head_sha: z.string(),
  diff: z.string(),
  files: z.record(z.string(), z.string()),
  reference_findings: z.array(referenceFindingSchema),
  graded: z.boolean(),
  created_at: z.string(),
})
export type ReviewTask = z.infer<typeof reviewTaskSchema>

export const reviewTasksSchema = z.array(reviewTaskSchema)

export const runCreatedSchema = z.looseObject({
  id: z.string(),
  status: z.string(),
})
export type RunCreated = z.infer<typeof runCreatedSchema>

export const attemptResultSchema = z.looseObject({
  model_id: z.string(),
  model_name: z.string(),
  provider: z.string(),
  model: nullable(z.string()),
  status: z.enum(['pending', 'running', 'done', 'failed']),
  findings: z.array(modelFindingSchema),
  metrics: nullable(metricsSchema),
  latency_ms: nullable(z.number()),
  cost_usd: nullable(z.number()),
  input_tokens: nullable(z.number()),
  output_tokens: nullable(z.number()),
  worst_price_source: nullable(z.string()),
  error: nullable(z.string()),
})
export type AttemptResult = z.infer<typeof attemptResultSchema>

export const reviewRunSchema = z.looseObject({
  id: z.string(),
  task_id: z.string(),
  status: z.enum(['running', 'done', 'failed']),
  graded: z.boolean(),
  sample_size_warning: z.string(),
  results: z.array(attemptResultSchema),
})
export type ReviewRun = z.infer<typeof reviewRunSchema>
