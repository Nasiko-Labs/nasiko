// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { ApiError } from '@/lib/api/client'
import { ROUTER_PAGE_VARIANTS } from '@/mocks/handlers'
import createHelp from './__fixtures__/nasiko-llm-config-create-help.txt?raw'
import attachHelp from './__fixtures__/nasiko-llm-config-attach-help.txt?raw'
import { copy, RETIRED } from './copy'
import { keySaveFailed, routerError } from './errors'
import {
  affects,
  attachedAgents,
  blockedClears,
  catalogIndex,
  cliAttach,
  cliFlags,
  cliLine,
  configPin,
  configWarnings,
  countText,
  defaultSecretName,
  duplicateName,
  fallbackInCatalog,
  groupAgentRows,
  isRoutable,
  keepOverrideCheck,
  keySource,
  matchesAgent,
  parseFallback,
  pickableProviders,
  requestPlan,
  routingSentence,
  secretNameError,
  summarize,
  usedBy,
  type RoutingState,
  type RowRead,
} from './routing'
import { ROUTER_VARIANT_KEYS } from './scenarioKeys'
import type { AgentRouting, LlmConfig, ProviderCatalog } from './types'

const cfg = (over: Partial<LlmConfig> = {}): LlmConfig => ({
  id: 'c1',
  name: 'main',
  provider: 'anthropic',
  model: 'claude-sonnet-4',
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
  created_at: '2026-09-01T00:00:00Z',
  updated_at: '2026-09-01T00:00:00Z',
  ...over,
})
const routing = (over: Partial<AgentRouting> = {}): AgentRouting => ({
  agent_id: 'a',
  llm_config_id: null,
  llm_config: null,
  source: 'none',
  inbound_format: 'openai',
  pinned_model: null,
  ...over,
})
const ok = (r: AgentRouting): RowRead => ({ state: 'ok', routing: r })
const catalog: ProviderCatalog[] = [
  {
    provider: 'openai',
    models: [
      { model: 'gpt-4o', pricing_available: true },
      { model: 'gpt-4o-mini', pricing_available: true },
    ],
  },
  { provider: 'anthropic', models: [{ model: 'claude-sonnet-4', pricing_available: true }] },
]
const idx = catalogIndex(catalog)

describe('routingSentence (plan §4.7)', () => {
  it('reads each case', () => {
    expect(routingSentence(null, null).full).toBe('No config: the request’s model, platform key')
    expect(routingSentence(cfg(), null).full).toBe('anthropic · claude-sonnet-4 · no fallbacks')
    expect(routingSentence(cfg({ fallback_models: ['a', 'b'] }), null).full).toBe(
      'anthropic · claude-sonnet-4 · falls back to 2 models',
    )
    expect(routingSentence(cfg({ fallback_models: ['a'] }), null).full).toContain(
      'falls back to 1 model',
    )
    expect(routingSentence(cfg({ pinned: true, pinned_model: 'claude-haiku' }), null).full).toBe(
      'Pinned to claude-haiku (config) · tiers and fallbacks off',
    )
    // A pin with no pinned_model pins the config's model (the CLI's documented default).
    expect(routingSentence(cfg({ pinned: true }), null).full).toBe(
      'Pinned to claude-sonnet-4 (config) · tiers and fallbacks off',
    )
    expect(routingSentence(cfg(), 'gpt-4o').full).toBe(
      'Overridden to gpt-4o (agent) · tiers and fallbacks off',
    )
  })

  it('shortens tiered configs for rows and keeps the tier list', () => {
    const s = routingSentence(
      cfg({ tier1_model: 'a', tier2_model: 'b', tier3_model: 'c', fallback_models: ['x'] }),
      null,
    )
    expect(s.kind).toBe('tiered')
    expect(s.full).toBe('Tiered: a / b / c · falls back to 1 model')
    expect(s.short).toBe('Tiered · 3 models')
    expect(s.tiers).toEqual(['a', 'b', 'c'])
  })

  it('lets an agent override beat a pinned, tiered config', () => {
    expect(
      routingSentence(cfg({ pinned: true, pinned_model: 'p', tier1_model: 't' }), 'o').kind,
    ).toBe('override')
    expect(
      routingSentence(cfg({ pinned: true, pinned_model: 'p', tier1_model: 't' }), null).kind,
    ).toBe('pinned')
    expect(configPin(cfg({ pinned: false, pinned_model: 'p' }))).toBeNull()
  })
})

describe('keys and fallbacks', () => {
  it('pays with the saved key or the platform key', () => {
    expect(keySource(null)).toEqual({ kind: 'platform' })
    expect(keySource(cfg())).toEqual({ kind: 'platform' })
    expect(keySource(cfg({ api_key_secret_name: 'ANTHROPIC_API_KEY' }))).toEqual({
      kind: 'user',
      secret: 'ANTHROPIC_API_KEY',
    })
  })

  it('parses bare, same-provider and cross-provider fallbacks (split on the first /)', () => {
    const c = cfg({ api_key_secret_name: 'K' })
    expect(parseFallback('claude-haiku', c)).toMatchObject({
      provider: 'anthropic',
      model: 'claude-haiku',
      crossProvider: false,
      key: { kind: 'user', secret: 'K' },
    })
    expect(parseFallback('anthropic/claude-haiku', c)).toMatchObject({
      provider: 'anthropic',
      crossProvider: false,
      key: { kind: 'user' },
    })
    expect(parseFallback('openai/gpt-4o', c)).toMatchObject({
      provider: 'openai',
      model: 'gpt-4o',
      crossProvider: true,
      key: { kind: 'platform' },
    })
    expect(parseFallback('openrouter/meta/llama', c)).toMatchObject({
      provider: 'openrouter',
      model: 'meta/llama',
    })
    expect(parseFallback('claude-haiku', cfg())).toMatchObject({ key: { kind: 'platform' } })
  })

  it('checks fallbacks against the catalog', () => {
    expect(fallbackInCatalog('openai/gpt-4o', cfg(), idx)).toBe(true)
    expect(fallbackInCatalog('claude-sonnet-4', cfg(), idx)).toBe(true)
    expect(fallbackInCatalog('gpt-4o', cfg(), idx)).toBe(false)
    expect(fallbackInCatalog('openai/gpt-7', cfg(), idx)).toBe(false)
  })
})

describe('providers', () => {
  it('routes built-ins and registered custom labels only', () => {
    expect(isRoutable('openai', [])).toBe(true)
    expect(isRoutable('gemini', [])).toBe(true)
    expect(isRoutable('google', [])).toBe(false)
    expect(isRoutable('openrouter', [])).toBe(false)
    expect(isRoutable('openrouter', ['openrouter'])).toBe(true)
    expect(pickableProviders(['custom'])).toEqual(['openai', 'anthropic', 'gemini', 'custom'])
    expect(pickableProviders([])).not.toContain('openrouter')
  })
})

describe('counts', () => {
  const def = cfg({ id: 'd', is_default: true })
  const rows: RowRead[] = [
    ok(routing({ source: 'attached', llm_config_id: 'c1', llm_config: cfg() })),
    ok(routing({ source: 'attached', llm_config_id: 'c1', llm_config: cfg() })),
    ok(routing({ source: 'owner-default', llm_config: def })),
    ok(routing({ source: 'none' })),
  ]

  it('summarizes sources and the default config', () => {
    const s = summarize(rows)
    expect(s.total).toBe(4)
    expect([s.attached.n, s.onDefault.n, s.none.n]).toEqual([2, 1, 1])
    expect(s.defaultConfig?.id).toBe('d')
    expect(s.attached.atLeast).toBe(false)
  })

  it('reads "at least" while any row is pending or failed', () => {
    const s = summarize([...rows, { state: 'error' }])
    expect(s.onDefault).toEqual({ n: 1, atLeast: true })
    expect(countText(s.onDefault)).toBe('at least 1')
    expect(countText({ n: 3, atLeast: false })).toBe('3')
  })

  it('counts what each change affects (plan §4.3)', () => {
    expect(affects('edit', cfg(), rows).n).toBe(2)
    expect(affects('edit', def, rows).n).toBe(1)
    expect(affects('set-default', cfg(), rows).n).toBe(2) // the default's agent and the no-config one move
    expect(affects('remove-default', def, rows).n).toBe(1)
    expect(affects('delete-default', def, rows).n).toBe(1)
    expect(usedBy(cfg(), rows).n).toBe(2)
    expect(affects('edit', cfg(), [...rows, { state: 'pending' }]).atLeast).toBe(true)
    expect(
      attachedAgents(
        'c1',
        rows.map((read, i) => ({ id: `a${i}`, read })),
      ),
    ).toEqual(['a0', 'a1'])
  })
})

describe('requestPlan (eng #2)', () => {
  const configs = [null, 'c1', 'c2'] as const
  const pins = [null, 'gpt-4o', 'gpt-4o-mini'] as const
  const all: RoutingState[] = configs.flatMap((llm_config_id) =>
    pins.map((pinned_model) => ({ llm_config_id, pinned_model })),
  )

  it('never sends llm_config_id and pinned_model together, nor inbound_format', () => {
    for (const current of all) {
      for (const desired of all) {
        for (const step of requestPlan(current, desired)) {
          expect(Object.keys(step.body).length, JSON.stringify({ current, desired, step })).toBe(1)
          expect(step.body).not.toHaveProperty('inbound_format')
        }
      }
    }
  })

  it('always ends at the desired state, simulated like the server', () => {
    for (const current of all) {
      for (const desired of all) {
        const s = { ...current }
        for (const step of requestPlan(current, desired)) {
          if ('llm_config_id' in step.body) {
            s.llm_config_id = step.body.llm_config_id ?? null
            s.pinned_model = null // attach and detach clear the pin
          } else {
            s.pinned_model = step.body.pinned_model ?? null
          }
        }
        expect(s, JSON.stringify({ current, desired })).toEqual(desired)
      }
    }
  })

  it('attaches first, then re-pins', () => {
    expect(
      requestPlan(
        { llm_config_id: 'c1', pinned_model: 'gpt-4o' },
        { llm_config_id: 'c2', pinned_model: 'gpt-4o' },
      ).map((s) => s.kind),
    ).toEqual(['attach', 'pin'])
    expect(
      requestPlan(
        { llm_config_id: 'c1', pinned_model: 'gpt-4o' },
        { llm_config_id: null, pinned_model: 'gpt-4o' },
      ).map((s) => s.kind),
    ).toEqual(['detach', 'pin'])
    expect(
      requestPlan(
        { llm_config_id: 'c1', pinned_model: 'gpt-4o' },
        { llm_config_id: 'c2', pinned_model: null },
      ).map((s) => s.kind),
    ).toEqual(['attach'])
    expect(
      requestPlan(
        { llm_config_id: 'c1', pinned_model: 'gpt-4o' },
        { llm_config_id: 'c1', pinned_model: null },
      ).map((s) => s.kind),
    ).toEqual(['unpin'])
    expect(
      requestPlan(
        { llm_config_id: 'c1', pinned_model: null },
        { llm_config_id: 'c1', pinned_model: null },
      ),
    ).toEqual([])
  })
})

describe('keepOverrideCheck (plan §4.5)', () => {
  it('needs the model in the target provider’s catalog', () => {
    expect(keepOverrideCheck('gpt-4o', cfg({ provider: 'openai' }), idx, true)).toEqual({
      allowed: true,
      anyProvider: false,
    })
    // Tier-only configs still have a provider.
    expect(
      keepOverrideCheck(
        'gpt-4o',
        cfg({ provider: 'openai', model: null, tier1_model: 'gpt-4o' }),
        idx,
        true,
      ).allowed,
    ).toBe(true)
    expect(keepOverrideCheck('gpt-4o', cfg(), idx, true)).toEqual({
      allowed: false,
      reason: 'gpt-4o isn’t offered by anthropic',
    })
  })

  it('checks the whole catalog when the target is no config, and refuses a superuser without the owner’s configs', () => {
    expect(keepOverrideCheck('gpt-4o', null, idx, true)).toEqual({
      allowed: true,
      anyProvider: true,
    })
    expect(keepOverrideCheck('gpt-9', null, idx, true).allowed).toBe(false)
    expect(keepOverrideCheck('gpt-4o', cfg({ provider: 'openai' }), idx, false)).toEqual({
      allowed: false,
      reason: copy.cantSeeOwnerConfigs,
    })
  })
})

describe('the config editor', () => {
  it('blocks emptying a field the server can’t clear, but not pinned or fallbacks', () => {
    const c = cfg({
      temperature: 0.3,
      pinned_model: 'x',
      tier1_model: 't',
      api_key_secret_name: 'K',
    })
    expect(
      blockedClears(c, {
        temperature: null,
        pinned_model: '',
        tier1_model: 't',
        api_key_secret_name: 'K',
        model: 'claude-sonnet-4',
      }),
    ).toEqual(['temperature', 'pinned_model'])
    expect(blockedClears(cfg(), { temperature: null, model: 'claude-sonnet-4' })).toEqual([])
    expect(blockedClears(cfg({ max_tokens: 100 }), { max_tokens: 50, model: 'm' })).toEqual([])
  })

  it('names duplicates and default secrets', () => {
    expect(duplicateName('main', ['main'])).toBe('main copy')
    expect(duplicateName('main', ['main', 'main copy', 'main copy 2'])).toBe('main copy 3')
    expect(defaultSecretName('anthropic')).toBe('ANTHROPIC_API_KEY')
    expect(defaultSecretName('my-llm')).toBe('MY_LLM_API_KEY')
    expect(defaultSecretName('4ai')).toBe('_4AI_API_KEY')
  })

  it('validates secret names like the secrets API', () => {
    expect(secretNameError('OPENAI_API_KEY')).toBeNull()
    expect(secretNameError('openai')).toMatch(/start with/)
    expect(secretNameError('A-B')).toMatch(/only contain/)
    expect(secretNameError('LD_PRELOAD')).toMatch(/reserved/)
    expect(secretNameError('')).toMatch(/1-128/)
  })
})

describe('the CLI equivalent (DX #4)', () => {
  const helpFlags = new Set([...createHelp.matchAll(/--[a-z-]+/g)].map((m) => m[0]))

  it('uses only flags the recorded --help lists, and never --secret-value', () => {
    const c = cfg({
      name: 'my config',
      fallback_models: ['openai/gpt-4o'],
      temperature: 0.2,
      max_tokens: 512,
      api_key_secret_name: 'K',
      pinned: true,
      pinned_model: 'claude-haiku',
      is_default: true,
    })
    const r = cliLine(c)
    expect('line' in r).toBe(true)
    if (!('line' in r)) return
    expect(r.line).toBe(
      "nasiko llm-config create --name 'my config' --provider anthropic --model claude-sonnet-4 --fallback openai/gpt-4o --temperature 0.2 --max-tokens 512 --api-key-secret K --pin --pinned-model claude-haiku --default",
    )
    for (const f of cliFlags(r.line)) expect(helpFlags, f).toContain(f)
    expect(r.line).not.toContain('--secret-value')
  })

  it('says when the CLI can’t express a config', () => {
    expect(cliLine(cfg({ tier1_model: 't' }))).toEqual({ unsupported: 'tiered' })
    expect(cliLine(cfg({ model: null }))).toEqual({ unsupported: 'no-model' })
  })

  it('quotes attach arguments and matches the recorded attach usage', () => {
    expect(cliAttach('support-bot', 'my config')).toBe(
      "nasiko llm-config attach support-bot 'my config'",
    )
    expect(attachHelp).toContain('nasiko llm-config attach [OPTIONS] <AGENT> <CONFIG>')
    expect(attachHelp).toContain('--inbound-format')
  })
})

describe('errors (plan §4.9)', () => {
  const e = (status: number, body: unknown) => new ApiError(status, body, '/api/x', 'x')

  it('maps the server’s plain-text errors to problem, cause and action', () => {
    expect(routerError(e(409, "an LLM config named 'main' already exists"))).toMatchObject({
      field: 'name',
      problem: 'That name is taken.',
    })
    expect(routerError(e(409, 'config is attached to 2 agent(s); detach it first')).problem).toBe(
      'This config is in use.',
    )
    expect(
      routerError(e(400, 'llm config not found or not owned by the agent owner')).problem,
    ).toMatch(/can’t be attached/)
    expect(routerError(e(400, 'pinned_model must not be empty')).field).toBe('pin')
    expect(
      routerError(e(400, "secret 'K' not found; provide secret_value to store it")).field,
    ).toBe('secret')
    expect(routerError(e(403, 'not the agent owner')).problem).toMatch(/owner/)
    expect(routerError(e(404, 'llm config not found')).problem).toMatch(/deleted elsewhere/)
    expect(routerError(e(502, null)).problem).toBe(copy.errDown)
  })

  it('never shows the database text of a failed secret write (S-6)', () => {
    const v = routerError(
      e(500, "failed to store secret 'K': duplicate key value violates unique constraint"),
    )
    expect(JSON.stringify(v)).not.toMatch(/duplicate key/)
  })

  it.each([
    [400, 'secret name must match [A-Z_][A-Z0-9_]*', 'That secret name isn’t allowed.', 'secret'],
    [400, "unsupported provider 'cohere'", 'The router can’t use that provider.', 'provider'],
    [
      400,
      "'x' is not a registered custom provider",
      'The router can’t use that provider.',
      'provider',
    ],
    [400, 'model or a tier model is required', 'The model settings aren’t valid.', 'model'],
    // Order matters: the pin rule wins over the broad model rule.
    [400, 'pinned_model must not be empty', 'Choose the model to pin.', 'pin'],
    [400, 'base_url must not be empty', 'The provider details aren’t complete.', 'baseUrl'],
    [409, 'provider is still referenced', 'This provider is still used by configs.', undefined],
    [504, 'gateway timeout', copy.errDown, undefined],
    // R2 budgets: matched before the config name rule ("already exists").
    [409, 'budget already exists for this scope', 'This scope already has a budget.', undefined],
    [409, 'budget changed elsewhere', copy.budgetChanged, undefined],
    [400, 'limit_usd must be greater than 0', copy.limitPositive, 'limit'],
    [400, 'thresholds must be whole percents from 1 to 100', copy.thresholdRange, 'thresholds'],
  ])('maps %i "%s"', (status, text, problem, field) => {
    const v = routerError(e(status, text))
    expect(v.problem).toBe(problem)
    expect(v.field).toBe(field)
  })

  it('shows anything unmapped verbatim with its status', () => {
    expect(routerError(e(418, 'teapot')).problem).toBe('OpenRuntime said: teapot (HTTP 418)')
    expect(routerError(e(500, 'internal error')).problem).toBe(
      'OpenRuntime said: internal error (HTTP 500)',
    )
  })

  it('knows when a failed save may still have stored the key (eng #6)', () => {
    // llm_configs.rs returns every 400/404/409 before ensure_secret; only a 5xx or a lost response can follow it.
    expect(keySaveFailed(e(409, 'already exists'))).toBe(false)
    expect(keySaveFailed(e(400, "secret 'K' not found"))).toBe(false)
    expect(keySaveFailed(e(400, 'pinned_model must not be empty'))).toBe(false)
    expect(keySaveFailed(e(500, 'internal error'))).toBe(true)
    expect(keySaveFailed(new TypeError('Failed to fetch'))).toBe(true)
  })
})

describe('configWarnings (DX #9)', () => {
  const ctx = { secrets: ['K'], customLabels: ['custom'], idx, catalogLoaded: true }

  it('warns about data made elsewhere, never about a clean config', () => {
    expect(configWarnings(cfg({ api_key_secret_name: 'K' }), ctx)).toEqual([])
    expect(configWarnings(cfg({ provider: 'openrouter', model: 'x' }), ctx)).toEqual([
      copy.openrouterHidden,
    ])
    expect(configWarnings(cfg({ provider: 'retired-llm', model: 'x' }), ctx)).toEqual([
      copy.providerGone,
    ])
    expect(configWarnings(cfg({ api_key_secret_name: 'GONE' }), ctx)).toEqual([
      copy.missingKey('GONE'),
    ])
    expect(configWarnings(cfg({ model: 'claude-9' }), ctx)).toEqual([copy.notInCatalog])
    expect(configWarnings(cfg({ fallback_models: ['openai/gpt-7'] }), ctx)).toEqual([
      copy.notInCatalog,
    ])
  })

  it('stays quiet while the secrets or the catalog are still loading', () => {
    expect(
      configWarnings(cfg({ api_key_secret_name: 'GONE', model: 'claude-9' }), {
        ...ctx,
        secrets: undefined,
        catalogLoaded: false,
      }),
    ).toEqual([])
  })
})

describe('copy and mock keys', () => {
  it('never uses a retired form', () => {
    const all = JSON.stringify(copy, (_k, v) =>
      typeof v === 'function' ? v('<model>', '<b>', '<c>') : v,
    )
    for (const r of RETIRED) expect(all).not.toContain(r)
    expect(all).not.toMatch(/"None"/)
    expect(all).not.toMatch(/R-L\d/)
  })

  it('lists the same ?mock= router variants as the handlers', () => {
    expect([...ROUTER_VARIANT_KEYS]).toEqual([...ROUTER_PAGE_VARIANTS])
  })
})

describe('groupAgentRows (Your agents folding)', () => {
  const row = (id: string, routing: Partial<AgentRouting> | 'error' | 'pending') => ({
    agent: { id },
    read:
      routing === 'error'
        ? { state: 'error' as const }
        : routing === 'pending'
          ? { state: 'pending' as const }
          : {
              state: 'ok' as const,
              routing: {
                agent_id: id,
                llm_config_id: null,
                llm_config: null,
                source: 'owner-default',
                inbound_format: 'openai',
                pinned_model: null,
                ...routing,
              } as AgentRouting,
            },
  })
  const rows = [
    row('a', {}),
    row('b', {}),
    row('c', {}),
    row('d', { source: 'attached' }),
    row('e', { pinned_model: 'gpt-4o' }),
    row('f', 'error'),
    row('g', 'pending'),
  ]
  const opts = { narrowed: false, recent: new Set<string>(), min: 3 }

  it('folds plain default rows; keeps attached, overridden, failed and pending ones visible', () => {
    const g = groupAgentRows(rows, opts)
    expect(g.folded.map((r) => r.agent.id)).toEqual(['a', 'b', 'c'])
    expect(g.shown.map((r) => r.agent.id)).toEqual(['d', 'e', 'f', 'g'])
  })
  it('doesn’t fold fewer than min, a narrowed list, or a row just changed', () => {
    expect(groupAgentRows(rows, { ...opts, min: 4 }).folded).toEqual([])
    expect(groupAgentRows(rows, { ...opts, narrowed: true }).folded).toEqual([])
    const g = groupAgentRows(rows, { ...opts, recent: new Set(['a']), min: 2 })
    expect(g.folded.map((r) => r.agent.id)).toEqual(['b', 'c'])
    expect(g.shown.map((r) => r.agent.id)).toContain('a')
  })
  it('matches agents by display or raw name, case-insensitively', () => {
    expect(matchesAgent({ name: 'seed-doc-writer', display_name: 'Doc Writer' }, 'doc')).toBe(true)
    expect(matchesAgent({ name: 'seed-doc-writer', display_name: 'Doc Writer' }, 'SEED-DOC')).toBe(
      true,
    )
    expect(matchesAgent({ name: 'x', display_name: 'Y' }, 'z')).toBe(false)
    expect(matchesAgent({ name: 'x' }, '  ')).toBe(true)
  })
})
