/**
 * Wire types for MAF workflows (plans/feat-workflows.md). nasiko-cloud-rs `main` `a6feda17` `oss/server/src/maf.rs`
 * (`MafResponse`, `ExecResponse`, `ExecWithHitlResponse`, `ExecWithWorkflowResponse`, `GenerateMafResponse`), plus the
 * NAS-697 additions at nasiko-cloud-rs-react `65908711` (`WorkflowListResponse`: the list metrics; `status: 'draft'`;
 * `hitl` on the runs list). Every route answers `{data, status_code, message}`; lists nest rows as `data.data`
 * (`Paginated`, `total` = page length).
 *
 * `/api/maf/*` is not in the OpenAPI spec (W-5), so the shapes are zod schemas: `z.looseObject`, only the fields the UI
 * reads, the NAS-697 ones optional (absent on `main`).
 */
import { z } from 'zod'
import type { HitlDto } from '@/features/chat/types'

const nullableString = z.string().nullable().optional()

/** `MafStep`: `step_id` is regenerated on every create and PUT. `agent_name` is `agents.name`, not display_name. */
const stepSchema = z.looseObject({
  step_id: z.string(),
  step_index: z.number(),
  agent_id: z.string(),
  agent_name: z.string(),
  task_description: z.string(),
})
export type MafStep = z.infer<typeof stepSchema>

/** `MafResponse`: GET/PUT one, create, draft, promote. `maf_json` is null when the stored JSON does not parse. */
export const workflowSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  description: nullableString,
  maf_json: z
    .looseObject({
      description: nullableString,
      steps: z.array(stepSchema).optional(),
      output_generation: nullableString,
    })
    .nullable(),
  /** `main`: always `active` here (lists and reads skip deleted rows). NAS-697 adds `draft`. */
  status: z.string().optional(),
  created_at: z.string().optional(),
  updated_at: z.string().optional(),
  execution_count: z.number().optional(),
})
export type Workflow = z.infer<typeof workflowSchema>

/** NAS-697 `Health`: bucketed from the all-time success rate (≥ 90 healthy, ≥ 50 degraded); `unknown` = never run. */
export type Health = 'healthy' | 'degraded' | 'unhealthy' | 'unknown'

/** A list row: `MafResponse` on `main`; NAS-697 `WorkflowListResponse` adds the metrics. */
export const workflowRowSchema = workflowSchema.extend({
  /** Percent to 1 dp; null = never run (not 0). */
  success_rate: z.number().nullable().optional(),
  health: z.enum(['healthy', 'degraded', 'unhealthy', 'unknown']).optional(),
  total_tokens: z.number().optional(),
  /** Start of the latest *started* run; queued runs are excluded. */
  last_run_at: nullableString,
  last_run_status: nullableString,
  step_count: z.number().optional(),
  agent_names: z.array(z.string()).optional(),
})
export type WorkflowRow = z.infer<typeof workflowRowSchema>

export const workflowListSchema = z.looseObject({ data: z.array(workflowRowSchema) })

/** `StepResult` (orchestrator `maf/types.rs`): one entry per step from the run's start; unstarted ones have no result. */
const stepResultSchema = z.looseObject({
  step_id: z.string(),
  step_index: z.number(),
  agent_id: nullableString,
  agent_name: nullableString,
  status: z.string(),
  error: nullableString,
  prompt: nullableString,
  extracted_info: nullableString,
  tokens_used: z.number().optional(),
  latency_ms: z.number().optional(),
})
export type StepResult = z.infer<typeof stepResultSchema>

/** A HITL row is `router/hitl.rs to_response`, the same DTO chat reads. */
const hitlSchema = z.custom<HitlDto>((v) => typeof v === 'object' && v !== null)

/** `ExecWithHitlResponse` (one run): `ExecResponse` + every HITL row of the run, pending and decided, oldest first. */
export const executionSchema = z.looseObject({
  id: z.string(),
  execution_number: z.number(),
  maf_id: z.string().nullable(),
  status: z.string(),
  attempt_count: z.number().optional(),
  max_attempts: z.number().optional(),
  tokens_used: z.number().optional(),
  started_at: nullableString,
  completed_at: nullableString,
  duration_ms: z.number().nullable().optional(),
  output: nullableString,
  step_results: z.array(stepResultSchema).nullable().optional(),
  error: nullableString,
  created_at: z.string(),
  /** Absent on `main`'s runs list (W-4). */
  hitl: z.array(hitlSchema).optional(),
})
export type Execution = z.infer<typeof executionSchema>

/** `ExecWithWorkflowResponse`: the workflow LEFT JOINed, so its name and status survive a delete (null once gone). */
export const executionRowSchema = executionSchema.extend({
  workflow_name: nullableString,
  workflow_status: nullableString,
})
export type ExecutionRow = z.infer<typeof executionRowSchema>

export const executionListSchema = z.looseObject({ data: z.array(executionRowSchema) })

/** `GenerateMafResponse`: the planner's proposal, never stored. */
export const planSchema = z.looseObject({
  name: z.string(),
  description: z.string(),
  output_generation: z.string(),
  steps: z.array(
    z.looseObject({ agent_id: z.string(), agent_name: z.string(), task_description: z.string() }),
  ),
})
export type GeneratedPlan = z.infer<typeof planSchema>

/** `run_workflow`: 202. */
export const runStartedSchema = z.looseObject({
  execution_id: z.string(),
  execution_number: z.number(),
  execution_count: z.number(),
})
export type RunStarted = z.infer<typeof runStartedSchema>

/** A step as sent: `UpdateStepRequest` on `main` requires `step_index` (W-2); create ignores it. */
export interface StepInput {
  step_index: number
  task_description: string
  agent_id?: string
}
