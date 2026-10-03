/**
 * Wire types for the agent routes (nasiko-server @ cb3aaf0c). Where the generated OpenAPI
 * schema is right it is reused; grants, secrets, containers and resources are missing from
 * it, and `/versions` is documented as a bare array although it is enveloped, so those are
 * written by hand here.
 *
 * Contract facts that shape the UI (plan §3):
 * - `GET /api/agents` is a bare `Agent[]` (snake_case), limit ≤ 100, `owner` must be a UUID.
 * - `GET /api/agents/{id}` is `{data: AgentDetailResponse}`, mostly camelCase; read it only
 *   through `normalizeDetail()`.
 * - Errors on these routes are a bare status or plain text, not `{error, code}`.
 */
import { z } from 'zod'
import type { WireSubset } from '@/lib/api/client'
import type { components } from '@/lib/api/schema.gen'

type S = components['schemas']

export type Agent = S['Agent']
export type AgentDetailResponse = S['AgentDetailResponse']
export type AgentVersion = S['AgentVersion']
export type DeploymentRow = S['DeploymentRow']
export type DeletedAgent = S['DeletedAgent']
export type RollbackResponse = S['RollbackResponse']

export interface Skill {
  id: string
  name: string
  description: string
  tags?: string[]
  examples?: unknown[]
}

/** `GET /api/agents/{id}/secrets` (agent_secrets.rs): names only, values are never returned. */
export interface SecretName {
  name: string
  updated_at: string | null
}

/** `GET /api/agents/{id}/grants` (agents/grants.rs `GrantsSummary`). */
export interface GrantsSummary {
  agent_id: string
  is_public: boolean
  user_grants: string[]
  agent_acl: string[]
}

/** `GET /api/agents/{id}/grants/users` (list_user_grants). */
export interface UserGrant {
  user_id: string
  /** Null when the user was deleted (LEFT JOIN). */
  username: string | null
}

/** `GET /api/agents/{id}/grants/agents` (list_agent_grants). */
export interface AgentGrant {
  target_agent_id: string
  /** Null when the target agent was deleted (LEFT JOIN). */
  target_name: string | null
}

/** `GET /api/search/users?q=` (catalog/routes.rs `UserSearchResult`), inside `{data}`. */
export interface UserSearchResult {
  id: string
  username: string
  display_name: string
}

/** `GET /api/users` (users/routes.rs `UserRow`, superuser only), inside `{data, total}`. */
export interface UserRow {
  id: string
  username: string
  display_name?: string | null
}

// ─── response schemas (lib/api/client.ts `Checked`): the fields the UI reads, extras allowed ────────────────────

const num = z.number().nullish()

/**
 * `GET /api/observability/agent/{ref}/stats` → `{data: {project: …}}` (handler.rs). Cost is Phoenix-style `{cost}`
 * buckets (observability/service.rs get_agent_stats); there is no `total_cost` (the Activity tab once read it and
 * showed $0.00: CHANGELOG 0.9.0.0).
 */
const agentStats = z.looseObject({
  trace_count: num,
  cost_summary: z.looseObject({ total: z.looseObject({ cost: num }).nullish() }).nullish(),
  latency_ms_p50: num,
  latency_ms_p99: num,
})
export type AgentStats = z.infer<typeof agentStats>
/** The stats sit under `project`; older servers sent them flat. */
export const agentStatsSchema = agentStats.extend({ project: agentStats.nullish() })

/** `GET /api/agents/{id}` data: the fields that aren't read defensively by `normalizeDetail`. */
export const agentDetailSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  display_name: z.string().nullish(),
  owner_id: z.string(),
  status: z.string(),
  can_manage: z.boolean(),
  is_coding_agent: z.boolean(),
  coding_agent_integration_id: z.string().nullish(),
  tags: z.array(z.string()),
  skills: z.array(z.unknown()),
}) satisfies z.ZodType<WireSubset<AgentDetailResponse>>

/** The finops dashboard's rows, for Your agents' 24 h usage (`useUsage24h`). */
export const usage24hSchema = z.looseObject({
  agents: z.array(
    z.looseObject({ agent_id: z.string(), operations: z.number(), total_cost: z.number() }),
  ),
}) satisfies z.ZodType<WireSubset<{ agents: S['AgentFinopsRow'][] }>>

/** `GET /api/observability/agent/{ref}/resources` (resources.rs). */
export interface AgentResources {
  agent_id?: string
  usage: {
    cpu_percent?: number | null
    memory_usage_bytes?: number | null
    memory_limit_bytes?: number | null
  } | null
  collected_at?: string | null
}

/** Read routes a non-deployer calls answer `200 {"available": false}` (lib.rs:108-111). */
export interface Unavailable {
  available: false
}
