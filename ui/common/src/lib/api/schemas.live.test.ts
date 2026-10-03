/**
 * The response schemas passed to apiData/apiFetch (plan §8 Phase 8 item 5) against every recorded live response of
 * their endpoint, both editions (src/test/__live__, docs/live-contract.md): a schema that rejects what the real
 * server sends would turn a working page into an error state. The mocks are covered by the page tests, which
 * request through the same schemas.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { http, HttpResponse } from 'msw'
import { describe, expect, it, vi } from 'vitest'
import type { z } from 'zod'
import { grantsBodySchema } from '@/features/agents/grants'
import { agentDetailSchema, agentStatsSchema, usage24hSchema } from '@/features/agents/types'
import {
  agentConnectorsSchema,
  agentToolsSchema,
  buildStatusSchema,
  connectorDetailSchema,
  connectorListSchema,
  consumersSchema,
  credentialStatusSchema,
  oauthStatusSchema,
  sharesSchema,
  shareTargetsSchema,
  toolkitListSchema,
} from '@/features/mcp/types'
import { settingsSchema } from '@/features/settings/types'
import {
  finopsDashboardSchema,
  finopsDaySchema,
  providerCatalogSchema,
} from '@/features/tokenops/types'
import { server } from '@/test/setup'
import { ApiError, apiData } from './client'

interface Fx {
  id: string
  edition: string
  status: number
  body: unknown
  request: { method: string; path: string }
}
const fixtures: Fx[] = []
const walk = (d: string, edition: string) => {
  for (const f of readdirSync(d)) {
    const p = join(d, f)
    if (statSync(p).isDirectory()) walk(p, edition)
    else if (p.endsWith('.json'))
      fixtures.push({ ...(JSON.parse(readFileSync(p, 'utf8')) as Fx), edition })
  }
}
for (const e of ['oss', 'ee']) walk(join(__dirname, '../../test/__live__', e), e)

type At = 'body' | 'data'
/** Recorded path → the schemas its callers pass; `data` when apiData checks the envelope's payload. */
const CHECKED: [path: string, schema: z.ZodType, at: At][] = [
  ['/api/agents/{agent0}', agentDetailSchema, 'data'],
  ['/api/agents/{agent0}/grants', grantsBodySchema, 'body'],
  ['/api/observability/agent/{agent0}/stats', agentStatsSchema, 'data'],
  ['/api/observability/finops/dashboard', finopsDashboardSchema, 'data'],
  ['/api/observability/finops/dashboard', usage24hSchema, 'data'],
  ['/api/observability/finops/spend-calendar/day', finopsDaySchema, 'data'],
  ['/api/llm-router/providers', providerCatalogSchema, 'body'],
  ['/api/settings', settingsSchema, 'body'],
  // MCP servers (plans/feat-mcp.md): the OpenAPI spec types every `data` as McpEnvelope (M-1).
  ['/api/mcp/connectors', connectorListSchema, 'data'],
  ['/api/mcp/composio/toolkits', toolkitListSchema, 'data'],
  ['/api/mcp/connectors/{mcpDocs}', connectorDetailSchema, 'data'],
  ['/api/mcp/connectors/{mcpUpload}', connectorDetailSchema, 'data'],
  ['/api/mcp/connectors/{mcpFailed}/build-status', buildStatusSchema, 'data'],
  ['/api/mcp/connectors/{mcpBearer}/credential/status', credentialStatusSchema, 'data'],
  ['/api/mcp/connectors/{mcpOauth}/oauth/status', oauthStatusSchema, 'data'],
  ['/api/mcp/connectors/{mcpDocs}/grants', sharesSchema, 'data'],
  ['/api/mcp/connectors/{mcpDocs}/consumers', consumersSchema, 'data'],
  ['/api/mcp/share-targets', shareTargetsSchema, 'data'],
  ['/api/mcp/agents/{agent0}/connectors', agentConnectorsSchema, 'data'],
  ['/api/mcp/agents/{agent0}/connectors/{mcpDocs}/tools', agentToolsSchema, 'data'],
]

const recorded = (path: string) =>
  fixtures.filter((f) => f.request.method === 'GET' && f.request.path === path && f.status === 200)
const payload = (f: Fx, at: At) => (at === 'data' ? (f.body as { data: object }).data : f.body)
const first = (path: string, at: At = 'data') => payload(recorded(path)[0]!, at) as object

describe('response schemas accept every recorded live response', () => {
  it.each(CHECKED)('%s has recordings', (path) => {
    expect(recorded(path).length).toBeGreaterThan(0)
  })
  it('covers both editions where EE was recorded (agent detail, grants)', () => {
    for (const p of ['/api/agents/{agent0}', '/api/agents/{agent0}/grants'])
      expect(new Set(recorded(p).map((f) => f.edition))).toEqual(new Set(['oss', 'ee']))
  })
  const cases = CHECKED.flatMap(([path, schema, at]) =>
    recorded(path).map((f) => [`${f.edition}/${f.id}`, f, schema, at] as const),
  )
  it.each(cases)('%s', (_, f, schema, at) => {
    const r = schema.safeParse(payload(f, at))
    expect(r.success ? [] : r.error.issues.slice(0, 5)).toEqual([])
  })
})

describe('what the schemas reject', () => {
  it('the grants shape of neither edition', () => {
    expect(grantsBodySchema.safeParse({ available: false }).success).toBe(false)
    expect(grantsBodySchema.safeParse([{ nope: 1 }]).success).toBe(false)
  })
  it('a retyped KPI, but not an added field', () => {
    const d = first('/api/observability/finops/dashboard') as { kpis: Record<string, object> }
    expect(finopsDashboardSchema.safeParse({ ...d, extra: 1 }).success).toBe(true)
    const kpis = { ...d.kpis, total_spend: { ...d.kpis.total_spend, current: '1' } }
    expect(finopsDashboardSchema.safeParse({ ...d, kpis }).success).toBe(false)
  })
  it('a stats cost that is not a number', () => {
    const d = first('/api/observability/agent/{agent0}/stats') as { project: object }
    const bad = { project: { ...d.project, cost_summary: { total: { cost: '0.19' } } } }
    expect(agentStatsSchema.safeParse(bad).success).toBe(false)
  })
})

describe('apiData with a schema', () => {
  it('returns the payload as sent, extra fields included', async () => {
    const day = { ...first('/api/observability/finops/spend-calendar/day'), extra: 1 }
    server.use(http.get('/api/x', () => HttpResponse.json({ data: day })))
    expect(await apiData('/api/x', { schema: finopsDaySchema })).toEqual(day)
  })
  it('throws an ApiError naming the failing field, and logs it in dev', async () => {
    const log = vi.spyOn(console, 'error').mockImplementation(() => {})
    server.use(http.get('/api/x', () => HttpResponse.json({ data: { date: 7 } })))
    const err = await apiData('/api/x', { schema: finopsDaySchema }).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect((err as ApiError).status).toBe(200)
    expect((err as ApiError).isRetryable).toBe(false)
    expect((err as ApiError).message).toMatch(/^GET \/api\/x → unexpected response \(date: /)
    expect(log).toHaveBeenCalledWith(
      'GET /api/x: the response failed its schema',
      expect.arrayContaining([expect.stringMatching(/^date: /)]),
    )
    log.mockRestore()
  })
})
