/**
 * The recorder's pure pieces (plans/feat-live-contract.md §5) and a guard over the committed fixtures: every
 * recordable manifest entry has one, in the current format, with no secret, no foreign id and no email.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { findSecret, foreignIds } from '../../../../scripts/lib/live.ts'
import {
  checkFixture,
  checkReport,
  contractEnv,
  contractOverrides,
  crossed,
  describeOverrides,
  fill,
  FIXTURE_VERSION,
  loadManifest,
  parseEnvFile,
  parseRecordArgs,
  recordable,
  requestFor,
  tokenContext,
  tokens,
  UNSET_ENV,
  type Fixture,
} from '../../../../scripts/record-live.ts'

const LIVE = join(__dirname, '..', '__live__')

describe('request params from the seed', () => {
  const t = tokens(tokenContext(new Date('2026-03-01T00:30:00Z')))
  it('derives months, the previous UTC day and the previous 30-day window from the anchor', () => {
    expect(t).toMatchObject({
      anchorMonth: '2026-03',
      prevMonth: '2026-02',
      prevDay: '2026-02-28',
      prevWindowEnd: '2026-01-30T00:30:00.000Z',
    })
    expect(t.agent0).toMatch(/^5eed0000-0001-/)
  })
  it('fills path and query tokens, and rejects unknown ones', () => {
    const r = requestFor(
      {
        id: 'x.y',
        feature: 'router',
        method: 'GET',
        path: '/api/agents/{agent0}/llm-config',
        query: { month: '{anchorMonth}' },
      },
      t,
    )
    expect(r).toEqual({ path: `/api/agents/${t.agent0}/llm-config`, query: { month: '2026-03' } })
    expect(() => fill('{nope}', t)).toThrow(/unknown token/)
  })
})

describe('crossed', () => {
  it('detects hour and UTC-day boundaries', () => {
    expect(
      crossed(new Date('2026-09-28T10:59:59Z'), new Date('2026-09-28T11:00:01Z'), 'hour'),
    ).toBe(true)
    expect(
      crossed(new Date('2026-09-28T10:00:01Z'), new Date('2026-09-28T10:59:59Z'), 'hour'),
    ).toBe(false)
    expect(crossed(new Date('2026-09-28T23:59:59Z'), new Date('2026-09-29T00:00:00Z'), 'day')).toBe(
      true,
    )
    expect(crossed(new Date('2026-09-28T10:59:59Z'), new Date('2026-09-28T11:00:01Z'), 'day')).toBe(
      false,
    )
  })
})

describe('contract server environment', () => {
  const example = parseEnvFile(
    '# comment\nDATABASE_URL=postgres://x/nasiko_dev\nexport OPENAI_API_KEY="sk-live"\nTRACE_USAGE_SYNC_SECS=120\nCODING_AGENT_OTLP_ENDPOINT=http://localhost:4318\nnot a line\n',
  )
  const overrides = contractOverrides({
    traces: true,
    admin: { username: 'admin', password: 'pw' },
  })
  const env = contractEnv(example, overrides)
  it('parses env files, including export and quotes', () => {
    expect(example).toEqual({
      DATABASE_URL: 'postgres://x/nasiko_dev',
      OPENAI_API_KEY: 'sk-live',
      TRACE_USAGE_SYNC_SECS: '120',
      CODING_AGENT_OTLP_ENDPOINT: 'http://localhost:4318',
    })
  })
  it('points at the throwaway DB, a private Tempo and Redis DB 15, with the materializer and syncs off', () => {
    expect(env.DATABASE_URL).toMatch(/\/nasiko_contract$/)
    // Not the dev Loki: seed agent ids exist in nasiko_dev too, and their logs must never reach a fixture.
    expect(env.LOKI_URL).not.toMatch(/:3100/)
    expect(env).toMatchObject({
      CP_BIND: '127.0.0.1:8181',
      TRACE_USAGE_SYNC_SECS: '0',
      MODEL_PRICING_SYNC_ENABLED: 'false',
      MODEL_CATALOG_SYNC_ENABLED: 'false',
      CONTAINER_HOURS_POLL_SECS: '0',
      REDIS_URL: 'redis://localhost:6379/15',
      TEMPO_URL: 'http://localhost:13200',
      SEED_AGENTS: '',
    })
  })
  it('drops variables the contract server must not inherit', () => {
    for (const k of UNSET_ENV) expect(env).not.toHaveProperty(k)
  })
  it('uses fresh secrets per run and redacts them when printed', () => {
    expect(
      contractOverrides({ traces: true, admin: { username: 'a', password: 'b' } }).JWT_SECRET,
    ).not.toBe(overrides.JWT_SECRET)
    const printed = describeOverrides(overrides)
    expect(printed).toContain('JWT_SECRET=<redacted>')
    expect(printed).toContain('ADMIN_PASSWORD=<redacted>')
    expect(printed).not.toContain(overrides.JWT_SECRET!)
  })
  it('turns observability off entirely with --no-traces', () => {
    expect(
      contractOverrides({ traces: false, admin: { username: 'a', password: 'b' } }),
    ).toMatchObject({ TEMPO_URL: '', LOKI_URL: '' })
  })
})

describe('parseRecordArgs', () => {
  const features = ['shell', 'tokenops', 'router', 'errors']
  it('defaults to its own server with traces', () => {
    expect(parseRecordArgs([], features)).toEqual({
      only: undefined,
      reuse: undefined,
      keepDb: false,
      traces: true,
      allowDirty: false,
      check: false,
      edition: 'oss',
    })
  })
  it('validates --only against the manifest features', () => {
    expect(() => parseRecordArgs(['--only', 'nope'], features)).toThrow(
      /unknown feature "nope"; valid features: shell, tokenops/,
    )
  })
  it('refuses a non-localhost --reuse-server', () => {
    expect(() =>
      parseRecordArgs(['--reuse-server', 'http://localhost.evil.test'], features),
    ).toThrow(/not localhost/)
  })
  it('accepts --check, and records EE against the running EE server', () => {
    expect(parseRecordArgs(['--check'], features)).toMatchObject({ check: true })
    expect(parseRecordArgs(['--edition', 'ee'], features)).toMatchObject({
      edition: 'ee',
      reuse: 'http://127.0.0.1:9090',
    })
    expect(() =>
      parseRecordArgs(['--edition', 'ee', '--reuse-server', 'http://evil.test'], features),
    ).toThrow(/not localhost/)
  })
  it('rejects unknown flags as bad arguments', () => {
    expect(() => parseRecordArgs(['--nope'], features)).toThrow(
      expect.objectContaining({ code: 64 }),
    )
  })
})

describe('--check', () => {
  const server = { sha: 'a', dirty: false, openapi_sha256: 'x', binary_mtime: null }
  const fx = (body: unknown, over: Partial<Fixture> = {}): Fixture => ({
    fixture_version: 1,
    id: 'tokenops.x',
    recorded_at: '',
    window: { start: '', end: '' },
    anchor: '',
    edition: 'oss',
    identity: 'admin',
    server,
    request: { method: 'GET', path: '/x', query: {} },
    status: 200,
    content_type: 'application/json',
    body,
    ...over,
  })
  const committed = fx({ data: { total: 1, rows: [{ a: 'x', b: null }] } })
  it('passes a re-record with other values, and exits 0', () => {
    const r = checkFixture(committed, fx({ data: { total: 2, rows: [{ a: 'y', b: null }] } }), {})
    expect(r).toMatchObject({ drift: [], info: [], fingerprintChanged: false })
    expect(checkReport([r]).code).toBe(0)
  })
  it('reports a removed or retyped field and a status change as drift (exit 1), even with a new fingerprint', () => {
    const r = checkFixture(
      committed,
      fx(
        { data: { total: '2', rows: [{ b: null }] } },
        { server: { ...server, openapi_sha256: 'y' } },
      ),
      {},
    )
    expect(r.drift.map((d) => `${d.kind} ${d.path}`)).toEqual([
      'extra-in-mock $.data.rows[].a',
      'type $.data.total',
    ])
    const report = checkReport([
      r,
      checkFixture(committed, fx(committed.body, { status: 500 }), {}),
    ])
    expect(report.code).toBe(1)
    expect(report.lines).toContain(
      'record-live: DRIFT tokenops.x $.data.total: type now string, committed number',
    )
    expect(report.summary).toMatch(/^2 of 2 endpoints drifted/)
  })
  it('lists a new field and a null-only change as information, not drift', () => {
    const r = checkFixture(
      committed,
      fx({ data: { total: 1, extra: true, rows: [{ a: 'x', b: 'now set' }] } }),
      {},
    )
    expect(r.drift).toEqual([])
    expect(checkReport([r]).lines).toEqual([
      'record-live: info  tokenops.x $.data.extra: new field (boolean)',
      expect.stringMatching(
        /^record-live: info {2}tokenops\.x \$\.data\.rows\[\]\.b: nullability now/,
      ),
    ])
  })
  it('exits 2 when only the openapi fingerprint changed', () => {
    expect(
      checkReport([
        checkFixture(
          committed,
          fx(committed.body, { server: { ...server, openapi_sha256: 'y' } }),
          {},
        ),
      ]).code,
    ).toBe(2)
  })
  it('treats a missing committed fixture as drift', () => {
    expect(checkReport([checkFixture(undefined, fx({}), {})]).lines).toEqual([
      'record-live: DRIFT tokenops.x $: no committed fixture',
    ])
  })
})

describe('committed fixtures', () => {
  const manifest = loadManifest()
  const files: string[] = []
  const walk = (d: string) => {
    for (const f of readdirSync(d)) {
      const p = join(d, f)
      if (statSync(p).isDirectory()) walk(p)
      else if (p.endsWith('.json') && !p.endsWith('manifest.json') && !p.endsWith('.schema.json'))
        files.push(p)
    }
  }
  walk(LIVE)
  const fixtures = files.map((f) => ({
    file: f,
    fx: JSON.parse(readFileSync(f, 'utf8')) as {
      id: string
      fixture_version: number
      body: unknown
      source?: string
    },
  }))
  /** The one foreign id a fixture may hold: the probe the unknown-agent request sends on purpose. */
  const PROBE = '00000000-0000-4000-8000-000000000000'

  it('has unique manifest ids', () => {
    const ids = manifest.endpoints.map((e) => e.id)
    expect(new Set(ids).size).toBe(ids.length)
  })
  it('has one fixture per recordable OSS entry, and one per hand-written entry', () => {
    const have = new Set(fixtures.map((x) => x.fx.id))
    const want = [
      ...recordable(manifest, 'oss').map((e) => e.id),
      ...manifest.endpoints.filter((e) => e.hand_written).map((e) => e.id),
    ]
    expect(want.filter((id) => !have.has(id))).toEqual([])
  })
  it.each(fixtures.map((x) => [x.fx.id, x]))('%s is current, scrubbed and seed-only', (_, x) => {
    const { fx } = x as (typeof fixtures)[number]
    expect(fx.fixture_version).toBe(FIXTURE_VERSION)
    expect(findSecret(fx.body)).toBeNull()
    expect(foreignIds(fx.body).filter((id) => id !== PROBE)).toEqual([])
    // example.com (RFC 2606) is the mock admin's address, which the recorder maps the live one to.
    expect(JSON.stringify(fx.body)).not.toMatch(
      /[\w.+-]+@(?!localhost\b|example\.com\b)[\w-]+\.[a-z]{2,}/i,
    )
  })
})
