import { describe, expect, it } from 'vitest'
import { copy } from './copy'
import {
  ago,
  configsUsing,
  registryProblem,
  secretNameProblem,
  settingsBody,
  valuesOf,
} from './logic'
import { SETTINGS_FIELDS } from './types'

const fresh = {
  registry_url: 'https://from-another-admin.example.com',
  catalog_tabs: 'devops',
}

describe('settingsBody (update_settings writes every column)', () => {
  it('sends every field: edited ones from the form, the rest from the fresh read', () => {
    const values = {
      ...valuesOf({ ...fresh, registry_url: 'stale.example.com' }),
      catalog_tabs: ' finance ',
    }
    const body = settingsBody(fresh, values, new Set(['catalog_tabs'] as const))
    expect(Object.keys(body).sort()).toEqual([...SETTINGS_FIELDS].sort())
    expect(body).toEqual({ ...fresh, catalog_tabs: 'finance' })
  })

  it('sends a cleared field as null, never undefined', () => {
    const values = { ...valuesOf(fresh), catalog_tabs: '  ' }
    const body = settingsBody({}, values, new Set(['catalog_tabs'] as const))
    expect(body.catalog_tabs).toBeNull()
    expect(body.registry_url).toBeNull()
    expect(Object.values(body).every((v) => v !== undefined)).toBe(true)
  })

  it('reads nulls as empty fields, and never what the server runs', () => {
    const values = valuesOf({ registry_url: null, max_flow_depth: 5, default_provider: 'openai' })
    expect(values).toEqual({ registry_url: '', catalog_tabs: '' })
  })
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
