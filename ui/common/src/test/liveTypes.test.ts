/**
 * Type contract (plans/feat-live-contract.md §7.2, UC2). Each recorded 2xx JSON body is validated against the response
 * schema in the server's own spec (`shell.openapi`, recorded with the run) where the spec covers the route, and
 * against a hand-written zod schema of the UI's wire type where it doesn't. Extra fields are allowed on both paths
 * (additive changes are information, printed by `record:live --check`); removed, renamed or retyped fields fail.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import type { CustomProvider, LlmConfig, SecretEntry } from '@/features/router/types'
import type {
  FinopsDayDrilldown,
  FinopsSpendCalendar,
  FinopsSpendTimeseries,
} from '@/features/tokenops/types'
import type { Me } from '@/lib/api/auth'
import { ADMIN_ID } from '@/mocks/seed-harness'

interface Fx {
  id: string
  status: number
  content_type: string
  body: unknown
  request: { method: string; path: string }
}
const LIVE = join(__dirname, '__live__', 'oss')
const fixtures: Fx[] = []
const walk = (d: string) => {
  for (const f of readdirSync(d)) {
    const p = join(d, f)
    if (statSync(p).isDirectory()) walk(p)
    else if (p.endsWith('.json')) fixtures.push(JSON.parse(readFileSync(p, 'utf8')) as Fx)
  }
}
walk(LIVE)
// The recorder writes placeholders for the admin's real id; put a UUID back so `format: uuid` still validates.
for (const f of fixtures)
  f.body = JSON.parse(JSON.stringify(f.body).replaceAll('<admin-id>', ADMIN_ID))
const byId = new Map(fixtures.map((f) => [f.id, f]))
const spec = byId.get('shell.openapi')!.body as {
  paths: Record<
    string,
    Record<
      string,
      { responses?: Record<string, { content?: Record<string, { schema?: unknown }> }> }
    >
  >
  components?: { schemas?: Record<string, unknown> }
}

// ── The spec path: OpenAPI 3.1 schemas are JSON Schema 2020-12, which zod 4 reads (`z.fromJSONSchema`) ────────────

const refs = (s: unknown) =>
  JSON.parse(JSON.stringify(s).replaceAll('#/components/schemas/', '#/$defs/')) as Record<
    string,
    unknown
  >
const defs = refs(spec.components?.schemas ?? {})
/** The spec route a fixture's request template (`/api/agents/{agent0}/llm-config`) falls under. */
function specRoute(path: string): string | undefined {
  const concrete = path.replace(/\{[^}]+\}/g, 'X')
  return Object.keys(spec.paths).find((p) =>
    new RegExp(`^${p.replace(/\{[^}]+\}/g, '[^/]+')}$`).test(concrete),
  )
}
function specSchema(fx: Fx): z.ZodType | null {
  const route = specRoute(fx.request.path)
  const schema =
    route &&
    spec.paths[route]![fx.request.method.toLowerCase()]?.responses?.[String(fx.status)]?.content?.[
      'application/json'
    ]?.schema
  return schema ? z.fromJSONSchema({ ...refs(schema), $defs: defs } as never) : null
}

/**
 * Where the spec disagrees with what the server sends and the UI's wire type agrees with the server: server
 * recommendations (docs/designs/openruntime-live-contract-recommendations.md), checked against zod instead. Each must
 * still fail the spec, so a fixed spec shows up as a stale entry here.
 */
const SPEC_GAPS: Record<string, string> = {
  'router.llm-configs':
    'C-1: the spec types `data` as one LlmConfig or null; GET /api/llm-configs returns an array',
  'router.secrets':
    'C-2: the spec types the body as a bare array; GET /api/secrets returns the {data, message, status_code} envelope',
  'agents.versions':
    'C-2: the spec types the body as a bare array; GET /api/agents/{id}/versions returns the envelope',
  'sessions.span':
    'C-4: the spec types ContentField.parsed_value as an object; span detail sends any parsed JSON (an array for GenAI messages)',
}
/** Features whose uncovered routes must have a wire-type schema (plan §7.2 first coverage; the rest are TODOs). */
const FIRST_COVERAGE = new Set(['shell', 'tokenops', 'router'])
const feature = (f: Fx) => f.id.split('.')[0]!

// ── The zod path: the UI's wire types, for routes the spec doesn't cover (and the spec gaps) ───────────────────────

const envelope = <T extends z.ZodType>(data: T) => z.object({ data })
const slice = z.object({ agent_name: z.string(), spend_usd: z.number() })
const iso = z.string().min(1)
const WIRE: Record<string, z.ZodType> = {
  '/api/me': z.object({
    sub: z.string(),
    username: z.string(),
    is_superuser: z.boolean(),
  }) satisfies z.ZodType<Me>,
  '/api/observability/finops/spend-timeseries': envelope(
    z.object({
      bucket: z.enum(['hour', 'day']),
      points: z.array(
        z.object({
          bucket_start: iso,
          spend_usd: z.number(),
          operations: z.number(),
          tool_calls: z.number(),
          top_agent_name: z.string().nullable(),
          top_agent_spend_usd: z.number().nullable(),
          p50_latency_ms: z.number().nullable(),
          p95_latency_ms: z.number().nullable(),
          p99_latency_ms: z.number().nullable(),
        }),
      ),
    }) satisfies z.ZodType<FinopsSpendTimeseries>,
  ),
  '/api/observability/finops/spend-calendar': envelope(
    z.object({
      days: z.array(
        z.object({
          date: z.string(),
          spend_usd: z.number(),
          operations: z.number(),
          intensity: z.number(),
        }),
      ),
      highlighted_dates: z.array(z.string()),
    }) satisfies z.ZodType<FinopsSpendCalendar>,
  ),
  '/api/observability/finops/spend-calendar/day': envelope(
    z.object({
      date: z.string(),
      hours: z.array(
        z.object({
          hour: z.number(),
          spend_usd: z.number(),
          top_agents: z.array(slice),
          others_spend_usd: z.number(),
        }),
      ),
      avg_hourly_spend_usd: z.number(),
      top_agents: z.array(slice),
      others_spend_usd: z.number(),
    }) satisfies z.ZodType<FinopsDayDrilldown>,
  ),
  '/api/custom-providers': envelope(
    z.array(
      z.object({
        id: z.string(),
        label: z.string(),
        display_name: z.string(),
        base_url: z.string(),
        default_model: z.string().nullable(),
        catalog_sync_enabled: z.boolean(),
        api_key_set: z.boolean(),
        last_sync_at: z.string().nullable(),
        last_sync_status: z.string().nullable(),
        last_sync_error: z.string().nullable(),
        created_at: iso,
      }) satisfies z.ZodType<CustomProvider>,
    ),
  ),
  '/api/llm-configs': envelope(
    z.array(
      z.object({
        id: z.string(),
        name: z.string(),
        provider: z.string(),
        model: z.string().nullable(),
        fallback_models: z.array(z.string()),
        temperature: z.number().nullable(),
        max_tokens: z.number().nullable(),
        api_key_secret_name: z.string().nullable(),
        pinned: z.boolean(),
        pinned_model: z.string().nullable(),
        tier1_model: z.string().nullable(),
        tier2_model: z.string().nullable(),
        tier3_model: z.string().nullable(),
        is_default: z.boolean(),
        created_at: iso,
        updated_at: iso,
      }) satisfies z.ZodType<LlmConfig>,
    ),
  ),
  '/api/secrets': envelope(
    z.array(
      z.object({
        id: z.string(),
        name: z.string(),
        created_at: iso,
        updated_at: iso,
      }) satisfies z.ZodType<SecretEntry>,
    ),
  ),
}

const issues = (r: { success: boolean; error?: z.ZodError }) =>
  r.success
    ? []
    : r.error!.issues.slice(0, 5).map((i) => `${i.path.join('.') || '$'}: ${i.message}`)
/** 2xx JSON bodies the UI reads; the spec itself and plain-text bodies (`/health`) are out of scope. */
const typed = fixtures.filter(
  (f) => f.id !== 'shell.openapi' && f.status < 300 && f.content_type.includes('json'),
)

describe("against the server's spec", () => {
  const covered = typed.filter((f) => !SPEC_GAPS[f.id] && specSchema(f))
  it('covers the routes the spec documents', () => {
    expect(covered.map((f) => f.id).sort()).toEqual(
      expect.arrayContaining([
        'router.agent-llm-config',
        'router.model-registry',
        'router.usage-by-agent',
        'tokenops.dashboard-30d',
        'tokenops.providers',
      ]),
    )
  })
  it.each(covered.map((f) => [f.id, f] as const))('%s matches its response schema', (_, f) => {
    expect(
      issues(specSchema(f)!.safeParse(f.body)),
      `re-check the UI's wire type for ${f.request.path}, or write the mismatch up as a server recommendation`,
    ).toEqual([])
  })
  it.each(Object.entries(SPEC_GAPS))('known gap %s still fails the spec (%s)', (id) => {
    const f = byId.get(id)!
    expect(
      specSchema(f)!.safeParse(f.body).success,
      `the spec now matches ${id}: remove it from SPEC_GAPS and its recommendation`,
    ).toBe(false)
  })
  it('fails a retyped or removed field the spec requires', () => {
    const f = byId.get('tokenops.dashboard-30d')!
    const body = f.body as { data: { summary: Record<string, unknown> } }
    const s = specSchema(f)!
    expect(
      s.safeParse({
        ...body,
        data: { ...body.data, summary: { ...body.data.summary, total_cost: '1' } },
      }).success,
    ).toBe(false)
    const { total_cost: _gone, ...rest } = body.data.summary
    expect(s.safeParse({ ...body, data: { ...body.data, summary: rest } }).success).toBe(false)
  })
})

describe("against the UI's wire types", () => {
  const zodChecked = typed.filter(
    (f) =>
      (SPEC_GAPS[f.id] || !specSchema(f)) &&
      (FIRST_COVERAGE.has(feature(f)) || WIRE[f.request.path]),
  )
  it('every typed fixture in the first-coverage features is checked one way or the other', () => {
    expect(zodChecked.filter((f) => !WIRE[f.request.path]).map((f) => f.id)).toEqual([])
  })
  it.each(zodChecked.map((f) => [f.id, f] as const))('%s matches its wire type', (_, f) => {
    expect(
      issues(WIRE[f.request.path]!.safeParse(f.body)),
      `the server changed ${f.request.path}: update the wire type in the feature's types.ts`,
    ).toEqual([])
  })
  it('allows extra fields, and fails a retyped one', () => {
    const f = byId.get('tokenops.calendar')!
    const body = f.body as { data: { days: Record<string, unknown>[] } }
    const s = WIRE[f.request.path]!
    expect(
      s.safeParse({
        data: {
          ...body.data,
          extra: 1,
          days: body.data.days.map((d) => ({ ...d, new_field: true })),
        },
      }).success,
    ).toBe(true)
    expect(
      s.safeParse({ data: { ...body.data, days: [{ ...body.data.days[0], spend_usd: '5' }] } })
        .success,
    ).toBe(false)
  })
})
