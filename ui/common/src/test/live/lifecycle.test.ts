// @vitest-environment node
/**
 * Recorder lifecycle and exit codes against a fake server (plans/feat-live-contract.md §7.5), never a live stack:
 * `main()` runs with `--reuse-server`, a temp manifest and fixture dir, and a throwaway clean git repo standing in
 * for nasiko-cloud-rs.
 */
import { createServer, type Server } from 'node:http'
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { gitIn, initRepo } from './gitRepo'
import { main, type RecordPaths } from '../../../../scripts/record-live.ts'
import { server as msw } from '@/test/setup'

/** What the fake server answers; tests change it between runs. */
const state = {
  total: 1 as number | string,
  openapi: 'v1',
  agents: [] as { id: string; name: string }[],
}

const json = (body: unknown) => ({ type: 'application/json', text: JSON.stringify(body) })
function route(
  method: string,
  path: string,
): { status?: number; type: string; text: string } | null {
  if (method === 'POST' && path === '/api/auth/login')
    return json({ token: 'fake-session', user_id: 'u-fake', is_superuser: true })
  if (method !== 'GET') return null
  switch (path) {
    case '/health':
      return { type: 'text/plain', text: 'ok' }
    case '/api/llm-configs':
      return json({
        data: [
          {
            id: '5eed0007-0000-4000-8000-0000000000ff',
            name: 'seed-marker-2026-09-28T10:00:00.000Z',
          },
        ],
      })
    case '/api/agents':
      return json(state.agents)
    case '/api/users':
      return json({ data: [] })
    case '/api/chat/sessions':
      return json({ data: [] })
    case '/api/openapi.json':
      return json({ openapi: '3.1.0', info: { version: state.openapi } })
    case '/api/me':
      return json({ sub: 'u-fake' })
    case '/api/users/me':
      return json({ email: null })
    case '/api/finops/x':
      return json({ data: { total: state.total, owner: 'u-fake', rows: [{ a: 'x' }] } })
    default:
      return { status: 404, ...json({ error: 'not found' }) }
  }
}

const MANIFEST = {
  strict: false,
  endpoints: [
    { id: 'shell.health', feature: 'shell', method: 'GET', path: '/health', auth: 'none' },
    {
      id: 'tokenops.x',
      feature: 'tokenops',
      method: 'GET',
      path: '/api/finops/x',
      query: { month: '{anchorMonth}' },
    },
  ],
}

let fake: Server
let base = ''
let dir = ''
let paths: RecordPaths
const repo = () => join(dir, 'cloud-rs')
const git = (...args: string[]) => gitIn(repo())(...args)
const fixture = (rel: string) =>
  JSON.parse(readFileSync(join(paths.fixtures, 'oss', rel), 'utf8')) as {
    anchor: string
    body: unknown
    server: { sha: string; openapi_sha256: string }
  }
const run = (...argv: string[]) => main(['--reuse-server', base, ...argv], paths)

beforeAll(async () => {
  // Real sockets here: the shared MSW server would refuse every unhandled request.
  msw.close()
  fake = createServer((req, res) => {
    const r = route(req.method ?? 'GET', new URL(req.url ?? '/', 'http://x').pathname)
    if (!r) {
      res.writeHead(405).end()
      return
    }
    res.writeHead(r.status ?? 200, { 'Content-Type': r.type }).end(r.text)
  })
  await new Promise<void>((ok) => fake.listen(0, '127.0.0.1', ok))
  const addr = fake.address()
  base = `http://127.0.0.1:${typeof addr === 'object' && addr ? addr.port : 0}`
})
afterAll(async () => {
  await new Promise((ok) => fake.close(ok))
  msw.listen({ onUnhandledRequest: 'error' })
})

beforeEach(() => {
  Object.assign(state, { total: 1, openapi: 'v1', agents: [] })
  dir = mkdtempSync(join(tmpdir(), 'record-live-test-'))
  paths = {
    fixtures: join(dir, 'fixtures'),
    manifest: join(dir, 'manifest.json'),
    liveDir: join(dir, '.live'),
  }
  writeFileSync(paths.manifest, JSON.stringify(MANIFEST))
  initRepo(repo())
  writeFileSync(join(repo(), 'README'), 'x\n')
  git('add', '.')
  git('commit', '-qm', 'init')
  vi.stubEnv('NASIKO_CLOUD_RS', repo())
  vi.spyOn(console, 'log').mockImplementation(() => {})
  vi.spyOn(console, 'error').mockImplementation(() => {})
})
afterEach(() => {
  vi.unstubAllEnvs()
  vi.restoreAllMocks()
  rmSync(dir, { recursive: true, force: true })
})

describe('record mode', () => {
  it('writes scrubbed, fingerprinted fixtures and releases its lock', async () => {
    expect(await run()).toMatchObject({
      code: 0,
      summary: expect.stringMatching(/^2 fixtures written/),
    })
    const x = fixture('tokenops/tokenops.x.json')
    expect(x.anchor).toBe('2026-09-28T10:00:00.000Z')
    expect(x.body).toEqual({ data: { total: 1, owner: '<admin-id>', rows: [{ a: 'x' }] } })
    expect(x.server.sha).toBe(git('rev-parse', 'HEAD').stdout.trim())
    expect(fixture('shell/shell.health.json').body).toBe('ok')
    expect(existsSync(join(paths.liveDir, 'record.lock'))).toBe(false)
  })

  it('a double re-record shows zero drift', async () => {
    await run()
    await run()
    expect(await run('--check')).toMatchObject({
      code: 0,
      summary: '2 endpoints match the committed fixtures',
    })
  })

  it('refuses a server holding non-seed data (exit 3) and writes nothing', async () => {
    state.agents = [{ id: 'a-real', name: 'prod-agent' }]
    await expect(run()).rejects.toMatchObject({
      stage: 'verify',
      code: 3,
      message: expect.stringContaining('non-seed data (1 rows, e.g. agent prod-agent)'),
    })
    expect(existsSync(paths.fixtures)).toBe(false)
  })

  it('refuses a dirty nasiko-cloud-rs tree (exit 4) unless --allow-dirty', async () => {
    writeFileSync(join(repo(), 'README'), 'changed\n')
    await expect(run()).rejects.toMatchObject({ code: 4 })
    expect(await run('--allow-dirty')).toMatchObject({ code: 0 })
  })

  it('rejects bad arguments with exit 64, and a non-localhost server as a safety refusal (exit 3)', async () => {
    await expect(main(['--nope'], paths)).rejects.toMatchObject({ code: 64 })
    await expect(
      main(['--reuse-server', 'http://localhost.evil.test'], paths),
    ).rejects.toMatchObject({ code: 3 })
  })

  it('clears a stale lock from a crashed run', async () => {
    await run()
    writeFileSync(join(paths.liveDir, 'record.lock'), '999999')
    expect(await run()).toMatchObject({ code: 0 })
  })
})

describe('--check', () => {
  beforeEach(async () => {
    await run()
  })

  it('exits 1 on a retyped field and leaves the committed fixtures alone', async () => {
    const before = readFileSync(join(paths.fixtures, 'oss/tokenops/tokenops.x.json'), 'utf8')
    state.total = 'one'
    expect(await run('--check')).toMatchObject({
      code: 1,
      summary: expect.stringMatching(/^1 of 2 endpoints drifted/),
    })
    expect(readFileSync(join(paths.fixtures, 'oss/tokenops/tokenops.x.json'), 'utf8')).toBe(before)
  })

  it('exits 2 when only the openapi fingerprint changed', async () => {
    state.openapi = 'v2'
    expect(await run('--check')).toMatchObject({
      code: 2,
      summary: 'openapi sha changed; no drift in 2 endpoints',
    })
  })

  it('exits 1 over 2 when both drift and fingerprint changed', async () => {
    state.openapi = 'v2'
    state.total = 'one'
    expect(await run('--check')).toMatchObject({ code: 1 })
  })
})
