import { describe, expect, it } from 'vitest'
import { copy } from './copy'
import {
  ago,
  configsUsing,
  INT_MAX,
  positiveIntProblem,
  registryProblem,
  secretNameProblem,
  settingsBody,
  valuesOf,
} from './logic'
import { SETTINGS_FIELDS } from './types'

const fresh = {
  router_model: 'gpt-4o',
  default_provider: 'anthropic',
  max_flow_depth: 8,
  max_flow_fan_out: 20,
  max_flow_tokens: 5000,
  flow_timeout_secs: 60,
  registry_url: 'https://from-another-admin.example.com',
  catalog_tabs: null,
}

describe('settingsBody (update_settings writes every column)', () => {
  it('sends every field: edited ones from the form, the rest from the fresh read', () => {
    const values = {
      ...valuesOf({ ...fresh, registry_url: 'stale.example.com' }),
      max_flow_depth: ' 9 ',
    }
    const body = settingsBody(fresh, values, new Set(['max_flow_depth'] as const))
    expect(Object.keys(body).sort()).toEqual([...SETTINGS_FIELDS].sort())
    expect(body).toEqual({ ...fresh, max_flow_depth: 9 })
  })

  it('sends numbers as numbers and a cleared field as null, never undefined', () => {
    const values = { ...valuesOf(fresh), catalog_tabs: '  ', max_flow_tokens: '100000' }
    const body = settingsBody({}, values, new Set(['catalog_tabs', 'max_flow_tokens'] as const))
    expect(body.catalog_tabs).toBeNull()
    expect(body.max_flow_tokens).toBe(100_000)
    expect(body.router_model).toBeNull()
    expect(Object.values(body).every((v) => v !== undefined)).toBe(true)
  })

  it('reads nulls as empty fields', () => {
    expect(valuesOf({ router_model: null, max_flow_depth: 5 })).toMatchObject({
      router_model: '',
      max_flow_depth: '5',
      catalog_tabs: '',
    })
  })
})

it('positiveIntProblem: a whole number from 1 to the column bound', () => {
  expect(positiveIntProblem('5', INT_MAX)).toBeNull()
  expect(positiveIntProblem(' ', INT_MAX)).toBe('required')
  for (const bad of ['0', '-1', '1.5', '1e3', 'ten', String(INT_MAX + 1)])
    expect(positiveIntProblem(bad, INT_MAX)).toBe('invalid')
})

it('registryProblem: blank, a host or a URL; not something that allows nothing', () => {
  for (const ok of [
    '',
    'https://registry.example.com/v2',
    'registry:5000',
    'http://localhost:5000',
  ])
    expect(registryProblem(ok)).toBeNull()
  for (const bad of ['https://', 'not a host', 'reg_istry.com', '-bad.com'])
    expect(registryProblem(bad)).toBe('invalid')
})

describe('secretNameProblem (validate_secret_name)', () => {
  it('follows the server rule', () => {
    expect(secretNameProblem('OPENAI_API_KEY')).toBeNull()
    expect(secretNameProblem('_X1')).toBeNull()
    expect(secretNameProblem('')).toBe('required')
    expect(secretNameProblem('A'.repeat(129))).toBe('length')
    expect(secretNameProblem('1KEY')).toBe('pattern')
    expect(secretNameProblem('openai')).toBe('pattern')
    expect(secretNameProblem('KEY-1')).toBe('pattern')
    expect(secretNameProblem('LD_PRELOAD')).toBe('reserved')
  })

  it('has words for every problem', () => {
    for (const p of ['required', 'length', 'pattern', 'reserved'] as const)
      expect(copy.secrets.nameProblem[p]).toBeTruthy()
  })
})

it('configsUsing lists the configs whose key is the secret', () => {
  const configs = [
    { name: 'default', api_key_secret_name: 'OPENAI_API_KEY' },
    { name: 'cheap', api_key_secret_name: null },
  ]
  expect(configsUsing(configs, 'OPENAI_API_KEY')).toEqual(['default'])
  expect(configsUsing(undefined, 'X')).toEqual([])
})

it('ago says it as the legacy list does', () => {
  const now = Date.parse('2026-03-20T15:00:00Z')
  expect(ago('2026-03-20T14:59:30Z', now)).toBe('30s ago')
  expect(ago('2026-03-20T14:10:00Z', now)).toBe('50m ago')
  expect(ago('2026-03-20T10:00:00Z', now)).toBe('5h ago')
  expect(ago('2026-03-17T15:00:00Z', now)).toBe('3d ago')
  expect(ago('2026-03-20T15:00:05Z', now)).toBe('0s ago')
})
