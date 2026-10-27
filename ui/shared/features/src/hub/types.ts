/**
 * Wire types for the hub's one proposed read, the agent event log (AH-3 in
 * docs/designs/openruntime-agent-hub-recommendations.md): `GET /api/observability/agent/{id}/events?start_time&end_time&limit`,
 * in the `{data}` envelope. No server has it yet: it is mocked under the `events` group and read only while that
 * group is a preview (`isPreview('events')`). A bare 404 means the server predates it.
 *
 * Kinds with their own icon: deploy, restart, start, stop, rollback, crash, route_change, optimization_change,
 * compress_change, mcp_rule, budget_warning, budget_stop. An unknown kind keeps its summary and gets the generic icon.
 */
import { z } from 'zod'

/** A timestamp the page will format: an unparseable one fails the read instead of throwing while rendering. */
const timestamp = z.string().refine((s) => !Number.isNaN(Date.parse(s)), 'not a timestamp')

const agentEventSchema = z.looseObject({
  id: z.string(),
  kind: z.string(),
  at: timestamp,
  /** Who made the change (a username), null for the platform itself (a crash, a budget crossing). */
  actor: z.string().nullable(),
  /** One plain sentence the server writes, e.g. "Routing switched to gpt-4.1-mini". */
  summary: z.string(),
  before: z.string().nullable().optional(),
  after: z.string().nullable().optional(),
})
export type AgentEvent = z.infer<typeof agentEventSchema>

export const agentEventsSchema = z.looseObject({
  events: z.array(agentEventSchema),
  /** More events exist in the window than the read returned (newest first). */
  truncated: z.boolean(),
})
export type AgentEvents = z.infer<typeof agentEventsSchema>

/**
 * The ledger's proposed per-session budget read (AH-8): `GET /api/budgets/session/{session_id}`, in the `{data}`
 * envelope, one entry per budget that applies to the session (its own, its agent's, its owner's). Every budget figure
 * is a preview (plans/feat-agent-centric.md decision 35: budgets are the proposed R-L10 contract), read only while the
 * `router` group is a preview. `used_before_usd` is null when no consumption record covers the period before the
 * session started ("not recorded", never 0).
 */
const sessionBudgetSchema = z.looseObject({
  budget_id: z.string(),
  scope: z.enum(['session', 'agent', 'owner']),
  limit_usd: z.number().positive(),
  /** The budget's period as `YYYY-MM` (UTC), or null for a session budget, which has none. */
  period: z
    .string()
    .regex(/^\d{4}-\d{2}$/)
    .nullable(),
  used_before_usd: z.number().nonnegative().nullable(),
  session_used_usd: z.number().nonnegative(),
  /** What this budget recorded per turn, keyed by the turn id the messages carry (`external_turn_id`). */
  turns: z.array(z.looseObject({ turn_id: z.string(), used_usd: z.number().nonnegative() })),
})
export type SessionBudget = z.infer<typeof sessionBudgetSchema>
export const sessionBudgetsSchema = z.array(sessionBudgetSchema)
