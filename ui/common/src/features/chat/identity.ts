/**
 * One identity per chat kind (v1c §5.2): the rail row, the header and the composer target all read
 * it. Built on `chatKind`, so it never disagrees with how the page treats a chat. Pure.
 */
import type { Agent } from '@/features/agents/types'
import { copy } from './copy'
import { agentLabel, chatKind, statusLabel, type ChatKind } from './format'
import type { ChatSessionRow } from './types'

export type IdentityKind = 'agent' | 'orchestrator' | 'recorded' | 'removed'

export interface ChatIdentity {
  kind: IdentityKind
  /** The display name: the agent's, "Orchestrator", the harness agent's, or "Agent removed". */
  name: string
  /** Line 2 of a rail row: the name, or the kind in words for Recorded and Removed (DS7). */
  railLabel: string
  /** The header chip's accessible description (§5.2 sublines, DS5). */
  subline: string
  agentId?: string
  /** The agent's status from the directory; absent when the directory doesn't know the agent. */
  status?: string
}

const KIND: Record<ChatKind, IdentityKind> = {
  direct: 'agent',
  routed: 'orchestrator',
  recorded: 'recorded',
  removed: 'removed',
}

/**
 * A recorded chat's agent name: the list prefixes harness names with `<username>-` (§2.2). The list
 * is caller-scoped, so the prefix is always the viewer's; until `me` loads the raw name shows.
 */
export function recordedName(raw: string | null | undefined, username?: string): string {
  const name = raw?.trim() ?? ''
  const prefix = username ? `${username}-` : ''
  return prefix && name.startsWith(prefix) && name.length > prefix.length
    ? name.slice(prefix.length)
    : name
}

export function chatIdentity(
  row: Pick<ChatSessionRow, 'agent_id' | 'agent_url' | 'is_coding_agent' | 'agent_name'>,
  byId: ReadonlyMap<string, Agent> | undefined,
  username?: string,
): ChatIdentity {
  const kind = KIND[chatKind(row)]
  switch (kind) {
    case 'agent': {
      const a = row.agent_id ? byId?.get(row.agent_id) : undefined
      const name = a ? agentLabel(a) : row.agent_name?.trim() || copy.identityAgent
      return {
        kind,
        name,
        railLabel: name,
        subline: a ? copy.identityAgentSubline(statusLabel(a.status)) : copy.identityAgent,
        agentId: row.agent_id ?? undefined,
        status: a?.status,
      }
    }
    case 'orchestrator':
      return {
        kind,
        name: copy.orchestratorName,
        railLabel: copy.orchestratorName,
        subline: copy.identityOrchestratorSubline,
      }
    case 'recorded': {
      const name = recordedName(row.agent_name, username) || copy.recordedBadge
      return {
        kind,
        name,
        railLabel: copy.recordedBadge,
        subline: copy.identityRecordedSubline,
        agentId: row.agent_id ?? undefined,
      }
    }
    case 'removed':
      return {
        kind,
        name: copy.identityRemoved,
        railLabel: copy.identityRemoved,
        subline: copy.identityRemovedSubline,
      }
  }
}

/** The identity of a new chat's target (§5.4): an agent, the Orchestrator, or nothing chosen yet. */
export function targetIdentity(
  target: { kind: 'direct'; agent: Agent } | { kind: 'routed' },
): ChatIdentity {
  if (target.kind === 'routed')
    return {
      kind: 'orchestrator',
      name: copy.orchestratorName,
      railLabel: copy.orchestratorName,
      subline: copy.identityOrchestratorSubline,
    }
  const name = agentLabel(target.agent)
  return {
    kind: 'agent',
    name,
    railLabel: name,
    subline: copy.identityAgentSubline(statusLabel(target.agent.status)),
    agentId: target.agent.id,
    status: target.agent.status,
  }
}
