/**
 * The MCP gateway mock (plans/feat-mcp.md §8), shaped as the `nasiko-mcp-gateway` crate's views at nasiko-cloud-rs
 * `2d6178e4` (`connectors.rs`, `catalog.rs`, `permissions.rs`, `oss/server/src/mcp/handlers/*`):
 * - every answer is the `{data, status_code, message}` envelope, errors too, with `McpError`'s status and
 *   `client_message` (`"not found: …"`, `"conflict: …"`; an unreachable probe is a 502 `backend error`);
 * - reachability is `authorizer.can_access_connector`: toolkits for everyone, else owner, a user grant, or public;
 * - an agent's server list keeps only servers the caller connected, plus no-auth ones (`list_connectors_view`);
 * - `PUT /tools` replaces each named connector's rules, creating an enabled access row when there was none.
 *
 * Ids: connectors `5eed000e-*`, builds `5eed000f-*`. The seeded upload `crm-sync` finishes 20 s after the state is
 * built; an upload started here builds for 15 s (`?mock=mcp-upload-fails` fails it).
 */
import { http, HttpResponse, type HttpHandler } from 'msw'
import type { AgentsState } from './agents'
import { readMultipart } from './deploy'
import type { HUser } from './seed-harness'
import { MCP_ACCESS, MCP_CONNECTIONS, MCP_CONNECTORS, mcpBuildId, mcpConnectorId } from './seed-mcp'

type Stance = 'allow' | 'ask' | 'block'
type AuthType = 'none' | 'bearer' | 'basic' | 'oauth2' | 'url_param'
const AUTH_TYPES: readonly string[] = ['none', 'bearer', 'basic', 'oauth2', 'url_param']
const STANCES: readonly string[] = ['allow', 'ask', 'block']

interface MockTool {
  name: string
  description: string
}

interface MockBuild {
  id: string
  version: string
  startedAt: number
  doneAt: number
  fails: boolean
  /** The version that built before this one: a failed build with none deletes its connector (build.rs). */
  prior: string | null
  error: string
  /** Tools the server exposes once it runs. */
  tools: MockTool[]
}

interface MockConnector {
  id: string
  provider: 'composio' | 'mcp_server'
  name: string
  display_name: string | null
  description: string | null
  url: string | null
  auth_type: AuthType | null
  url_param_name: string | null
  credential_header_name: string | null
  owner_id: string | null
  is_active: boolean
  source_kind: 'external_url' | 'uploaded_build'
  build: MockBuild | null
  created_at: string
  updated_at: string
  tools: MockTool[]
  public: boolean
  userGrants: { id: string; by: string; at: string }[]
  /** EE org-unit grants (`mcp_sharing.rs`); the OSS server has none. */
  unitGrants: { id: string; name: string; at: string }[]
}

interface Connection {
  credential: string | null
  oauthExpires: string | null
  active: boolean
}

export interface McpState {
  connectors: MockConnector[]
  /** `${userId}:${connectorId}` → the caller's connection (`mcp_user_connections`). */
  connections: Map<string, Connection>
  /** `${agentId}:${connectorId}` → `agent_connector_access` (enabled + tool rules). */
  access: Map<string, { enabled: boolean; rules: Record<string, Stance> }>
  serial: number
}

export interface McpCtx {
  loggedIn(): boolean
  /** The signed-in user (id, superuser flag). */
  me(): { id: string; is_superuser: boolean }
  users(): HUser[]
  agents(): AgentsState
  now(): number
  hasVariant(v: string): boolean
  state(): McpState
}

/** Build the state from the shared MCP seed (`seed-mcp.ts`), the agents mock's users and the live seed agents' ids. */
export function buildMcpState(
  agents: AgentsState,
  adminId: string,
  agentIds: readonly string[],
  now: number,
): McpState {
  const others = agents.users.filter((u) => u.id !== adminId && !u.service_account)
  const who = (o: 'admin' | 'other0' | 'other1' | null) =>
    o === null ? null : o === 'admin' ? adminId : (others[o === 'other0' ? 0 : 1]?.id ?? adminId)
  const at = (daysAgo: number) => new Date(now - daysAgo * 86_400_000).toISOString()
  let serial = 0
  const connectors: MockConnector[] = MCP_CONNECTORS.map((c) => {
    const owner = who(c.owner)
    const grantee = who(c.sharedWith)
    let build: MockBuild | null = null
    if (c.build) {
      const started = c.build.status === 'building' ? now : now - c.daysAgo * 86_400_000
      build = {
        id: mcpBuildId(++serial),
        version: c.build.version,
        startedAt: started,
        doneAt: started + (c.build.status === 'building' ? 20_000 : 90_000),
        fails: c.build.status === 'failed',
        prior: c.build.prior,
        error: c.build.error ?? '',
        tools: c.tools,
      }
    }
    return {
      id: mcpConnectorId(c.n),
      provider: c.provider,
      name: c.name,
      display_name: c.display_name,
      description: c.description,
      // A build in progress has no container yet (`settle` sets its URL and tools when it runs).
      url: c.build?.status === 'building' ? null : c.url,
      auth_type: c.auth_type,
      url_param_name: null,
      credential_header_name: c.credential_header_name,
      owner_id: owner,
      is_active: c.build?.status !== 'building',
      source_kind: c.source_kind,
      build,
      created_at: at(c.daysAgo),
      updated_at: at(Math.max(0, c.daysAgo - 1)),
      tools: c.build?.status === 'building' ? [] : c.tools,
      public: c.public,
      userGrants:
        grantee && grantee !== owner ? [{ id: grantee, by: owner ?? adminId, at: at(3) }] : [],
      unitGrants: [],
    }
  })
  const byName = (name: string) => mcpConnectorId(MCP_CONNECTORS.find((c) => c.name === name)!.n)
  const connections = new Map<string, Connection>(
    MCP_CONNECTIONS.map((c) => [
      `${adminId}:${byName(c.name)}`,
      { credential: c.credential, oauthExpires: null, active: true },
    ]),
  )
  const access = new Map<string, { enabled: boolean; rules: Record<string, Stance> }>()
  for (const a of MCP_ACCESS) {
    const agentId = agentIds[a.agent]
    if (agentId)
      access.set(`${agentId}:${byName(a.name)}`, { enabled: a.enabled, rules: { ...a.rules } })
  }
  return { connectors, connections, access, serial }
}

// ── Derived state ─────────────────────────────────────────────────────────────────────────────

type BuildState = 'pending' | 'building' | 'running' | 'failed'

/** `mcp/build.rs`: the row is `pending` until the worker claims it (2 s here), then `building`, then the outcome. */
function buildState(b: MockBuild, now: number): BuildState {
  if (now < b.startedAt + 2_000) return 'pending'
  if (now < b.doneAt) return 'building'
  return b.fails ? 'failed' : 'running'
}

/** A finished build turns the connector live once (`url`, `is_active`, synced tools), as the worker does. */
function settle(c: MockConnector, now: number) {
  if (!c.build || buildState(c.build, now) !== 'running' || c.url) return
  c.url = `http://mcp-${c.name}:8080/mcp`
  c.is_active = true
  c.tools = c.build.tools
}

function buildLogs(c: MockConnector, now: number): string[] {
  const b = c.build
  if (!b) return []
  const s = buildState(b, now)
  const lines = [
    `Cloning source for ${c.name} ${b.version}`,
    'Step 1/7 : FROM python:3.12-slim',
    'Step 2/7 : WORKDIR /app',
    'Step 3/7 : COPY . .',
  ]
  if (s === 'pending') return lines.slice(0, 1)
  if (s === 'building') return lines
  if (s === 'failed')
    return [
      ...lines,
      'Step 4/7 : RUN pip install -r requirements.txt',
      'ERROR: Could not find a version that satisfies the requirement mcp-sdk==9.9',
      b.error,
    ]
  return [
    ...lines,
    'Step 4/7 : RUN pip install -r requirements.txt',
    'Step 5/7 : EXPOSE 8080',
    'Step 6/7 : CMD ["python", "server.py"]',
    `Successfully tagged nasiko/mcp-${c.name}:${b.version}`,
    `MCP server listening on :8080 (${c.tools.length} tools)`,
  ]
}

const key = (a: string, b: string) => `${a}:${b}`

function canAccess(c: MockConnector, user: string): boolean {
  return (
    c.provider === 'composio' ||
    c.owner_id === user ||
    c.public ||
    c.userGrants.some((g) => g.id === user)
  )
}

// ── Responses (`mcp/mod.rs` ApiResponse / ApiError) ────────────────────────────────────────────

const ok = (data: unknown, message = 'ok', status = 200) =>
  HttpResponse.json({ data, status_code: status, message }, { status })

class McpHttpError extends Error {
  readonly status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}
const notFound = (what: string) => new McpHttpError(404, `not found: ${what}`)
const bad = (m: string) => new McpHttpError(400, `bad request: ${m}`)
const forbidden = (m: string) => new McpHttpError(403, `forbidden: ${m}`)

function isPublicUrl(raw: string): boolean {
  try {
    const u = new URL(raw)
    if (!['http:', 'https:'].includes(u.protocol)) return false
    return !/^(localhost|127\.|10\.|192\.168\.|169\.254\.|0\.0\.0\.0|\[::1\])/.test(u.hostname)
  } catch {
    return false
  }
}

export function mcpHandlers(ctx: McpCtx): HttpHandler[] {
  const run = async (fn: () => Response | Promise<Response>): Promise<Response> => {
    if (!ctx.loggedIn())
      return HttpResponse.json(
        { data: null, status_code: 401, message: 'missing or invalid token' },
        { status: 401 },
      )
    try {
      return await fn()
    } catch (err) {
      if (err instanceof McpHttpError)
        return HttpResponse.json(
          { data: null, status_code: err.status, message: err.message },
          { status: err.status },
        )
      throw err
    }
  }
  const s = () => {
    const st = ctx.state()
    for (const c of st.connectors) settle(c, ctx.now())
    // `delete_mcp_connector_or_mark_failed`: a first upload that fails leaves no connector behind.
    st.connectors = st.connectors.filter(
      (c) => !(c.build && !c.build.prior && buildState(c.build, ctx.now()) === 'failed'),
    )
    return st
  }
  const me = () => ctx.me()
  const reachable = (id: string) => {
    const c = s().connectors.find((x) => x.id === id)
    if (!c || !canAccess(c, me().id)) throw notFound(`connector '${id}' not found`)
    return c
  }
  const owned = (id: string) => {
    const c = s().connectors.find((x) => x.id === id)
    if (!c) throw notFound(`connector '${id}' not found`)
    if (c.owner_id !== me().id && !me().is_superuser)
      throw forbidden('this connector does not belong to you')
    return c
  }
  const connection = (c: MockConnector) => s().connections.get(key(me().id, c.id))
  const userLabel = (id: string | null) =>
    id ? (ctx.users().find((u) => u.id === id)?.username ?? null) : null

  const status = (c: MockConnector) => (c.build ? buildState(c.build, ctx.now()) : null)
  const dto = (c: MockConnector) => ({
    connector_id: c.id,
    provider_type: c.provider,
    owner_id: c.owner_id,
    name: c.name,
    url: c.url,
    transport: 'streamable_http',
    auth_type: c.auth_type,
    url_param_name: c.url_param_name,
    // The column's default (0003_mcp.sql).
    credential_header_name: c.credential_header_name ?? 'Authorization',
    description: c.description,
    display_name: c.display_name,
    logo_url: null,
    is_active: c.is_active,
    oauth_configured: c.auth_type === 'oauth2',
    source_kind: c.source_kind,
    build_status: status(c),
    setup_status: null,
    setup_error: null,
    created_at: c.created_at,
    updated_at: c.updated_at,
  })
  const view = (c: MockConnector) => ({
    ...dto(c),
    is_owner: c.owner_id === me().id,
    version: c.build?.version ?? null,
    tool_count: c.tools.length,
    is_connected: !!connection(c)?.active,
    owner_username: userLabel(c.owner_id),
  })
  const label = (c: MockConnector) =>
    c.display_name ?? c.name.charAt(0).toUpperCase() + c.name.slice(1)

  const manageAgent = (agentId: string) => {
    const a = ctx.agents().agents.find((x) => x.id === agentId && !x.deleted)
    if (!a || (a.owner_id !== me().id && !me().is_superuser))
      throw forbidden('you do not have permission to manage this agent')
    return a
  }
  const accessReasons = (c: MockConnector) => {
    const users = ctx.users().filter((u) => !u.service_account && u.is_active)
    const row = (u: HUser, via: string): Record<string, unknown> => ({
      user_id: u.id,
      username: u.username,
      display_name: u.display_name,
      email: u.email,
      role: u.role,
      via,
      via_label: null,
    })
    const out = []
    for (const u of users) {
      if (u.id === c.owner_id) out.push(row(u, 'owner'))
      else if (c.userGrants.some((g) => g.id === u.id)) out.push(row(u, 'direct'))
      else if (c.unitGrants.some((g) => u.unit_ids.includes(g.id))) {
        // EE: inherited through a granted unit (direct membership here; the server walks the subtree).
        const g = c.unitGrants.find((x) => u.unit_ids.includes(x.id))
        out.push({ ...row(u, 'org_unit'), via_label: g?.name ?? null })
      } else if (c.public) out.push(row(u, 'public'))
    }
    return out
  }

  const P = '/api/mcp'
  // Static segments beside `/connectors/{id}` win on the server (axum): the `:id` handlers skip them.
  const STATIC = new Set(['probe', 'my-uploads', 'pinned', 'recent', 'upload', 'upload-github'])

  return [
    http.get(`${P}/connectors`, () =>
      run(() => {
        if (ctx.hasVariant('mcp-empty'))
          return ok({ created_by_you: [], shared_with_you: [], total: 0 })
        const mine = me().id
        const visible = s().connectors.filter(
          (c) => c.provider === 'mcp_server' && canAccess(c, mine),
        )
        const created = visible.filter((c) => c.owner_id === mine).map(view)
        const shared = visible.filter((c) => c.owner_id !== mine).map(view)
        return ok(
          {
            created_by_you: created,
            shared_with_you: shared,
            total: created.length + shared.length,
          },
          'Connectors retrieved successfully',
        )
      }),
    ),
    http.get(`${P}/composio/toolkits`, () =>
      run(() => {
        if (ctx.hasVariant('mcp-toolkits-fail'))
          throw new McpHttpError(502, 'composio error: upstream timed out')
        const list = ctx.hasVariant('mcp-empty')
          ? []
          : s()
              .connectors.filter((c) => c.provider === 'composio')
              .map((c) => ({
                connector_id: c.id,
                name: c.name,
                display_name: label(c),
                description: c.description,
                logo_url: null,
                auth_flow: 'oauth',
                tool_count: c.tools.length,
                is_connected: !!connection(c)?.active,
              }))
        return ok({ toolkits: list, total: list.length })
      }),
    ),
    http.post(`${P}/connectors/probe`, async ({ request }) =>
      run(async () => {
        const { url = '' } = (await request.json().catch(() => ({}))) as { url?: string }
        if (!isPublicUrl(url)) throw bad('URL must be a public http(s) address')
        if (/unreachable/.test(url))
          throw new McpHttpError(
            502,
            'backend error: could not reach MCP server: connection refused',
          )
        const clean = url.replace(/\/+$/, '')
        if (/oauth|atlassian/.test(url))
          return ok({
            url: clean,
            auth_type: 'oauth2',
            requires: 'oauth_flow',
            supports_dcr: true,
            hint: 'This server supports OAuth 2.1 with automatic client registration — no client credentials needed.',
          })
        if (/key|token|api/.test(url))
          return ok({
            url: clean,
            auth_type: 'bearer',
            requires: 'api_key_input',
            hint: 'This server requires a Bearer token or API key.',
            instructions: null,
          })
        return ok({
          url: clean,
          auth_type: 'none',
          requires: 'nothing',
          hint: 'This server requires no authentication.',
          instructions: null,
        })
      }),
    ),
    http.post(`${P}/connectors`, async ({ request }) =>
      run(async () => {
        const b = (await request.json().catch(() => ({}))) as Record<string, string | undefined>
        const name = (b.name ?? '').trim()
        const auth = b.auth_type ?? 'none'
        if (!name || !b.url) throw bad('invalid request body: missing field `name` or `url`')
        if (!AUTH_TYPES.includes(auth))
          throw bad(`auth_type must be one of ${JSON.stringify(AUTH_TYPES)}`)
        if (auth === 'url_param' && !b.url_param_name)
          throw bad("url_param_name is required when auth_type='url_param'")
        if (!isPublicUrl(b.url)) throw bad('URL must be a public http(s) address')
        const st = s()
        if (st.connectors.some((c) => c.owner_id === me().id && c.name === name))
          throw new McpHttpError(409, `conflict: you already have a connector named '${name}'`)
        const at = new Date(ctx.now()).toISOString()
        const c: MockConnector = {
          id: mcpConnectorId(1000 + ++st.serial),
          provider: 'mcp_server',
          name,
          display_name: b.display_name ?? null,
          description: b.description ?? null,
          url: b.url.trim(),
          auth_type: auth as AuthType,
          url_param_name: b.url_param_name ?? null,
          credential_header_name: b.credential_header_name ?? null,
          owner_id: me().id,
          is_active: true,
          source_kind: 'external_url',
          build: null,
          created_at: at,
          updated_at: at,
          // Tools sync on first use (`sync_connector_tools`); a fresh server lists none yet.
          tools: [],
          public: false,
          userGrants: [],
          unitGrants: [],
        }
        st.connectors.push(c)
        return ok(dto(c), 'Connector created successfully', 201)
      }),
    ),
    http.post(`${P}/connectors/upload`, async ({ request }) =>
      run(async () => {
        const fd = await readMultipart(request).catch(() => null)
        const name = String(fd?.get('name') ?? '').trim()
        if (!name) throw bad('name is required')
        if (!(fd?.get('file') instanceof Blob) && !(fd?.get('source') instanceof Blob))
          throw bad('source zip is required')
        return queue(name, String(fd?.get('version_tag') ?? '') || 'v1')
      }),
    ),
    http.post(`${P}/connectors/upload-github`, async ({ request }) =>
      run(async () => {
        const b = (await request.json().catch(() => ({}))) as Record<string, string | undefined>
        const name = (b.name ?? '').trim()
        if (!name) throw bad('name is required')
        let host = ''
        try {
          host = new URL(b.github_url ?? '').host
        } catch {
          /* checked below */
        }
        if (host !== 'github.com') throw bad('only https://github.com repositories can be cloned')
        return queue(name, b.version_tag || 'v1')
      }),
    ),
    http.get(`${P}/connectors/:id`, ({ params }) =>
      run(() => {
        const id = String(params.id)
        if (STATIC.has(id)) return new HttpResponse(null, { status: 405 })
        const c = reachable(id)
        const conn = connection(c)
        // `get_connector_view` has no top-level `version` (the list's; M-2): it is in `upload_info`.
        const { version: _listOnly, ...detail } = view(c)
        return ok(
          {
            ...detail,
            tools: c.tools,
            connection_count: [...s().connections.keys()].filter((k) => k.endsWith(c.id)).length,
            is_public: c.public,
            ...(c.build
              ? {
                  upload_info: {
                    upload_type: 'uploaded_build',
                    build_status: status(c),
                    version: c.build.version,
                    image_tag:
                      status(c) === 'running' ? `nasiko/mcp-${c.name}:${c.build.version}` : null,
                    error_msg: status(c) === 'failed' ? c.build.error : null,
                  },
                }
              : {}),
            has_credential: !!conn?.credential,
            agent_count: 0,
          },
          'Connector retrieved successfully',
        )
      }),
    ),
    http.patch(`${P}/connectors/:id`, async ({ params, request }) =>
      run(async () => {
        const c = owned(String(params.id))
        if (c.provider !== 'mcp_server') throw bad('only custom MCP connectors can be updated here')
        const b = (await request.json().catch(() => ({}))) as Record<string, unknown>
        if (typeof b.url === 'string' && !isPublicUrl(b.url))
          throw bad('URL must be a public http(s) address')
        for (const k of ['display_name', 'description', 'url'] as const)
          if (typeof b[k] === 'string') c[k] = b[k]
        if (typeof b.is_active === 'boolean') c.is_active = b.is_active
        c.updated_at = new Date(ctx.now()).toISOString()
        return ok(dto(c), 'Connector updated successfully')
      }),
    ),
    http.delete(`${P}/connectors/:id`, ({ params }) =>
      run(() => {
        const c = owned(String(params.id))
        const st = s()
        st.connectors = st.connectors.filter((x) => x !== c)
        for (const k of [...st.access.keys()]) if (k.endsWith(c.id)) st.access.delete(k)
        for (const k of [...st.connections.keys()]) if (k.endsWith(c.id)) st.connections.delete(k)
        return ok(null, 'Connector deleted successfully')
      }),
    ),
    http.get(`${P}/connectors/:id/build-status`, ({ params }) =>
      run(() => {
        const c = owned(String(params.id))
        if (!c.build) throw notFound('no build for this connector')
        return ok({
          build_status: status(c),
          error_msg: status(c) === 'failed' ? c.build.error : null,
          image_tag: status(c) === 'running' ? `nasiko/mcp-${c.name}:${c.build.version}` : null,
        })
      }),
    ),
    http.get(`${P}/connectors/:id/build-logs`, ({ params }) =>
      run(() =>
        ok(buildLogs(owned(String(params.id)), ctx.now()), 'build logs retrieved successfully'),
      ),
    ),

    // Connect / disconnect (`handlers/connect.rs`).
    http.post(`${P}/connect`, async ({ request }) =>
      run(async () => {
        const b = (await request.json().catch(() => ({}))) as {
          connector_id?: string
          credentials?: { value?: string }
        }
        const c = reachable(b.connector_id ?? '')
        const k = key(me().id, c.id)
        const st = s()
        const body = { connector_id: c.id, name: c.name }
        if (c.provider === 'composio') {
          // The popup completes the Composio flow; the mock counts it done at once.
          st.connections.set(k, { credential: null, oauthExpires: null, active: true })
          return ok(
            { status: 'initiated', ...body, oauth_url: 'about:blank#composio-oauth' },
            'OAuth flow initiated',
            201,
          )
        }
        if (c.auth_type === 'oauth2') {
          st.connections.set(k, {
            credential: null,
            oauthExpires: new Date(ctx.now() + 3_600_000).toISOString(),
            active: true,
          })
          return ok(
            { status: 'oauth_required', ...body, authorization_url: 'about:blank#mcp-oauth' },
            'OAuth authorization required',
          )
        }
        if (c.auth_type && c.auth_type !== 'none') {
          const value = b.credentials?.value?.trim()
          if (!value) throw bad(`'${c.name}' requires credentials.value`)
          st.connections.set(k, { credential: value, oauthExpires: null, active: true })
        } else st.connections.set(k, { credential: null, oauthExpires: null, active: true })
        return ok({ status: 'connected', ...body }, 'Service connected successfully')
      }),
    ),
    http.delete(`${P}/connections/:id`, ({ params }) =>
      run(() => {
        const k = key(me().id, String(params.id))
        if (!s().connections.delete(k)) throw notFound('no connection for this connector')
        return ok(
          { message: 'Disconnected', connector_id: params.id, composio_revoked: false },
          'Disconnected successfully',
        )
      }),
    ),

    // Credentials (`handlers/credentials.rs`): stored whatever the verification says.
    http.get(`${P}/connectors/:id/credential/status`, ({ params }) =>
      run(() => {
        const c = reachable(String(params.id))
        const conn = connection(c)
        return ok({
          connector_id: c.id,
          name: c.name,
          connected: !!conn?.credential,
          auth_type: conn?.credential ? c.auth_type : null,
        })
      }),
    ),
    http.post(`${P}/connectors/:id/credential`, async ({ params, request }) =>
      run(async () => {
        const c = reachable(String(params.id))
        const { value = '' } = (await request.json().catch(() => ({}))) as { value?: string }
        const verified = !/invalid/i.test(value)
        s().connections.set(key(me().id, c.id), {
          credential: value,
          oauthExpires: null,
          active: verified,
        })
        return ok(
          {
            connector_id: c.id,
            name: c.name,
            connected: verified,
            error: verified ? null : 'upstream returned 401 Unauthorized',
          },
          verified
            ? 'Credential registered and verified successfully'
            : 'Credential stored, but verification failed — see the error field',
          201,
        )
      }),
    ),
    http.delete(`${P}/connectors/:id/credential`, ({ params }) =>
      run(() => {
        const c = reachable(String(params.id))
        s().connections.delete(key(me().id, c.id))
        return ok(null, 'Credential deleted successfully')
      }),
    ),

    // OAuth 2.1 (`handlers/oauth.rs`): the mock's popup is about:blank, and authorizing completes at once.
    http.post(`${P}/connectors/:id/oauth/authorize`, ({ params }) =>
      run(() => {
        const c = reachable(String(params.id))
        if (c.auth_type !== 'oauth2') throw notFound(`OAuth connector '${c.id}' not found`)
        s().connections.set(key(me().id, c.id), {
          credential: null,
          oauthExpires: new Date(ctx.now() + 3_600_000).toISOString(),
          active: true,
        })
        return ok({ connector_id: c.id, name: c.name, authorization_url: 'about:blank#mcp-oauth' })
      }),
    ),
    http.get(`${P}/connectors/:id/oauth/status`, ({ params }) =>
      run(() => {
        const c = reachable(String(params.id))
        const conn = connection(c)
        return ok({
          connector_id: c.id,
          name: c.name,
          authorized: !!conn?.oauthExpires,
          expires_at: conn?.oauthExpires ?? null,
          scope: null,
        })
      }),
    ),
    http.delete(`${P}/connectors/:id/oauth/token`, ({ params }) =>
      run(() => {
        const c = reachable(String(params.id))
        if (!s().connections.delete(key(me().id, c.id))) throw notFound('no token to revoke')
        return ok(null, 'OAuth token revoked successfully')
      }),
    ),

    // Sharing (`handlers/sharing.rs`): owner or superuser.
    http.get(`${P}/connectors/:id/grants`, ({ params }) =>
      run(() => {
        const c = owned(String(params.id))
        return ok({
          grants: [
            ...c.userGrants.map((g, i) => ({
              id: `${c.id}-g${i}`,
              grant_type: 'user',
              grantee_id: g.id,
              granted_by: g.by,
              created_at: g.at,
            })),
            ...(c.public
              ? [
                  {
                    id: `${c.id}-pub`,
                    grant_type: 'public',
                    grantee_id: '*',
                    granted_by: c.owner_id,
                    created_at: c.created_at,
                  },
                ]
              : []),
          ],
          is_public: c.public,
          access_reasons: accessReasons(c),
        })
      }),
    ),
    http.post(`${P}/connectors/:id/grants/public`, ({ params }) =>
      run(() => {
        const c = owned(String(params.id))
        const wasNew = !c.public
        c.public = true
        const v = { id: `${c.id}-pub`, grant_type: 'public', grantee_id: '*', was_new: wasNew }
        return wasNew ? ok(v, 'Connector made public', 201) : ok(v, 'Connector is already public')
      }),
    ),
    http.delete(`${P}/connectors/:id/grants/public`, ({ params }) =>
      run(() => {
        owned(String(params.id)).public = false
        return ok(null, 'Public access revoked')
      }),
    ),
    http.post(`${P}/connectors/:id/grants/users/:userId`, ({ params }) =>
      run(() => {
        const c = owned(String(params.id))
        const uid = String(params.userId)
        if (!ctx.users().some((u) => u.id === uid)) throw notFound(`user '${uid}' not found`)
        const wasNew = !c.userGrants.some((g) => g.id === uid)
        if (wasNew)
          c.userGrants.push({ id: uid, by: me().id, at: new Date(ctx.now()).toISOString() })
        const v = { id: `${c.id}-${uid}`, grant_type: 'user', grantee_id: uid, was_new: wasNew }
        return wasNew
          ? ok(v, 'Connector shared with user', 201)
          : ok(v, 'Connector is already shared with this user')
      }),
    ),
    http.delete(`${P}/connectors/:id/grants/users/:userId`, ({ params }) =>
      run(() => {
        const c = owned(String(params.id))
        c.userGrants = c.userGrants.filter((g) => g.id !== String(params.userId))
        return ok(null, 'User access revoked')
      }),
    ),
    http.get(`${P}/share-targets`, ({ request }) =>
      run(() => {
        const q = (new URL(request.url).searchParams.get('q') ?? '').trim().toLowerCase()
        const users = ctx
          .users()
          .filter(
            (u) =>
              !u.service_account &&
              u.is_active &&
              q &&
              `${u.username} ${u.display_name}`.toLowerCase().includes(q),
          )
          .map((u) => ({ user_id: u.id, username: u.username, display_name: u.display_name }))
        return ok({ users }, 'Share targets retrieved successfully')
      }),
    ),
    http.get(`${P}/connectors/:id/consumers`, ({ params }) =>
      run(() => {
        const c = owned(String(params.id))
        const st = s()
        const agents = ctx
          .agents()
          .agents.filter((a) => !a.deleted && st.access.has(key(a.id, c.id)))
          .map((a) => {
            const row = st.access.get(key(a.id, c.id))!
            const blocked = Object.values(row.rules).filter((v) => v === 'block').length
            return {
              agent_id: a.id,
              agent_name: a.name,
              agent_display_name: a.display_name,
              agent_owner_id: a.owner_id,
              owner_username: userLabel(a.owner_id),
              enabled: row.enabled,
              tools_used: row.enabled ? Math.max(0, c.tools.length - blocked) : 0,
              total_tools: c.tools.length,
              tool_rules: Object.entries(row.rules).map(([pattern, stance]) => ({
                pattern,
                stance,
              })),
            }
          })
        return ok(
          {
            agents,
            users: c.userGrants.map((g) => ({
              user_id: g.id,
              username: userLabel(g.id),
              display_name: ctx.users().find((u) => u.id === g.id)?.display_name ?? null,
              granted_by: g.by,
              granted_by_username: userLabel(g.by),
              created_at: g.at,
            })),
            teams: c.unitGrants.map((u) => ({
              id: u.id,
              name: u.name,
              granted_by: null,
              created_at: u.at,
            })),
            departments: [],
          },
          'Connector consumers retrieved successfully',
        )
      }),
    ),

    // Per-agent access (`handlers/permissions.rs`).
    http.get(`${P}/agents/:agentId/connectors`, ({ params }) =>
      run(() => {
        const agentId = String(params.agentId)
        manageAgent(agentId)
        const st = s()
        const connectors = st.connectors
          .filter((c) => canAccess(c, me().id))
          .filter((c) => !!connection(c)?.active || c.auth_type === 'none')
          .map((c) => ({
            connector_id: c.id,
            provider_type: c.provider,
            name: c.name,
            display_name: label(c),
            description: c.description,
            logo_url: null,
            enabled: st.access.get(key(agentId, c.id))?.enabled ?? false,
            connected: true,
          }))
        return ok({ connectors }, 'Agent connectors retrieved successfully')
      }),
    ),
    http.put(`${P}/agents/:agentId/connectors/:connectorId`, async ({ params, request }) =>
      run(async () => {
        const agentId = String(params.agentId)
        manageAgent(agentId)
        const c = reachable(String(params.connectorId))
        const { enabled } = (await request.json().catch(() => ({}))) as { enabled?: unknown }
        if (typeof enabled !== 'boolean') throw bad('invalid request body: missing field `enabled`')
        const st = s()
        const row = st.access.get(key(agentId, c.id))
        st.access.set(key(agentId, c.id), { enabled, rules: row?.rules ?? {} })
        return ok({ connector_id: c.id, enabled }, 'Connector access updated successfully')
      }),
    ),
    http.get(`${P}/agents/:agentId/connectors/:connectorId/tools`, ({ params }) =>
      run(() => {
        const agentId = String(params.agentId)
        manageAgent(agentId)
        const c = reachable(String(params.connectorId))
        const rules = s().access.get(key(agentId, c.id))?.rules ?? {}
        return ok(
          {
            tools: c.tools.map((t) => ({
              ...t,
              stance: rules[t.name] ?? 'allow',
              last_synced_at: c.updated_at,
            })),
          },
          'Connector tools retrieved successfully',
        )
      }),
    ),
    http.get(`${P}/agents/:agentId/tools`, ({ params }) =>
      run(() => {
        const agentId = String(params.agentId)
        manageAgent(agentId)
        const rules = [...s().access.entries()]
          .filter(([k]) => k.startsWith(`${agentId}:`))
          .flatMap(([k, row]) =>
            Object.entries(row.rules).map(([tool_pattern, stance]) => ({
              connector_id: k.slice(agentId.length + 1),
              tool_pattern,
              stance,
            })),
          )
        return ok({ rules }, 'Tool rules retrieved successfully')
      }),
    ),
    http.put(`${P}/agents/:agentId/tools`, async ({ params, request }) =>
      run(async () => {
        const agentId = String(params.agentId)
        manageAgent(agentId)
        const { rules = [] } = (await request.json().catch(() => ({}))) as {
          rules?: { connector_id: string; tool_pattern: string; stance: string }[]
        }
        if (rules.some((r) => !STANCES.includes(r.stance)))
          throw bad(`stance must be one of ${JSON.stringify(STANCES)}`)
        const byConnector = new Map<string, Record<string, Stance>>()
        for (const r of rules) {
          reachable(r.connector_id)
          byConnector.set(r.connector_id, {
            ...byConnector.get(r.connector_id),
            [r.tool_pattern]: r.stance as Stance,
          })
        }
        const st = s()
        for (const [connectorId, next] of byConnector) {
          const row = st.access.get(key(agentId, connectorId))
          st.access.set(key(agentId, connectorId), { enabled: row?.enabled ?? true, rules: next })
        }
        return ok({ rules }, 'Tool rules updated successfully')
      }),
    ),
  ]

  /** `mcp/build.rs queue_*_upload`: writes the `pending` row before answering 202. */
  function queue(name: string, version: string): Response {
    const st = s()
    if (st.connectors.some((c) => c.owner_id === me().id && c.name === name))
      throw new McpHttpError(409, `conflict: you already have a connector named '${name}'`)
    const now = ctx.now()
    const at = new Date(now).toISOString()
    const build: MockBuild = {
      id: mcpBuildId(++st.serial),
      version,
      startedAt: now,
      doneAt: now + 15_000,
      fails: ctx.hasVariant('mcp-upload-fails'),
      prior: null,
      error: 'docker build failed: no Dockerfile or pyproject.toml at the archive root',
      tools: [
        { name: 'echo', description: 'Echo the input back.' },
        { name: 'ping', description: 'Health check.' },
      ],
    }
    const c: MockConnector = {
      id: mcpConnectorId(1000 + ++st.serial),
      provider: 'mcp_server',
      name,
      display_name: null,
      description: null,
      url: null,
      auth_type: 'none',
      url_param_name: null,
      credential_header_name: null,
      owner_id: me().id,
      is_active: false,
      source_kind: 'uploaded_build',
      build,
      created_at: at,
      updated_at: at,
      tools: [],
      public: false,
      userGrants: [],
      unitGrants: [],
    }
    st.connectors.push(c)
    return ok({ connector_id: c.id, build_id: build.id }, 'MCP server build queued', 202)
  }
}
