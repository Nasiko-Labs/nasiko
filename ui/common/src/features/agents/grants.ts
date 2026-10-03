/**
 * Agent grants in both editions (recorded at nasiko-server ea233d20, src/test/__live__/{oss,ee}/agents/):
 * - OSS `GET /api/agents/{id}/grants` (agents/grants.rs) answers a summary `{agent_id, is_public, user_grants,
 *   agent_acl}`, and lists users and agents at `/grants/users` and `/grants/agents`.
 * - EE (ee/server/src/grants.rs) answers a bare `Grant[]` and has no `/grants/users` or `/grants/agents` reads: those
 *   paths fall through to the agent proxy (503 for an agent that isn't running). Writes are path-shaped:
 *   `POST|DELETE /grants/users/{user_id}`, `/grants/agents/{target_id}`.
 * The Access tab works on the summary; `normalizeGrants` builds it from either shape once `grantsBodySchema` has
 * checked it (the EE shape once broke this tab outright: CHANGELOG 0.9.0.0).
 */
import { z } from 'zod'
import type { WireSubset } from '@/lib/api/client'
import type { GrantsSummary } from './types'

/** An EE grant row (`Grant` in ee/server/src/grants.rs). */
export interface EeGrant {
  id: string
  agent_id: string
  grant_type: 'user' | 'org_unit' | 'organization' | 'agent' | 'public' | string
  grantee_id: string
  granted_by: string | null
  created_at: string
}

export interface AccessGrants extends GrantsSummary {
  edition: 'oss' | 'ee'
  /** EE only: org units granted access, and whether the whole organization is. */
  units: number
  organization: boolean
}

/** Either edition's `/grants` body; only the fields the summary is built from are checked. */
export const grantsBodySchema = z.union([
  z.array(z.looseObject({ grant_type: z.string(), grantee_id: z.string() })),
  z.looseObject({
    agent_id: z.string(),
    is_public: z.boolean(),
    user_grants: z.array(z.string()),
    agent_acl: z.array(z.string()),
  }),
]) satisfies z.ZodType<WireSubset<EeGrant[] | GrantsSummary>>
export type GrantsBody = z.infer<typeof grantsBodySchema>

/** The summary from either edition's `/grants` body (checked by `grantsBodySchema`). */
export function normalizeGrants(body: GrantsBody, agentId: string): AccessGrants {
  if (Array.isArray(body)) {
    const of = (t: string) => body.filter((g) => g.grant_type === t).map((g) => g.grantee_id)
    // `is_public` from grant rows is a fallback only: EE's real flag is agents.is_public (the caller reads /visibility).
    return {
      edition: 'ee',
      agent_id: agentId,
      is_public: of('public').length > 0,
      user_grants: of('user'),
      agent_acl: of('agent'),
      units: of('org_unit').length,
      organization: of('organization').length > 0,
    }
  }
  const { agent_id, is_public, user_grants, agent_acl } = body
  return {
    agent_id,
    is_public,
    user_grants,
    agent_acl,
    edition: 'oss',
    units: 0,
    organization: false,
  }
}

/** Where a grant write goes: OSS takes the id in the body, EE in the path. */
export function grantWrite(
  edition: 'oss' | 'ee' | undefined,
  agentId: string,
  kind: 'users' | 'agents',
  targetId: string,
): { path: string; body?: Record<string, string> } {
  const base = `/api/agents/${agentId}/grants/${kind}`
  // Never guess: an OSS-shaped write to EE falls through to the agent proxy and reaches the agent itself.
  if (edition === undefined)
    throw new Error('grant writes need the server edition: /grants has not loaded')
  if (edition === 'ee') return { path: `${base}/${targetId}` }
  return { path: base, body: kind === 'users' ? { user_id: targetId } : { agent_id: targetId } }
}
