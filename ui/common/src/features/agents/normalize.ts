/**
 * `GET /api/agents/{id}` mixes camelCase (the A2A card fields) with a few snake_case ones
 * (catalog/routes.rs `AgentDetailResponse`, `rename_all = "camelCase"` plus overrides).
 * Components read only this view model, so a server rename breaks one function, not a page.
 */
import { isHarness } from './status'
import type { AgentDetailResponse, Skill } from './types'

export interface AgentView {
  id: string
  name: string
  displayName: string
  description: string
  ownerId: string
  status: string
  version: string
  url: string
  iconUrl: string | null
  documentationUrl: string | null
  protocolVersion: string
  preferredTransport: string
  inputModes: string[]
  outputModes: string[]
  capabilities: { streaming: boolean; pushNotifications: boolean; stateTransitionHistory: boolean }
  skills: Skill[]
  tags: string[]
  canManage: boolean
  isHarness: boolean
  integrationId: string | null
  /** The raw `metadata` bag: a PUT replaces the whole column, so feature writes spread it (catalog/routes.rs). */
  metadata: Record<string, unknown>
  /** `metadata.features.prompt_comments === 'enabled'` (state.rs `agent_env` → `NASIKO_PROMPT_COMMENTS`). */
  promptComments: boolean
  /** `compress_enabled`: token optimization across the stack. */
  compress: boolean
  /** `minimal_code_enabled`, offered only when `has_coding_skills` (the server's own gate). */
  minimalCode: boolean
  codingSkills: boolean
  createdAt: string
  updatedAt: string
}

const str = (v: unknown, fallback = ''): string => (typeof v === 'string' ? v : fallback)
const strOrNull = (v: unknown): string | null => (typeof v === 'string' && v.trim() ? v : null)
const strings = (v: unknown): string[] =>
  Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
const bool = (v: unknown): boolean => v === true
const record = (v: unknown): Record<string, unknown> =>
  v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {}

/**
 * The `metadata` PUT body that sets one `features.*` flag. The PUT replaces the whole column, so every other key
 * stays; `agent_env` reads string values only (`enabled` / `disabled`, state.rs).
 */
export function withFeature(metadata: Record<string, unknown>, key: string, on: boolean) {
  return {
    ...metadata,
    features: { ...record(metadata.features), [key]: on ? 'enabled' : 'disabled' },
  }
}

function skills(v: unknown): Skill[] {
  if (!Array.isArray(v)) return []
  return v.flatMap((raw): Skill[] => {
    if (!raw || typeof raw !== 'object') return []
    const s = raw as Record<string, unknown>
    const name = str(s.name) || str(s.id)
    if (!name) return []
    return [
      {
        id: str(s.id, name),
        name,
        description: str(s.description),
        tags: strings(s.tags),
        examples: Array.isArray(s.examples) ? s.examples : [],
      },
    ]
  })
}

/** Skill examples are strings in practice; anything else is dropped rather than rendered raw. */
export function exampleTexts(skill: Skill): string[] {
  return (skill.examples ?? []).filter(
    (e): e is string => typeof e === 'string' && e.trim().length > 0,
  )
}

export function normalizeDetail(d: AgentDetailResponse): AgentView {
  const raw = d as unknown as Record<string, unknown>
  // Capabilities are stored as the agent card sent them (camelCase keys, utils.rs:51);
  // accept snake_case too, in case a hand-registered agent used it.
  const metadata = record(raw.metadata)
  const caps = (
    raw.capabilities && typeof raw.capabilities === 'object' ? raw.capabilities : {}
  ) as Record<string, unknown>
  return {
    id: str(raw.id),
    name: str(raw.name),
    displayName: strOrNull(raw.display_name) ?? str(raw.name),
    description: str(raw.description),
    ownerId: str(raw.owner_id),
    status: str(raw.status),
    version: str(raw.version),
    url: str(raw.url),
    iconUrl: strOrNull(raw.iconUrl ?? raw.icon_url),
    documentationUrl: strOrNull(raw.documentationUrl ?? raw.documentation_url),
    protocolVersion: str(raw.protocolVersion ?? raw.protocol_version),
    preferredTransport: str(raw.preferredTransport ?? raw.preferred_transport),
    inputModes: strings(raw.defaultInputModes ?? raw.default_input_modes),
    outputModes: strings(raw.defaultOutputModes ?? raw.default_output_modes),
    capabilities: {
      streaming: bool(caps.streaming),
      pushNotifications: bool(caps.pushNotifications ?? caps.push_notifications),
      stateTransitionHistory: bool(caps.stateTransitionHistory ?? caps.state_transition_history),
    },
    skills: skills(raw.skills),
    tags: strings(raw.tags),
    canManage: bool(raw.can_manage),
    isHarness: isHarness({
      is_coding_agent: bool(raw.is_coding_agent),
      tags: strings(raw.tags),
      metadata: raw.metadata,
    }),
    integrationId: strOrNull(raw.coding_agent_integration_id),
    metadata,
    promptComments: record(metadata.features).prompt_comments === 'enabled',
    compress: bool(raw.compress_enabled),
    minimalCode: bool(raw.minimal_code_enabled),
    codingSkills: bool(raw.has_coding_skills),
    createdAt: str(raw.created_at),
    updatedAt: str(raw.updated_at),
  }
}

/** Only http(s) URLs become links or images (owner-controlled fields, plan §6.5). */
export function safeHttpUrl(v: string | null | undefined): string | null {
  if (!v) return null
  try {
    const u = new URL(v)
    return u.protocol === 'http:' || u.protocol === 'https:' ? u.toString() : null
  } catch {
    return null
  }
}

/** Images (agent icons): https only. */
export function safeHttpsUrl(v: string | null | undefined): string | null {
  const u = safeHttpUrl(v)
  return u?.startsWith('https:') ? u : null
}

export const isUuid = (v: string): boolean =>
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(v)
