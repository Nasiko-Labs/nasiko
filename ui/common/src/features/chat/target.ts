/**
 * Who a chat talks to (plan §6.11, DX-A1, EN12; v1b §5.1). Pure, so every branch is unit-tested.
 * Only `/chat?auto=1` or an existing routed chat is routed: an unresolved `?agent=` disables Send
 * and says why, and never falls back to a routed chat.
 */
import type { useAgentsDirectory } from '@/features/agents/api'
import { isHarness } from '@/features/agents/status'
import type { Agent, Skill } from '@/features/agents/types'
import { agentLabel, chatKind } from './format'
import { exampleTexts, isUuid } from '@/features/agents/normalize'
import { foldAgentName } from './agentName'
import { agentParamText } from './search'

import type { RememberedTarget } from './rememberTarget'
import type { ChatSessionRow, HitlDto } from './types'

export type Target =
  | { kind: 'loading' }
  | { kind: 'choose' }
  | { kind: 'invalid'; value: string }
  | { kind: 'ambiguous'; value: string; agents: Agent[] }
  | { kind: 'direct'; agent: Agent }
  | { kind: 'routed' }
  | { kind: 'readonly'; why: 'recorded' | 'removed'; agentId?: string | null }

interface Directory {
  loaded: boolean
  byId: Map<string, Agent>
  byNameAll: Map<string, Agent[]>
}

const chattable = (a: Agent) => !isHarness(a)

/** Longest `?agent=` value treated as an id or name; longer is the banner (DX-1). */
const AGENT_PARAM_MAX = 200

/**
 * `/chat?agent=` and `?auto=`, in that order (UC1): a present `agent` decides (a UUID is the
 * target; a name resolves among the agents the caller can see; a blank, oversized or non-string
 * value is the banner) and `auto` is ignored; else `auto` is routed; else the chooser.
 */
export function resolveAgentParam(raw: unknown, dir: Directory, auto = false): Target {
  if (raw === undefined) return auto ? { kind: 'routed' } : { kind: 'choose' }
  if (typeof raw !== 'string') return { kind: 'invalid', value: agentParamText(raw) }
  const value = raw.trim()
  if (!value || value.length > AGENT_PARAM_MAX) return { kind: 'invalid', value: raw }
  if (!dir.loaded) return { kind: 'loading' }
  if (isUuid(value)) {
    const a = dir.byId.get(value.toLowerCase()) ?? dir.byId.get(value)
    return a && chattable(a) ? { kind: 'direct', agent: a } : { kind: 'invalid', value }
  }
  const matches = (dir.byNameAll.get(value) ?? []).filter(chattable)
  if (matches.length === 1) return { kind: 'direct', agent: matches[0] }
  if (matches.length > 1) return { kind: 'ambiguous', value, agents: matches }
  return { kind: 'invalid', value }
}

/**
 * An existing chat: its kind comes from the rail row, else a request's execution record. A
 * request from the orchestrator makes the chat routed before its `agent_id` (the sub-agent's) is
 * consulted (EN-6). `listDone` means the list has no more pages to search.
 */
export function resolveChatTarget(
  row: ChatSessionRow | undefined,
  requests: readonly HitlDto[],
  listDone: boolean,
  dir: Directory,
): Target {
  if (row) {
    const kind = chatKind(row)
    if (kind === 'routed') return { kind: 'routed' }
    if (kind === 'recorded') return { kind: 'readonly', why: kind, agentId: row.agent_id }
    if (kind === 'removed') return { kind: 'readonly', why: 'removed' }
    // chatKind says direct only with an agent_id.
    return row.agent_id ? fromAgentId(row.agent_id, dir) : { kind: 'readonly', why: 'removed' }
  }
  if (requests.some((r) => r.execution.origin === 'orchestrator')) return { kind: 'routed' }
  const fromRequest = requests.find((r) => r.execution.agent_id)?.execution.agent_id
  if (fromRequest) return fromAgentId(fromRequest, dir)
  return listDone ? { kind: 'readonly', why: 'removed' } : { kind: 'loading' }
}

function fromAgentId(agentId: string, dir: Directory): Target {
  if (!dir.loaded) return { kind: 'loading' }
  const a = dir.byId.get(agentId)
  return a ? { kind: 'direct', agent: a } : { kind: 'readonly', why: 'removed', agentId }
}

/** The agents the chooser offers: running A2A agents only, never harnesses. */
export function chooserAgents(rows: readonly Agent[] | undefined): Agent[] {
  return (rows ?? []).filter((a) => chattable(a) && a.status === 'running')
}

/**
 * The agent a routed frame named (§5.8): `tool_call.agent` is a display name, not a UUID. Exactly
 * one running, accessible, chattable agent under the folded name → that agent, else null. A
 * fallback for links only, never the source of truth for attribution.
 */
export function resolveAskedAgent(
  name: string,
  agents: readonly Agent[] | undefined,
): { id: string; name: string } | null {
  const key = foldAgentName(name)
  const matches = chooserAgents(agents).filter((a) => foldAgentName(a.name) === key)
  return matches.length === 1 ? { id: matches[0].id, name: matches[0].name } : null
}

/** A new chat's default target (v1c §5.4): the remembered one (C2), else the only running agent (UC-A). */
export type Preselect = { kind: 'routed' } | { kind: 'agent'; id: string } | null

/**
 * What `/chat` with no `?agent=` or `?auto=1` preselects, re-evaluated as the directory loads. Nothing
 * once the user has typed (their text belongs to "no target"). A remembered Orchestrator needs no
 * directory row; a remembered agent must be present, running and not a harness. Otherwise exactly
 * one running agent is preselected; with none or several the user chooses (UC1).
 */
export function preselectTarget(o: {
  remembered: RememberedTarget | null
  agents: readonly Agent[] | undefined
  typed: boolean
}): Preselect {
  if (o.typed) return null
  if (o.remembered?.kind === 'orchestrator') return { kind: 'routed' }
  if (!o.agents) return null
  const running = chooserAgents(o.agents)
  const remembered =
    o.remembered?.kind === 'agent'
      ? running.find((a) => a.id === (o.remembered as { id: string }).id)
      : undefined
  if (remembered) return { kind: 'agent', id: remembered.id }
  return running.length === 1 ? { kind: 'agent', id: running[0].id } : null
}

/**
 * Prompts that ask an agent about itself: fine as that agent's example, weak as the Orchestrator's (it can only
 * answer them vaguely). Skipped for the Orchestrator's chips.
 */
const GENERIC_PROMPT =
  /^\s*(hi|hello|hey|help|who are you|what (can|do) you (do|help( me)? with)|what do you do|how can you help( me)?)\s*[?!.]*\s*$/i
export const isGenericPrompt = (text: string) => GENERIC_PROMPT.test(text)

/**
 * One example prompt per running agent for the Orchestrator's suggestions (C1): the first string of the first
 * skill that has one, skipping generic prompts ("What can you help me with?").
 */
export function orchestratorExamples(
  agents: readonly Agent[] | undefined,
  max = 3,
): { agentId: string; text: string }[] {
  const out: { agentId: string; text: string }[] = []
  for (const a of chooserAgents(agents)) {
    const text = ((a.skills ?? []) as Skill[])
      .flatMap((s) => exampleTexts(s))
      .find((t) => !!t && !isGenericPrompt(t))
    if (text && !out.some((o) => o.text === text)) out.push({ agentId: a.id, text })
    if (out.length === max) break
  }
  return out
}

/** Up to `max` example prompts for an agent: the first string in each skill's `examples` (§5.4). */
export function agentExamples(agent: Agent, max = 3): string[] {
  const out: string[] = []
  for (const s of (agent.skills ?? []) as Skill[]) {
    const t = exampleTexts(s)[0]
    if (t && !out.includes(t)) out.push(t)
    if (out.length === max) break
  }
  return out
}

/** The TargetPicker's agents (v1c §5.4): running ones by display name, then the rest, never harnesses. */
export function pickerAgents(rows: readonly Agent[] | undefined): {
  running: Agent[]
  stopped: Agent[]
} {
  const all = (rows ?? [])
    .filter(chattable)
    .sort((a, b) => agentLabel(a).localeCompare(agentLabel(b)))
  return {
    running: all.filter((a) => a.status === 'running'),
    stopped: all.filter((a) => a.status !== 'running'),
  }
}

/** The directory as the target rules read it. */
export const directoryOf = (dir: ReturnType<typeof useAgentsDirectory>) => ({
  loaded: !!dir.data || dir.isError,
  byId: dir.byId,
  byNameAll: dir.byNameAll,
})
