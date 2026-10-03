/**
 * The router page's pure rules (plan §4, §7). Every branchy decision lives here, table-driven where it can be,
 * so components stay thin and `logic.test.ts` covers the combinations.
 *
 *   agent ──► attached config ──else──► owner's default ──else──► no config
 *               │ agent override? ─► that model, tiers and fallbacks off
 *               │ config pin on?  ─► pinned model (or its model), tiers and fallbacks off
 *               │ tier models?    ─► per-request tier, then fallbacks
 *               └ single model    ─► the model, then fallbacks
 */
import { copy } from './copy'
import type { AgentRouting, AttachBody, LlmConfig, ProviderCatalog, RoutingConfig } from './types'

/** Providers the router speaks natively (llm-router resolver `is_builtin_provider`). */
const BUILTIN_PROVIDERS = ['openai', 'anthropic', 'gemini'] as const

// ─── models, tiers and pins ─────────────────────────────────────────────────

const tierModels = (c: Pick<LlmConfig, 'tier1_model' | 'tier2_model' | 'tier3_model'>): string[] =>
  [c.tier1_model, c.tier2_model, c.tier3_model].filter((m): m is string => !!m)

/** The model a config pins (its `pinned_model`, else its model, as the CLI documents), or null when not pinned. */
export function configPin(c: RoutingConfig): string | null {
  if (!c.pinned) return null
  return c.pinned_model || c.model || null
}

type SentenceKind = 'override' | 'pinned' | 'tiered' | 'single' | 'none'

export interface Sentence {
  kind: SentenceKind
  /** The whole sentence (sheets, the agent card). */
  full: string
  /** The row form ("Tiered · 3 models"); equal to `full` except for tiered configs. */
  short: string
  /** Tier models, listed in the tiered row's tooltip. */
  tiers: string[]
}

/** The one routing sentence rows, sheets and the agent card share (plan §4.7). */
export function routingSentence(config: RoutingConfig | null, agentPin: string | null): Sentence {
  if (agentPin) return one('override', copy.overridden(agentPin))
  if (!config) return one('none', copy.noConfig)
  const pin = configPin(config)
  if (pin) return one('pinned', copy.pinnedConfig(pin))
  const fallbacks = copy.fallsBack(config.fallback_models.length)
  const tiers = tierModels(config)
  if (tiers.length)
    return {
      kind: 'tiered',
      full: copy.tiered(tiers.join(' / '), fallbacks),
      short: copy.tieredShort(tiers.length),
      tiers,
    }
  return one('single', copy.single(config.provider, config.model ?? '—', fallbacks))
}
const one = (kind: SentenceKind, text: string): Sentence => ({
  kind,
  full: text,
  short: text,
  tiers: [],
})

// ─── keys and fallbacks ─────────────────────────────────────────────────────

export type KeySource = { kind: 'user'; secret: string } | { kind: 'platform' }

/** A config with a saved key pays with it; no config, or no key name, uses the platform key. */
export function keySource(config: RoutingConfig | null): KeySource {
  return config?.api_key_secret_name
    ? { kind: 'user', secret: config.api_key_secret_name }
    : { kind: 'platform' }
}

export interface Fallback {
  entry: string
  provider: string
  model: string
  /** Another provider: the call uses the platform key, and is skipped when the platform has none. */
  crossProvider: boolean
  key: KeySource
}

/** `provider/model` or a bare model on the config's provider (llm-router `split_prefixed`: the first `/`). */
export function parseFallback(
  entry: string,
  config: Pick<LlmConfig, 'provider' | 'api_key_secret_name'>,
): Fallback {
  const i = entry.indexOf('/')
  const provider = i >= 0 ? entry.slice(0, i) : config.provider
  const model = i >= 0 ? entry.slice(i + 1) : entry
  const crossProvider = provider !== config.provider
  const key: KeySource =
    !crossProvider && config.api_key_secret_name
      ? { kind: 'user', secret: config.api_key_secret_name }
      : { kind: 'platform' }
  return { entry, provider, model, crossProvider, key }
}

// ─── the catalog ────────────────────────────────────────────────────────────

export interface CatalogIndex {
  has: (provider: string, model: string) => boolean
  anywhere: (model: string) => boolean
  providers: string[]
}

export function catalogIndex(catalog: readonly ProviderCatalog[] | undefined): CatalogIndex {
  const by = new Map<string, Set<string>>()
  for (const p of catalog ?? []) by.set(p.provider, new Set(p.models.map((m) => m.model)))
  return {
    has: (provider, model) => !!by.get(provider)?.has(model),
    anywhere: (model) => [...by.values()].some((s) => s.has(model)),
    providers: [...by.keys()],
  }
}

/** The router can call a provider when it is built in or a registered custom provider (resolver rule). */
export function isRoutable(provider: string, customLabels: readonly string[]): boolean {
  return (
    (BUILTIN_PROVIDERS as readonly string[]).includes(provider) || customLabels.includes(provider)
  )
}

/** Providers a new config may use: built-ins plus custom labels. `openrouter` only when a custom provider has that label. */
export function pickableProviders(customLabels: readonly string[]): string[] {
  return [
    ...BUILTIN_PROVIDERS,
    ...customLabels.filter((l) => !(BUILTIN_PROVIDERS as readonly string[]).includes(l)),
  ]
}

export function fallbackInCatalog(
  entry: string,
  config: Pick<LlmConfig, 'provider' | 'api_key_secret_name'>,
  idx: CatalogIndex,
): boolean {
  const f = parseFallback(entry, config)
  return idx.has(f.provider, f.model)
}

// ─── rows, counts and "Affects N" ───────────────────────────────────────────

/** One owned agent's routing read: pending, failed, or the answer. */
export type RowRead =
  { state: 'pending' } | { state: 'error' } | { state: 'ok'; routing: AgentRouting }

export interface Count {
  n: number
  /** Some reads are pending or failed, so the true count may be higher ("at least N"). */
  atLeast: boolean
}

export interface Summary {
  total: number
  attached: Count
  onDefault: Count
  none: Count
  /** The default config every "Your default" row resolved to, when any row did. */
  defaultConfig: RoutingConfig | null
}

export function summarize(rows: readonly RowRead[]): Summary {
  const unknown = rows.some((r) => r.state !== 'ok')
  const count = (source: AgentRouting['source']) => ({
    n: rows.filter((r) => r.state === 'ok' && r.routing.source === source).length,
    atLeast: unknown,
  })
  const def = rows.find((r) => r.state === 'ok' && r.routing.source === 'owner-default')
  return {
    total: rows.length,
    attached: count('attached'),
    onDefault: count('owner-default'),
    none: count('none'),
    defaultConfig: def && def.state === 'ok' ? def.routing.llm_config : null,
  }
}

export type AffectsKind = 'edit' | 'set-default' | 'remove-default' | 'delete-default'

/**
 * How many of the caller's agents a change moves (plan §4.3): editing counts the config's attached agents plus,
 * for the default, the agents on it; Set as default moves those on the old default or on no config; Remove
 * default and deleting the default move those on it. Harness rows count like any row.
 */
export function affects(
  kind: AffectsKind,
  config: Pick<LlmConfig, 'id' | 'is_default'>,
  rows: readonly RowRead[],
): Count {
  const atLeast = rows.some((r) => r.state !== 'ok')
  const ok = rows.flatMap((r) => (r.state === 'ok' ? [r.routing] : []))
  const attached = ok.filter((r) => r.source === 'attached' && r.llm_config_id === config.id).length
  const onDefault = ok.filter((r) => r.source === 'owner-default').length
  const none = ok.filter((r) => r.source === 'none').length
  const n =
    kind === 'edit'
      ? attached + (config.is_default ? onDefault : 0)
      : kind === 'set-default'
        ? onDefault + none
        : onDefault
  return { n, atLeast }
}

/** "Used by N" on a config row: its attached agents, plus the default's agents. */
export const usedBy = (
  config: Pick<LlmConfig, 'id' | 'is_default'>,
  rows: readonly RowRead[],
): Count => affects('edit', config, rows)

/** Agents attached to a config (the delete 409 list). */
export function attachedAgents(
  configId: string,
  rows: readonly { id: string; read: RowRead }[],
): string[] {
  return rows
    .filter(
      (r) =>
        r.read.state === 'ok' &&
        r.read.routing.source === 'attached' &&
        r.read.routing.llm_config_id === configId,
    )
    .map((r) => r.id)
}

export const countText = (c: Count): string => (c.atLeast ? copy.atLeast(c.n) : String(c.n))

// ─── the agent routing request plan (plan §4.5, eng #2) ─────────────────────

export interface RoutingState {
  llm_config_id: string | null
  pinned_model: string | null
}

type StepKind = 'attach' | 'detach' | 'pin' | 'unpin'

export interface Step {
  kind: StepKind
  body: AttachBody
}

/**
 * The ordered PATCHes that move an agent from `current` to `desired`. The server applies `pinned_model` only
 * when `llm_config_id` is absent, and an attach or a detach clears the pin, so a config change and a pin are
 * always two requests, config first. No step ever carries both fields, and `inbound_format` is never sent.
 */
export function requestPlan(current: RoutingState, desired: RoutingState): Step[] {
  const steps: Step[] = []
  const routeChanged = desired.llm_config_id !== current.llm_config_id
  if (routeChanged) {
    steps.push(
      desired.llm_config_id
        ? { kind: 'attach', body: { llm_config_id: desired.llm_config_id } }
        : { kind: 'detach', body: { llm_config_id: null } },
    )
  }
  // After a route change the pin is gone, so compare against null.
  const pinNow = routeChanged ? null : current.pinned_model
  if (desired.pinned_model !== pinNow) {
    steps.push(
      desired.pinned_model
        ? { kind: 'pin', body: { pinned_model: desired.pinned_model } }
        : { kind: 'unpin', body: { pinned_model: null } },
    )
  }
  return steps
}

export type KeepCheck = { allowed: true; anyProvider: boolean } | { allowed: false; reason: string }

/**
 * Whether "Keep the override" can re-add `pin` after switching to `target` (plan §4.5): the pinned model must be
 * in the catalog for the target's provider; with no config, anywhere in the catalog. A superuser on another
 * user's agent can't read the owner's configs, so the check can't run.
 */
export function keepOverrideCheck(
  pin: string,
  target: RoutingConfig | null,
  idx: CatalogIndex,
  canReadConfigs: boolean,
): KeepCheck {
  if (!canReadConfigs) return { allowed: false, reason: copy.cantSeeOwnerConfigs }
  if (!target)
    return idx.anywhere(pin)
      ? { allowed: true, anyProvider: true }
      : { allowed: false, reason: copy.keepNotOffered(pin, 'any provider in the catalog') }
  return idx.has(target.provider, pin)
    ? { allowed: true, anyProvider: false }
    : { allowed: false, reason: copy.keepNotOffered(pin, target.provider) }
}

// ─── the config editor ──────────────────────────────────────────────────────

const CLEARABLE_FIELDS = [
  'model',
  'temperature',
  'max_tokens',
  'api_key_secret_name',
  'pinned_model',
  'tier1_model',
  'tier2_model',
  'tier3_model',
] as const
export type ClearableField = (typeof CLEARABLE_FIELDS)[number]

/**
 * Fields a draft empties that the server can't clear (PATCH merges with COALESCE: null keeps the value).
 * `pinned` (a bool) and `fallback_models` (an array, `[]` is not null) are not in the list: both can change freely.
 */
export function blockedClears(
  original: LlmConfig,
  draft: Partial<Record<ClearableField, string | number | null>>,
): ClearableField[] {
  return CLEARABLE_FIELDS.filter((f) => {
    const was = original[f]
    const now = draft[f]
    return was !== null && was !== '' && (now === null || now === '' || now === undefined)
  })
}

/** "<name> copy", then "<name> copy 2", … skipping names already taken. */
export function duplicateName(name: string, taken: readonly string[]): string {
  const set = new Set(taken)
  const first = `${name} copy`
  if (!set.has(first)) return first
  for (let i = 2; ; i++) if (!set.has(`${first} ${i}`)) return `${first} ${i}`
}

/** The default secret name for a provider's key: `ANTHROPIC_API_KEY`, `MY_LLM_API_KEY`. */
export function defaultSecretName(provider: string): string {
  const base = provider
    .toUpperCase()
    .replace(/[^A-Z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
  return `${/^[A-Z_]/.test(base) ? base : `_${base}`}_API_KEY`
}

const RESERVED_SECRETS = [
  'PATH',
  'LD_PRELOAD',
  'LD_LIBRARY_PATH',
  'LD_AUDIT',
  'DYLD_INSERT_LIBRARIES',
  'DYLD_LIBRARY_PATH',
  'IFS',
  'HOME',
  'SHELL',
  'BASH_ENV',
  'ENV',
  'PYTHONPATH',
  'NODE_OPTIONS',
  'PERL5LIB',
  'GIT_SSH_COMMAND',
]

/** The secrets API's rule (`secrets::routes::validate_secret_name`), applied before a config save sends the name. */
export function secretNameError(name: string): string | null {
  if (!name || name.length > 128) return 'secret name must be 1-128 characters'
  if (!/^[A-Z_]/.test(name)) return 'secret name must start with [A-Z_]'
  if (!/^[A-Z_][A-Z0-9_]*$/.test(name)) return 'secret name may only contain [A-Z0-9_]'
  if (RESERVED_SECRETS.includes(name)) return `${name} is reserved`
  return null
}

// ─── the CLI equivalent (DX #4) ─────────────────────────────────────────────

/** Shell-quote a value only when it needs it. */
const q = (v: string) => (/^[A-Za-z0-9_./:@-]+$/.test(v) ? v : `'${v.replace(/'/g, `'\\''`)}'`)

/**
 * `nasiko llm-config create …` for a config, or null when the CLI can't express it (tier models, or no model).
 * Never includes `--secret-value`: a key typed on a command line ends up in shell history.
 */
export function cliLine(
  c: RoutingConfig,
): { line: string } | { unsupported: 'tiered' | 'no-model' } {
  if (tierModels(c).length) return { unsupported: 'tiered' }
  if (!c.model) return { unsupported: 'no-model' }
  const parts = [
    'nasiko',
    'llm-config',
    'create',
    '--name',
    q(c.name),
    '--provider',
    q(c.provider),
    '--model',
    q(c.model),
  ]
  for (const f of c.fallback_models) parts.push('--fallback', q(f))
  if (c.temperature !== null) parts.push('--temperature', String(c.temperature))
  if (c.max_tokens !== null) parts.push('--max-tokens', String(c.max_tokens))
  if (c.api_key_secret_name) parts.push('--api-key-secret', q(c.api_key_secret_name))
  if (c.pinned) {
    parts.push('--pin')
    if (c.pinned_model) parts.push('--pinned-model', q(c.pinned_model))
  }
  if (c.is_default) parts.push('--default')
  return { line: parts.join(' ') }
}

export const cliAttach = (agent: string, config: string) =>
  `nasiko llm-config attach ${q(agent)} ${q(config)}`
export const cliDetach = (agent: string) => `nasiko llm-config detach ${q(agent)}`

/** Flags a generated line uses, for the test against the recorded `--help`. */
export const cliFlags = (line: string): string[] => [
  ...new Set(line.split(' ').filter((p) => p.startsWith('--'))),
]

// ─── data made elsewhere (DX #9) ────────────────────────────────────────────

export interface WarnContext {
  /** Saved secret names; undefined while the list hasn't loaded (no missing-key warning then). */
  secrets: readonly string[] | undefined
  customLabels: readonly string[]
  idx: CatalogIndex
  catalogLoaded: boolean
}

/**
 * Warnings for a config as stored, made by the CLI or the legacy UI: a provider the router can't call, a saved key
 * that no longer exists, models off the catalog. The page shows the stored values with these, never hides them.
 */
export function configWarnings(c: RoutingConfig, ctx: WarnContext): string[] {
  const out: string[] = []
  if (!isRoutable(c.provider, ctx.customLabels))
    out.push(c.provider === 'openrouter' ? copy.openrouterHidden : copy.providerGone)
  if (c.api_key_secret_name && ctx.secrets && !ctx.secrets.includes(c.api_key_secret_name))
    out.push(copy.missingKey(c.api_key_secret_name))
  if (ctx.catalogLoaded && isRoutable(c.provider, ctx.customLabels)) {
    const models = [c.model, ...tierModels(c), configPin(c)].filter((m): m is string => !!m)
    const off =
      models.some((m) => !ctx.idx.has(c.provider, m)) ||
      c.fallback_models.some((f) => !fallbackInCatalog(f, c, ctx.idx))
    if (off) out.push(copy.notInCatalog)
  }
  return out
}

// ─── Your agents: which rows fold (plans/feat-llm-router.md §4.2 amendment, 2026-09-28) ─────────────────────────────

export interface AgentGroups<T> {
  /** Rows that need a look: attached, overridden, no config, failed or pending reads, and rows just changed. */
  shown: T[]
  /** Rows that just follow your default: folded into one summary row (empty when not folding). */
  folded: T[]
}

/**
 * Split rows into the ones that stay visible and the ones on your default, which fold once there are `min` of them.
 * Nothing folds while a filter or a search narrows the list (the viewer asked for those rows), and a row changed on
 * this page stays visible so it doesn't vanish into the group after its save.
 */
export function groupAgentRows<T extends { agent: { id: string }; read: RowRead }>(
  rows: readonly T[],
  opts: { narrowed: boolean; recent: ReadonlySet<string>; min: number },
): AgentGroups<T> {
  const plain = (r: T) =>
    r.read.state === 'ok' &&
    r.read.routing.source === 'owner-default' &&
    !r.read.routing.pinned_model &&
    !opts.recent.has(r.agent.id)
  const folded = opts.narrowed ? [] : rows.filter(plain)
  if (folded.length < opts.min) return { shown: [...rows], folded: [] }
  return { shown: rows.filter((r) => !plain(r)), folded }
}

/** Case-insensitive name match for the agents search (display name or raw name). */
export const matchesAgent = (a: { name: string; display_name?: string | null }, q: string) => {
  const needle = q.trim().toLowerCase()
  return (
    !needle ||
    (a.display_name ?? '').toLowerCase().includes(needle) ||
    a.name.toLowerCase().includes(needle)
  )
}
