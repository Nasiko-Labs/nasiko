// @vitest-environment node
/**
 * Recorder refusals and the stale-fixture sweep against a fake server (plans/feat-live-contract.md §5, §7.5), like
 * lifecycle.test.ts: a live lock, a missing seed marker, an EE run against an OSS server, a member identity on OSS, a
 * secret-shaped response value, and removal of stale (but never hand-written) fixtures.
 */
import { createServer, type Server } from 'node:http'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { gitIn, initRepo } from './gitRepo'
import { main, type RecordPaths } from '../../../../scripts/record-live.ts'
import { server as msw } from '@/test/setup'

const state = { marker: true }

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
        data: state.marker
          ? [
              {
                id: '5eed0007-0000-4000-8000-0000000000ff',
                name: 'seed-marker-2026-09-28T10:00:00.000Z',
              },
            ]
          : [],
      })
    case '/api/agents':
      return json([])
    case '/api/users':
      return json({ data: [] })
    case '/api/chat/sessions':
      return json({ data: [] })
    case '/api/openapi.json':
      return json({ openapi: '3.1.0' })
    case '/api/me':
      return json({ sub: 'u-fake' })
    case '/api/users/me':
      return json({ email: null })
    case '/api/finops/x':
      return json({ data: { total: 1 } })
    case '/api/finops/leak':
      return json({ data: { note: 'Bearer abcdefghijklmnopqrstuvwx' } })
    default:
      return { status: 404, ...json({ error: 'not found' }) }
  }
}

const X = { id: 'tokenops.x', feature: 'tokenops', method: 'GET', path: '/api/finops/x' }

let fake: Server
let base = ''
let dir = ''
let paths: RecordPaths
const repo = () => join(dir, 'cloud-rs')
const git = (...args: string[]) => gitIn(repo())(...args)
const withManifest = (...endpoints: object[]) =>
  writeFileSync(paths.manifest, JSON.stringify({ strict: false, endpoints }))
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
  state.marker = true
  dir = mkdtempSync(join(tmpdir(), 'record-live-edges-'))
  paths = {
    fixtures: join(dir, 'fixtures'),
    manifest: join(dir, 'manifest.json'),
    liveDir: join(dir, '.live'),
  }
  withManifest(X)
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

describe('refusals (exit 3 or 64, nothing written)', () => {
  it('a lock held by a running process refuses the run and is left in place', async () => {
    mkdirSync(paths.liveDir, { recursive: true })
    const lock = join(paths.liveDir, 'record.lock')
    writeFileSync(lock, String(process.pid))
    await expect(run()).rejects.toMatchObject({
      stage: 'lock',
      code: 3,
      message: expect.stringContaining('another recording is running'),
      fix: expect.stringMatching(/^rm /),
    })
    expect(existsSync(lock)).toBe(true)
    expect(existsSync(paths.fixtures)).toBe(false)
  })

  it('a server without the seed marker is refused with the seed-live fix', async () => {
    state.marker = false
    await expect(run()).rejects.toMatchObject({
      stage: 'verify',
      code: 3,
      message: expect.stringMatching(/no seed marker/),
      fix: expect.stringContaining('seed-live.ts --database nasiko_contract'),
    })
    expect(existsSync(paths.fixtures)).toBe(false)
  })

  it('--edition ee against an OSS server (org units 404) and a member identity on OSS are refused', async () => {
    await expect(run('--edition', 'ee')).rejects.toMatchObject({
      stage: 'preflight',
      code: 3,
      message: expect.stringMatching(/is an OSS server, not EE/),
      fix: expect.stringMatching(/^npm run ee:server/),
    })
    withManifest({ ...X, id: 'ee.member-only', auth: 'member' })
    await expect(run()).rejects.toMatchObject({
      stage: 'manifest',
      code: 64,
      message: 'ee.member-only: member identity is EE-only',
    })
    expect(existsSync(paths.fixtures)).toBe(false)
  })

  it('a secret-shaped value the scrub missed stops the run before any fixture is written', async () => {
    withManifest(X, {
      id: 'tokenops.leak',
      feature: 'tokenops',
      method: 'GET',
      path: '/api/finops/leak',
    })
    await expect(run()).rejects.toMatchObject({
      stage: 'scrub',
      code: 3,
      message: 'refusing to write a fixture: a secret-shaped value at data.note',
    })
    expect(existsSync(paths.fixtures)).toBe(false)
  })
})

describe('the stale-fixture sweep', () => {
  it("removes a recorded feature's fixtures no manifest entry wrote, but keeps hand-written ones and other features", async () => {
    const put = (rel: string, body: object) => {
      mkdirSync(join(paths.fixtures, rel, '..'), { recursive: true })
      writeFileSync(join(paths.fixtures, rel), JSON.stringify(body))
    }
    put('oss/tokenops/tokenops.retired.json', { id: 'tokenops.retired' })
    put('oss/tokenops/tokenops.hand.json', { id: 'tokenops.hand', source: 'hand-written' })
    put('oss/tokenops/notes.txt', {})
    put('oss/router/router.other.json', { id: 'router.other' })
    expect(await run()).toMatchObject({
      code: 0,
      summary: expect.stringMatching(/^1 fixtures written, 1 stale removed/),
    })
    expect(existsSync(join(paths.fixtures, 'oss/tokenops/tokenops.retired.json'))).toBe(false)
    for (const kept of [
      'oss/tokenops/tokenops.x.json',
      'oss/tokenops/tokenops.hand.json',
      'oss/tokenops/notes.txt',
      'oss/router/router.other.json',
    ]) {
      expect(existsSync(join(paths.fixtures, kept)), kept).toBe(true)
    }
  })
})
