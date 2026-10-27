/**
 * Wire types for flows (plans/feat-flows.md §0). nasiko-cloud-rs `development` `6ab60326` `oss/server/src/flows.rs`
 * (`Flow`, `FlowStep`; `list_flows` answers `Paginated<Flow>` with `total` = the page length, `get_flow` answers
 * `{flow, steps}` with steps by `step_order`). Not in the OpenAPI spec (FL-7), so the shapes are zod schemas:
 * `z.looseObject`, only the fields the UI reads. The FL-2/FL-3 step fields are optional (absent on today's server).
 */
import { z } from 'zod'

const nullableString = z.string().nullable().optional()

/** `Flow`. `flow_id` is the request's OTel trace id; the totals are never written today (FL-1). */
const flowSchema = z.looseObject({
  flow_id: z.string(),
  root_agent_id: nullableString,
  root_agent_name: nullableString,
  title: nullableString,
  /** `running`, `completed`, `failed`, `paused`. */
  status: z.string(),
  duration_ms: z.number().nullable().optional(),
  error_message: nullableString,
  /** A free JSON value: `context_id` (proxy, when a chat session exists), `mode` (`free_flowing` for MAF). */
  metadata: z.unknown().optional(),
  created_at: z.string(),
  completed_at: nullableString,
})
export type Flow = z.infer<typeof flowSchema>

/** `FlowStep`. `latency_ms` is rounded to whole seconds today (FL-4); the timestamps are not. */
const flowStepSchema = z.looseObject({
  id: z.string(),
  step_order: z.number(),
  depth: z.number(),
  agent_id: nullableString,
  agent_name: z.string(),
  caller_agent_name: nullableString,
  input_summary: nullableString,
  output_summary: nullableString,
  /** `pending`, `running`, `completed`, `failed`, `awaiting_human`, `resumed`. */
  status: z.string(),
  tokens_used: z.number().optional(),
  latency_ms: z.number().nullable().optional(),
  error_message: nullableString,
  created_at: z.string(),
  completed_at: nullableString,
  /** Proposed (FL-2, FL-3). */
  call_id: nullableString,
  parent_step_id: nullableString,
  caller_agent_id: nullableString,
})
export type FlowStep = z.infer<typeof flowStepSchema>

export const flowListSchema = z.looseObject({ data: z.array(flowSchema), total: z.number() })

export const flowDetailSchema = z.looseObject({ flow: flowSchema, steps: z.array(flowStepSchema) })
export type FlowDetail = z.infer<typeof flowDetailSchema>

/** A string field of the flow's free-form `metadata`, or null. */
export function metaString(flow: Pick<Flow, 'metadata'>, key: string): string | null {
  const m = flow.metadata
  if (!m || typeof m !== 'object' || Array.isArray(m)) return null
  const v = (m as Record<string, unknown>)[key]
  return typeof v === 'string' && v ? v : null
}

/**
 * The flow's chat session (O1): FL-5's `session_id` when the server sends it, else `metadata.context_id`, which the
 * dispatcher and the proxy set to the A2A context (chat sends its session id there). A workflow flow's `context_id` is
 * its MAF execution id (`maf/executor.rs`), never a session, so it has none until FL-5.
 */
export function flowSession(flow: Flow): string | null {
  const s = (flow as Record<string, unknown>).session_id
  if (typeof s === 'string' && s) return s
  if (metaString(flow, 'mode') === 'free_flowing') return null
  return metaString(flow, 'context_id')
}
