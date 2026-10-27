/**
 * Pure MCP logic (plans/feat-mcp.md), tested in logic.test.ts: the catalog's one card model over custom servers and
 * toolkits, its filters, server status, the upload name rule, the register body, and tool-rule summaries.
 */
import type { Tab, View } from './search'
import type {
  AgentTool,
  AuthFlow,
  AuthType,
  Connector,
  ConnectorList,
  Stance,
  Toolkit,
} from './types'

/** `catalog.rs auth_flow_for` for a custom server. */
export function authFlowOf(authType: string | null | undefined): AuthFlow {
  if (authType === 'oauth2') return 'oauth'
  if (authType === 'bearer' || authType === 'basic' || authType === 'url_param') return 'api_key'
  return 'none'
}

/** Upload builds only: Building until the image runs, Build failed after. Others are Active or Inactive. */
export type ServerStatus = 'active' | 'inactive' | 'building' | 'failed'

export function serverStatus(
  c: Pick<Connector, 'source_kind' | 'build_status' | 'is_active'>,
): ServerStatus {
  if (c.source_kind === 'uploaded_build') {
    if (c.build_status === 'pending' || c.build_status === 'building') return 'building'
    if (c.build_status === 'failed') return 'failed'
  }
  return c.is_active ? 'active' : 'inactive'
}

/** One catalog card: a custom MCP server or a platform toolkit. */
export interface Service {
  id: string
  kind: 'server' | 'toolkit'
  name: string
  label: string
  description: string | null
  logoUrl: string | null
  authFlow: AuthFlow
  authType: string | null
  toolCount: number
  connected: boolean
  /** A custom server someone else owns. */
  shared: boolean
  owner: string | null
  version: string | null
  status: ServerStatus | null
}

export const labelOf = (c: { name: string; display_name?: string | null }) =>
  c.display_name || c.name

/** Both kinds in one list, sorted by display name (legacy `#services`). */
export function toServices(list: ConnectorList | undefined, toolkits: Toolkit[]): Service[] {
  const server = (c: Connector, shared: boolean): Service => ({
    id: c.connector_id,
    kind: 'server',
    name: c.name,
    label: labelOf(c),
    description: c.description ?? null,
    logoUrl: c.logo_url ?? null,
    authFlow: authFlowOf(c.auth_type),
    authType: c.auth_type ?? null,
    toolCount: c.tool_count,
    connected: c.is_connected,
    shared,
    owner: c.owner_username ?? null,
    version: c.version ?? null,
    status: serverStatus(c),
  })
  const all: Service[] = [
    ...toolkits.map((t): Service => ({
      id: t.connector_id,
      kind: 'toolkit',
      name: t.name,
      label: labelOf(t),
      description: t.description ?? null,
      logoUrl: t.logo_url ?? null,
      authFlow: t.auth_flow,
      authType: null,
      toolCount: t.tool_count,
      connected: t.is_connected,
      shared: false,
      owner: null,
      version: null,
      status: null,
    })),
    ...(list?.created_by_you ?? []).map((c) => server(c, false)),
    ...(list?.shared_with_you ?? []).map((c) => server(c, true)),
  ]
  return all.sort((a, b) => a.label.localeCompare(b.label))
}

export function inView(s: Service, view: View | undefined): boolean {
  if (view === 'yours') return s.kind === 'server' && !s.shared
  if (view === 'shared') return s.kind === 'server' && s.shared
  if (view === 'toolkits') return s.kind === 'toolkit'
  return true
}

export const inTab = (s: Service, tab: Tab) =>
  tab === 'all' || (tab === 'connected' ? s.connected : !s.connected)

export function tabCounts(scoped: readonly Service[]): Record<Tab, number> {
  const connected = scoped.filter((s) => s.connected).length
  return { all: scoped.length, available: scoped.length - connected, connected }
}

export function matchesQuery(s: Service, q: string): boolean {
  const term = q.trim().toLowerCase()
  if (!term) return true
  return [s.label, s.name, s.description ?? ''].join(' ').toLowerCase().includes(term)
}

/** Custom servers only have a page; building and failed ones have no Connect (legacy card). */
export const canConnect = (s: Service) => s.status !== 'building' && s.status !== 'failed'

export const isBuilding = (list: ConnectorList | undefined) =>
  [...(list?.created_by_you ?? []), ...(list?.shared_with_you ?? [])].some(
    (c) => serverStatus(c) === 'building',
  )

/**
 * Uploads that can't give an agent tools yet (building or build failed), by id. The agent's server list includes
 * every no-auth server whatever its build (M-9), and its rows carry no build state, so the catalog list supplies it.
 */
export function unusable(list: ConnectorList | undefined): Map<string, 'building' | 'failed'> {
  const out = new Map<string, 'building' | 'failed'>()
  for (const c of [...(list?.created_by_you ?? []), ...(list?.shared_with_you ?? [])]) {
    const st = serverStatus(c)
    if (st === 'building' || st === 'failed') out.set(c.connector_id, st)
  }
  return out
}

/** The legacy upload dialog's rule: the zip's file name, lowercased, to `[a-z0-9._-]`, at most 128 characters. */
export function nameFromFile(fileName: string): string {
  return fileName
    .replace(/\.zip$/i, '')
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^[^a-z0-9_]+/, '')
    .replace(/-{2,}/g, '-')
    .replace(/-+$/, '')
    .slice(0, 128)
}

export interface RegisterValues {
  name: string
  display_name: string
  url: string
  auth_type: AuthType
  credential_header_name: string
  basic_username: string
  basic_password: string
  oauth_client_id: string
  oauth_client_secret: string
  url_param_name: string
  description: string
}

/** Which optional fields each auth type shows (and sends). */
const AUTH_FIELDS: Record<AuthType, (keyof RegisterValues)[]> = {
  none: [],
  bearer: ['credential_header_name'],
  basic: ['basic_username', 'basic_password'],
  oauth2: ['oauth_client_id', 'oauth_client_secret'],
  url_param: ['url_param_name'],
}

/** `CreateConnector`: required fields, then only non-empty optional ones for the chosen auth type (legacy). */
export function registerBody(v: RegisterValues): Record<string, string> {
  const body: Record<string, string> = {
    name: v.name.trim(),
    url: v.url.trim(),
    auth_type: v.auth_type,
  }
  for (const key of ['display_name', 'description', ...AUTH_FIELDS[v.auth_type]] as const) {
    const value = v[key].trim()
    if (value) body[key] = value
  }
  return body
}

export const authFields = (t: AuthType) => AUTH_FIELDS[t]

/** "3 of 5 tools allowed · 1 ask first" parts (legacy agent page summary). */
export function stanceCounts(tools: readonly Pick<AgentTool, 'stance'>[]) {
  return {
    total: tools.length,
    allowed: tools.filter((t) => t.stance === 'allow').length,
    asks: tools.filter((t) => t.stance === 'ask').length,
  }
}

/**
 * The connector's whole rule set with one tool changed: `PUT /tools` replaces the rules of each connector it names,
 * so a single click must re-send them all (legacy `#setToolStance`).
 */
export function rulesWith(
  connectorId: string,
  tools: readonly Pick<AgentTool, 'name' | 'stance'>[],
  tool: string,
  stance: Stance,
) {
  return tools.map((t) => ({
    connector_id: connectorId,
    tool_pattern: t.name,
    stance: t.name === tool ? stance : t.stance,
  }))
}

/** `AccessReason.via` → the Grant column (legacy labels; team/department/org_unit are EE inheritance). */
export function grantKind(via: string): 'owner' | 'direct' | 'inherited' | 'public' {
  if (via === 'owner' || via === 'direct' || via === 'public') return via
  return 'inherited'
}

/** The OAuth page opens only over https (`about:` too: the mock's, it can't run script). */
export function safePopupUrl(url: string | undefined): string | null {
  if (!url) return null
  try {
    return ['https:', 'about:'].includes(new URL(url).protocol) ? url : null
  } catch {
    return null
  }
}
