/**
 * LLM router mock state (plan §6): configs, per-agent routing, user secret names, custom providers and the tier
 * registry, derived from the agents mock and mutated by the `router` handlers. Behaviour mirrors nasiko-server
 * @ cb3aaf0c so the page meets the server's quirks in mock mode too:
 * - configs (`server/src/llm_configs.rs`): scoped to `created_by`; PATCH merges with COALESCE (null keeps the
 *   value, `[]` and `false` apply); a named secret is upserted BEFORE the config write (ensure_secret), so a
 *   failing save can still have replaced the key; create with `is_default` swaps the default; soft delete clears
 *   `is_default` and answers a plain-text 409 while live agents are attached.
 * - agent routing (`server/src/agents/llm_config.rs`): owner or superuser; an attach or a detach clears the agent
 *   pin; `pinned_model` applies only when `llm_config_id` is absent; `inbound_format` is checked after the writes.
 * - custom providers (`server/src/llm_router/custom_providers.rs`): writes are superuser-only; delete answers a
 *   JSON 409 `{message, referencing_configs}`.
 * Ids are `5eed0007-*` (configs) and `5eed0008-*` (custom providers), so the live seed can use the same ones.
 */
import type {
  AgentUsage,
  AttachBody,
  CreateConfigBody,
  CreateCustomProviderBody,
  CustomProvider,
  ProviderKind,
  LlmConfig,
  ModelMapping,
  RoutingConfig,
  SecretEntry,
  UpdateConfigBody,
  UpdateCustomProviderBody,
} from '@/features/router/types'
import type { MockAgent } from './agents'
import { MockHttpError } from './aggregate'
import type { Seed } from './seed'
import { ADMIN_ID } from './seed-harness'

export interface MockConfig extends LlmConfig {
  created_by: string
  deleted: boolean
}

interface MockRouting {
  llm_config_id: string | null
  pinned_model: string | null
  inbound_format: string
}

interface MockCustomProvider extends CustomProvider {
  deleted: boolean
  models: string[]
}

export interface RouterState {
  configs: MockConfig[]
  routing: Map<string, MockRouting>
  secrets: Map<string, SecretEntry[]>
  custom: MockCustomProvider[]
  registry: ModelMapping[]
}

const SUPPORTED_PROVIDERS = ['openai', 'anthropic', 'gemini', 'openrouter']
const INBOUND = ['openai', 'anthropic', 'gemini']
const pad = (n: number) => String(n).padStart(12, '0')
export const configId = (n: number) => `5eed0007-0000-4000-8000-${pad(n)}`
const customId = (n: number) => `5eed0008-0000-4000-8000-${pad(n)}`

/** The seed's routing layout. Agents are the admin's own, in list order (created_at DESC is irrelevant here). */
export function buildRouterState(
  agents: readonly MockAgent[],
  now: number,
  opts: { empty?: boolean; legacy?: boolean } = {},
): RouterState {
  const iso = new Date(now - 7 * 86_400_000).toISOString()
  const mk = (
    n: number,
    created_by: string,
    c: Partial<LlmConfig> & Pick<LlmConfig, 'name' | 'provider'>,
  ): MockConfig => ({
    id: configId(n),
    model: null,
    fallback_models: [],
    temperature: null,
    max_tokens: null,
    api_key_secret_name: null,
    pinned: false,
    pinned_model: null,
    tier1_model: null,
    tier2_model: null,
    tier3_model: null,
    is_default: false,
    created_at: iso,
    updated_at: iso,
    created_by,
    deleted: false,
    ...c,
  })
  const others = [
    ...new Set(agents.filter((a) => !a.deleted && a.owner_id !== ADMIN_ID).map((a) => a.owner_id)),
  ]
  const configs: MockConfig[] = opts.empty
    ? []
    : [
        mk(1, ADMIN_ID, {
          name: 'anthropic-default',
          provider: 'anthropic',
          model: 'claude-sonnet-4',
          fallback_models: ['openai/gpt-4o-mini'],
          temperature: 0.3,
          api_key_secret_name: 'ANTHROPIC_API_KEY',
          is_default: true,
        }),
        mk(2, ADMIN_ID, {
          name: 'research-tiers',
          provider: 'openai',
          model: 'gpt-4o-mini',
          tier1_model: 'gpt-4o',
          tier2_model: 'gpt-4o-mini',
          tier3_model: 'gpt-4o-mini',
          fallback_models: ['gpt-4o-mini'],
          api_key_secret_name: 'OPENAI_API_KEY',
        }),
        mk(3, ADMIN_ID, {
          name: 'fast-openai',
          provider: 'openai',
          model: 'gpt-4o-mini',
          pinned: true,
          pinned_model: 'gpt-4o-mini',
          max_tokens: 1024,
        }),
        mk(4, ADMIN_ID, {
          name: 'local-models',
          provider: 'custom',
          model: 'custom-local',
          fallback_models: ['anthropic/claude-sonnet-4'],
        }),
      ]
  if (!opts.empty && others[0])
    configs.push(
      mk(5, others[0], {
        name: 'team-default',
        provider: 'anthropic',
        model: 'claude-sonnet-4',
        api_key_secret_name: 'ANTHROPIC_API_KEY',
        is_default: true,
      }),
    )
  if (opts.legacy) {
    // Data made outside the page (CLI, legacy UI): the page must show it as stored, with warnings (DX #9).
    configs.push(
      mk(6, ADMIN_ID, {
        name: 'cli-openrouter',
        provider: 'openrouter',
        model: 'meta-llama/llama-3-70b',
      }),
      mk(7, ADMIN_ID, {
        name: 'lost-key',
        provider: 'openai',
        model: 'gpt-4o',
        api_key_secret_name: 'DELETED_KEY',
      }),
      mk(8, ADMIN_ID, { name: 'old-endpoint', provider: 'retired-llm', model: 'retired-1' }),
      mk(9, ADMIN_ID, {
        name: 'off-catalog',
        provider: 'anthropic',
        model: 'claude-9-preview',
        fallback_models: ['openai/gpt-7'],
      }),
    )
  }
  const routing = new Map<string, MockRouting>()
  const mine = agents.filter((a) => !a.deleted && a.owner_id === ADMIN_ID)
  const a2a = mine.filter((a) => !a.tags.includes('coding-agent'))
  const harness = mine.find((a) => a.tags.includes('coding-agent'))
  for (const a of agents)
    routing.set(a.id, { llm_config_id: null, pinned_model: null, inbound_format: 'openai' })
  if (!opts.empty) {
    const set = (a: MockAgent | undefined, r: Partial<MockRouting>) => {
      if (a) routing.set(a.id, { ...routing.get(a.id)!, ...r })
    }
    set(a2a[0], { llm_config_id: configId(2) })
    set(a2a[1], { llm_config_id: configId(2), pinned_model: 'gpt-4o' })
    set(a2a[2], { llm_config_id: configId(3) })
    set(a2a[3], { llm_config_id: configId(4) })
    set(harness, { llm_config_id: configId(1), inbound_format: 'anthropic' })
    if (opts.legacy) set(a2a[4], { llm_config_id: configId(7) })
  }
  const secret = (name: string): SecretEntry => ({
    id: `5eed0009-0000-4000-8000-${pad(name.length)}`,
    name,
    created_at: iso,
    updated_at: iso,
  })
  return {
    configs,
    routing,
    secrets: new Map([
      [ADMIN_ID, [secret('ANTHROPIC_API_KEY'), secret('OPENAI_API_KEY')]],
      ...others.map((o) => [o, [secret('ANTHROPIC_API_KEY')]] as [string, SecretEntry[]]),
    ]),
    custom: [
      {
        id: customId(1),
        label: 'custom',
        display_name: 'Custom',
        base_url: 'http://localhost:11434/v1',
        kind: 'openai',
        api_version: null,
        default_model: 'custom-local',
        catalog_sync_enabled: true,
        api_key_set: true,
        last_sync_at: new Date(now - 3_600_000).toISOString(),
        last_sync_status: 'ok',
        last_sync_error: null,
        created_at: iso,
        deleted: false,
        models: ['custom-local'],
      },
    ],
    registry: [
      { provider: 'anthropic', tier: 1, model: 'claude-sonnet-4' },
      { provider: 'anthropic', tier: 2, model: 'claude-sonnet-4' },
      { provider: 'anthropic', tier: 3, model: 'claude-sonnet-4' },
      { provider: 'openai', tier: 1, model: 'gpt-4o' },
      { provider: 'openai', tier: 2, model: 'gpt-4o-mini' },
      { provider: 'openai', tier: 3, model: 'gpt-4o-mini' },
    ],
  }
}

/** A config as `CONFIG_JSON` returns it (no owner or deleted flag). */
function configJson(c: MockConfig): LlmConfig {
  const { created_by: _o, deleted: _d, ...wire } = c
  return wire
}

/** The nested config in the agent routing response: CONFIG_JSON (agents/llm_config.rs, ea233d20) has no timestamps. */
function routingConfigJson(c: MockConfig): RoutingConfig {
  const { created_at: _c, updated_at: _u, ...rest } = configJson(c)
  return rest
}

// ─── configs ────────────────────────────────────────────────────────────────

const live = (s: RouterState, user: string) =>
  s.configs.filter((c) => c.created_by === user && !c.deleted)

export function listConfigs(s: RouterState, user: string): LlmConfig[] {
  return live(s, user)
    .sort((a, b) => a.name.localeCompare(b.name))
    .map(configJson)
}

function findConfig(s: RouterState, id: string, user: string): MockConfig {
  const c = live(s, user).find((x) => x.id === id)
  if (!c) throw new MockHttpError(404, 'llm config not found')
  return c
}

/** `resolve_provider_label`: a built-in name, a custom label, or a custom provider UUID → its label. */
function providerLabel(s: RouterState, provider: string): string {
  const custom = s.custom.find((p) => !p.deleted && (p.id === provider || p.label === provider))
  if (custom) return custom.label
  if (SUPPORTED_PROVIDERS.includes(provider)) return provider
  throw new MockHttpError(
    400,
    `unsupported provider '${provider}' (expected a built-in [${SUPPORTED_PROVIDERS.join(', ')}] or a registered custom provider)`,
  )
}

/** `ensure_secret`: upsert when a value comes with the name; a missing name without a value is a 400. Runs BEFORE the config write. */
function ensureSecret(
  s: RouterState,
  user: string,
  name: string | null | undefined,
  value: string | null | undefined,
  now: number,
) {
  if (!name) return
  const list = s.secrets.get(user) ?? []
  const stamp = new Date(now).toISOString()
  if (value) {
    const hit = list.find((x) => x.name === name)
    s.secrets.set(
      user,
      hit
        ? list.map((x) => (x.name === name ? { ...x, updated_at: stamp } : x))
        : [
            ...list,
            {
              id: `5eed0009-0000-4000-8000-${pad(list.length + 100)}`,
              name,
              created_at: stamp,
              updated_at: stamp,
            },
          ],
    )
    return
  }
  if (!list.some((x) => x.name === name))
    throw new MockHttpError(400, `secret '${name}' not found; provide secret_value to store it`)
}

function validateModel(
  model: string | null | undefined,
  hasTier: boolean,
  pinned: string | null | undefined,
) {
  if (model !== undefined && model !== null && !model.trim())
    throw new MockHttpError(400, 'model must not be empty when provided')
  if ((model === undefined || model === null) && !hasTier)
    throw new MockHttpError(
      400,
      'either model or at least one tier model (tier1_model, tier2_model, tier3_model) is required',
    )
  if (pinned !== undefined && pinned !== null && !pinned.trim())
    throw new MockHttpError(400, 'pinned_model must not be empty')
}

export function createConfig(
  s: RouterState,
  user: string,
  body: CreateConfigBody,
  now: number,
): LlmConfig {
  if (!body.name?.trim()) throw new MockHttpError(400, 'name must not be empty')
  const provider = providerLabel(s, body.provider)
  validateModel(
    body.model,
    !!(body.tier1_model || body.tier2_model || body.tier3_model),
    body.pinned_model,
  )
  if (live(s, user).some((c) => c.name === body.name))
    throw new MockHttpError(409, `an LLM config named '${body.name}' already exists`)
  ensureSecret(s, user, body.api_key_secret_name, body.secret_value, now)
  if (body.is_default) for (const c of live(s, user)) c.is_default = false
  const stamp = new Date(now).toISOString()
  const c: MockConfig = {
    id: configId(100 + s.configs.length),
    name: body.name,
    provider,
    model: body.model ?? null,
    fallback_models: body.fallback_models ?? [],
    temperature: body.temperature ?? null,
    max_tokens: body.max_tokens ?? null,
    api_key_secret_name: body.api_key_secret_name ?? null,
    pinned: body.pinned ?? false,
    pinned_model: body.pinned_model ?? null,
    tier1_model: body.tier1_model ?? null,
    tier2_model: body.tier2_model ?? null,
    tier3_model: body.tier3_model ?? null,
    is_default: body.is_default ?? false,
    created_at: stamp,
    updated_at: stamp,
    created_by: user,
    deleted: false,
  }
  s.configs.push(c)
  return configJson(c)
}

/** COALESCE merge: a field sent as null (or absent) keeps its value. */
export function updateConfig(
  s: RouterState,
  id: string,
  user: string,
  body: UpdateConfigBody,
  now: number,
): LlmConfig {
  const c = findConfig(s, id, user)
  const provider = body.provider ? providerLabel(s, body.provider) : null
  if (body.model !== undefined && body.model !== null && !body.model.trim())
    throw new MockHttpError(400, 'model must not be empty when provided')
  if (body.pinned_model !== undefined && body.pinned_model !== null && !body.pinned_model.trim())
    throw new MockHttpError(400, 'pinned_model must not be empty')
  if (body.name !== undefined && body.name !== null) {
    if (!body.name.trim()) throw new MockHttpError(400, 'name must not be empty')
    if (live(s, user).some((x) => x.name === body.name && x.id !== id))
      throw new MockHttpError(409, `an LLM config named '${body.name}' already exists`)
  }
  ensureSecret(s, user, body.api_key_secret_name, body.secret_value, now)
  const keep = <T>(v: T | null | undefined, was: T): T => (v === null || v === undefined ? was : v)
  Object.assign(c, {
    name: keep(body.name, c.name),
    provider: provider ?? c.provider,
    model: keep(body.model, c.model),
    fallback_models: keep(body.fallback_models, c.fallback_models),
    temperature: keep(body.temperature, c.temperature),
    max_tokens: keep(body.max_tokens, c.max_tokens),
    api_key_secret_name: keep(body.api_key_secret_name, c.api_key_secret_name),
    pinned: keep(body.pinned, c.pinned),
    pinned_model: keep(body.pinned_model, c.pinned_model),
    tier1_model: keep(body.tier1_model, c.tier1_model),
    tier2_model: keep(body.tier2_model, c.tier2_model),
    tier3_model: keep(body.tier3_model, c.tier3_model),
    updated_at: new Date(now).toISOString(),
  })
  return configJson(c)
}

export function deleteConfig(
  s: RouterState,
  id: string,
  user: string,
  liveAgents: ReadonlySet<string>,
  forceInUse?: number,
) {
  const c = findConfig(s, id, user)
  const n =
    forceInUse ??
    [...s.routing.entries()].filter(([agent, r]) => r.llm_config_id === id && liveAgents.has(agent))
      .length
  if (n > 0) throw new MockHttpError(409, `config is attached to ${n} agent(s); detach it first`)
  c.deleted = true
  c.is_default = false
}

export function setDefault(s: RouterState, id: string, user: string, on: boolean): LlmConfig {
  const c = findConfig(s, id, user)
  if (on) for (const x of live(s, user)) x.is_default = x.id === id
  else c.is_default = false
  return configJson(c)
}

// ─── agent routing ──────────────────────────────────────────────────────────

/** `resolve_agent_config`: attached (if still live) → the owner's default → none. */
export function resolveRouting(s: RouterState, agent: MockAgent) {
  const r = s.routing.get(agent.id) ?? {
    llm_config_id: null,
    pinned_model: null,
    inbound_format: 'openai',
  }
  const attached = r.llm_config_id
    ? s.configs.find((c) => c.id === r.llm_config_id && !c.deleted)
    : undefined
  const def = s.configs.find((c) => c.created_by === agent.owner_id && c.is_default && !c.deleted)
  const [config, source] = attached
    ? ([attached, 'attached'] as const)
    : def
      ? ([def, 'owner-default'] as const)
      : ([null, 'none'] as const)
  return {
    agent_id: agent.id,
    llm_config_id: r.llm_config_id,
    llm_config: config ? routingConfigJson(config) : null,
    source,
    inbound_format: r.inbound_format,
    pinned_model: r.pinned_model,
  }
}

/**
 * `update_llm_config`: attach/detach first (both clear the pin), then the pin only when `llm_config_id` was
 * absent, then `inbound_format`, each written as it goes (not atomic). `failPin` simulates the re-pin failing.
 */
export function patchRouting(
  s: RouterState,
  agent: MockAgent,
  body: AttachBody,
  opts: { failPin?: boolean } = {},
) {
  const r = {
    ...(s.routing.get(agent.id) ?? {
      llm_config_id: null,
      pinned_model: null,
      inbound_format: 'openai',
    }),
  }
  if ('llm_config_id' in body) {
    if (body.llm_config_id) {
      const ok = s.configs.some(
        (c) => c.id === body.llm_config_id && c.created_by === agent.owner_id && !c.deleted,
      )
      if (!ok) throw new MockHttpError(400, 'llm config not found or not owned by the agent owner')
      r.llm_config_id = body.llm_config_id
    } else {
      r.llm_config_id = null
    }
    r.pinned_model = null
    s.routing.set(agent.id, { ...r })
  } else if ('pinned_model' in body) {
    if (body.pinned_model !== null && body.pinned_model !== undefined && !body.pinned_model.trim())
      throw new MockHttpError(400, 'pinned_model must not be empty')
    if (opts.failPin) throw new MockHttpError(500, 'internal error')
    r.pinned_model = body.pinned_model ?? null
    s.routing.set(agent.id, { ...r })
  }
  if (body.inbound_format) {
    if (!INBOUND.includes(body.inbound_format))
      throw new MockHttpError(
        400,
        `unsupported inbound_format '${body.inbound_format}' (expected one of: ${INBOUND.join(', ')})`,
      )
    s.routing.set(agent.id, { ...r, inbound_format: body.inbound_format })
  }
  // LlmConfigUpdateResponse (agents/llm_config.rs): the GET's fields without inbound_format.
  const { inbound_format: _f, ...patched } = resolveRouting(s, agent)
  return patched
}

// ─── secrets, registry, custom providers ────────────────────────────────────

/** secrets/routes.rs `create_secret`: an upsert (a known name gets the new value and `updated_at`). */
export function upsertSecret(s: RouterState, user: string, name: string, now: number): SecretEntry {
  ensureSecret(s, user, name, 'value', now)
  return (s.secrets.get(user) ?? []).find((x) => x.name === name)!
}

/** `update_secret` / `delete_secret`: false when the caller has no secret by that name (a 404). */
export function touchSecret(s: RouterState, user: string, name: string, now: number): boolean {
  if (!(s.secrets.get(user) ?? []).some((x) => x.name === name)) return false
  ensureSecret(s, user, name, 'value', now)
  return true
}
export function deleteSecret(s: RouterState, user: string, name: string): boolean {
  const list = s.secrets.get(user) ?? []
  if (!list.some((x) => x.name === name)) return false
  s.secrets.set(
    user,
    list.filter((x) => x.name !== name),
  )
  return true
}

export const listSecrets = (s: RouterState, user: string): SecretEntry[] =>
  [...(s.secrets.get(user) ?? [])].sort((a, b) => a.name.localeCompare(b.name))

export const listCustom = (s: RouterState): CustomProvider[] =>
  s.custom.filter((p) => !p.deleted).map(({ deleted: _d, models: _m, ...wire }) => wire)

function slugify(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
}

/** `custom_providers.rs` `validate_dialect`: `kind` defaults to openai; Azure needs an api-version. */
export function validateDialect(kind: string | undefined, apiVersion: string | undefined) {
  const k = (kind ?? 'openai').trim()
  const v = apiVersion?.trim() || null
  if (k === 'openai' || k === 'bedrock-converse')
    return { kind: k as ProviderKind, api_version: null }
  if (k === 'azure-openai') {
    if (!v)
      throw new MockHttpError(
        400,
        "api_version is required for kind 'azure-openai' (e.g. 2024-10-21)",
      )
    return { kind: k as ProviderKind, api_version: v }
  }
  throw new MockHttpError(
    400,
    `unknown kind '${k}' (expected 'openai', 'azure-openai', or 'bedrock-converse')`,
  )
}

/** `dialect.rs` `normalize_base`: Azure accepts the portal's `…/openai` form too. */
const normalizeBase = (kind: string, base: string) => {
  const b = base.trim().replace(/\/+$/, '')
  return kind === 'azure-openai' ? b.replace(/\/openai$/, '').replace(/\/+$/, '') : b
}

export function createCustom(s: RouterState, body: CreateCustomProviderBody, now: number) {
  const dialect = validateDialect(body.kind, body.api_version)
  const display = body.display_name?.trim()
  const base = normalizeBase(dialect.kind, body.base_url ?? '')
  if (!display || !base) throw new MockHttpError(400, 'display_name and base_url are required')
  if (!body.api_key?.trim()) throw new MockHttpError(400, 'api_key is required')
  let label = slugify(display)
  if (['openai', 'anthropic', 'gemini'].includes(label)) label = `${label}-custom`
  let candidate = label
  for (let i = 2; s.custom.some((p) => !p.deleted && p.label === candidate); i++)
    candidate = `${label}-${i}`
  const p: MockCustomProvider = {
    id: customId(100 + s.custom.length),
    label: candidate,
    display_name: display,
    base_url: base,
    kind: dialect.kind,
    api_version: dialect.api_version,
    // `CreateRequest` has no default_model (4d57453c): serde drops it, so the mock does too.
    default_model: null,
    catalog_sync_enabled: body.catalog_sync_enabled,
    api_key_set: true,
    last_sync_at: null,
    last_sync_status: null,
    last_sync_error: null,
    created_at: new Date(now).toISOString(),
    deleted: false,
    models: [],
  }
  s.custom.push(p)
  return { id: p.id, label: p.label, discovered_models: 0, priced_models: 0 }
}

export function updateCustom(s: RouterState, id: string, body: UpdateCustomProviderBody) {
  const p = s.custom.find((x) => x.id === id && !x.deleted)
  if (!p) throw new MockHttpError(404, 'no such custom provider')
  if (body.display_name) p.display_name = body.display_name
  const kind = p.kind ?? 'openai'
  const version = body.api_version?.trim()
  if (kind === 'azure-openai' && body.api_version !== undefined && !version)
    throw new MockHttpError(400, "api_version cannot be cleared for kind 'azure-openai'")
  if (body.base_url) p.base_url = normalizeBase(kind, body.base_url)
  if (kind === 'azure-openai' && version) p.api_version = version
  if (body.api_key) p.api_key_set = true
  if (body.default_model) p.default_model = body.default_model
  if (body.catalog_sync_enabled !== undefined) p.catalog_sync_enabled = body.catalog_sync_enabled
  return { id }
}

export function deleteCustom(s: RouterState, id: string) {
  const p = s.custom.find((x) => x.id === id && !x.deleted)
  if (!p) throw new MockHttpError(404, 'no such custom provider')
  const refs = s.configs
    .filter((c) => c.provider === p.label && !c.deleted)
    .map((c) => c.name)
    .sort()
  if (refs.length)
    throw new MockHttpError(
      409,
      'provider is still referenced by LLM configs; repoint them first',
      {
        message: 'provider is still referenced by LLM configs; repoint them first',
        referencing_configs: refs,
      },
    )
  p.deleted = true
  return { id }
}

export function syncCustom(s: RouterState, id: string, now: number, fail: boolean) {
  const p = s.custom.find((x) => x.id === id && !x.deleted)
  if (!p) throw new MockHttpError(404, 'no such custom provider')
  if (fail) {
    p.last_sync_status = 'error'
    p.last_sync_error = 'connection refused'
    throw new MockHttpError(500, 'internal error')
  }
  p.last_sync_at = new Date(now).toISOString()
  p.last_sync_status = 'ok'
  p.last_sync_error = null
  if (!p.models.length) p.models = [p.default_model ?? `${p.label}-model`]
  return { discovered_models: p.models.length }
}

// ─── usage (token_usage, per agent) ─────────────────────────────────────────

/**
 * `/api/usage/by-agent`: the caller's billed rows grouped by agent, ordered by total tokens (no tie-breaker),
 * `COALESCE(SUM(cost_usd), 0)` (unpriced calls add $0). Derived from the seed's calls, which stand in for
 * `token_usage` rows (the router meters what traces record, so the two differ only by what traces miss).
 */
/**
 * usage/routes.rs by-agent (ea233d20): token_usage rows of the CALLER (`tu.user_id`), grouped by agent, newest `days`,
 * ordered by total tokens. The LEFT JOIN keeps a deleted agent's raw name. The live seed (seed-trace-usage.ts
 * routerSeed) writes one row per seed trace with an agent in the 30 days before the anchor, all as the seed admin.
 */
export function usageByAgent(
  seed: Seed,
  _agents: readonly MockAgent[],
  user: string,
  days: number,
  now: number,
): AgentUsage[] {
  if (user !== ADMIN_ID) return []
  const since = Math.max(now - days * 86_400_000, Date.parse(seed.anchor) - 30 * 86_400_000)
  const names = new Map(seed.agents.map((a) => [a.id, a.name]))
  const by = new Map<string, AgentUsage & { _lat: number }>()
  for (const t of seed.traces) {
    if (!t.agent_id || t.ts < since || t.ts > now) continue
    const row = by.get(t.agent_id) ?? {
      agent_id: t.agent_id,
      agent_name: names.get(t.agent_id) ?? null,
      request_count: 0,
      total_input_tokens: 0,
      total_output_tokens: 0,
      total_tokens: 0,
      total_cost_usd: 0,
      avg_latency_ms: null,
      _lat: 0,
    }
    row.request_count++
    row.total_input_tokens += t.input_tokens
    row.total_output_tokens += t.output_tokens
    row.total_tokens += t.input_tokens + t.output_tokens
    row.total_cost_usd += t.cost_usd
    row._lat += t.latency_ms
    by.set(t.agent_id, row)
  }
  return [...by.values()]
    .map(({ _lat, ...r }) => ({
      ...r,
      total_cost_usd: Math.round(r.total_cost_usd * 1e6) / 1e6,
      avg_latency_ms: r.request_count ? _lat / r.request_count : null,
    }))
    .sort((a, b) => b.total_tokens - a.total_tokens)
}
