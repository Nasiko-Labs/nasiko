// @vitest-environment node
/**
 * The live-contract scripts' shared pieces (plans/feat-live-contract.md §7.5): the loopback guard, the scrub and
 * secret guard, login's no-retry handling of lockout and rate limits and its remaining failures, the error table,
 * the stack guards (they refuse before any docker call), OTLP attribute encoding, and the seed wrapper's database
 * guards, --otlp rules and trace posting. Scripts are imported by relative path, like the chat smoke's fixture test.
 */
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { attr } from '../../../../scripts/lib/otlp.ts'
import {
  composeFile,
  dropThrowaway,
  ensureDatabase,
  flushRedisDb,
  recreateThrowaway,
} from '../../../../scripts/lib/stack.ts'
import {
  ERRORS,
  EXIT,
  Failure,
  findSecret,
  foreignIds,
  isLoopbackUrl,
  login,
  requireLoopback,
  scrubBody,
} from '../../../../scripts/lib/live.ts'
import { parseSeedArgs, postTraces, sqlFlags } from '../../../../scripts/seed-live.ts'

describe('isLoopbackUrl', () => {
  it.each([
    'http://localhost:8181',
    'http://127.0.0.1:8080/x',
    'http://[::1]:3000',
    'https://localhost',
  ])('accepts %s', (u) => {
    expect(isLoopbackUrl(u)).toBe(true)
  })
  it.each([
    'http://localhost.evil.test',
    'http://evil.localhost',
    'http://127.0.0.2',
    'http://example.com',
    'ftp://localhost',
    'localhost:8080',
    '',
  ])('rejects %s', (u) => {
    expect(isLoopbackUrl(u)).toBe(false)
  })
  it('requireLoopback throws a not-ready Failure with a fix', () => {
    expect(() => requireLoopback('http://10.0.0.5', '--url')).toThrow(Failure)
    try {
      requireLoopback('http://10.0.0.5', '--url')
    } catch (err) {
      expect((err as Failure).code).toBe(EXIT.notReady)
      expect((err as Failure).fix).toMatch(/localhost/)
    }
  })
})

describe('scrubBody', () => {
  it('redacts credential keys but keeps each JSON type and every counter', () => {
    const out = scrubBody({
      token: 'abc',
      api_key: 'sk-xyz',
      nested: [{ password: 'p', input_tokens: 12, total_tokens: 40 }],
      api_key_secret_name: 'OPENAI',
      api_key_set: true,
    }) as Record<string, unknown>
    expect(out.token).toBe('<redacted>')
    expect(out.api_key).toBe('<redacted>')
    expect(out.nested).toEqual([{ password: '<redacted>', input_tokens: 12, total_tokens: 40 }])
    expect(out.api_key_secret_name).toBe('OPENAI')
    expect(out.api_key_set).toBe(true)
  })
  it('replaces known real values everywhere they appear', () => {
    const admin = '1b1e9a1e-8b8f-4c3a-9d3e-2f6c7a1b0c9d'
    const out = scrubBody(
      { created_by: admin, url: `/api/users/${admin}` },
      { replace: new Map([[admin, '<admin-id>']]) },
    )
    expect(out).toEqual({ created_by: '<admin-id>', url: '/api/users/<admin-id>' })
  })
  it('turns declared server-generated ids into stable placeholders, leaving seed ids', () => {
    const out = scrubBody(
      {
        data: [
          { id: 'aaa' },
          { id: 'bbb' },
          { id: 'aaa' },
          { id: '5eed0000-0001-4000-8000-000000000001' },
        ],
      },
      { placeholders: ['data[].id'] },
    )
    expect(out).toEqual({
      data: [
        { id: '<id-1>' },
        { id: '<id-2>' },
        { id: '<id-1>' },
        { id: '5eed0000-0001-4000-8000-000000000001' },
      ],
    })
  })
})

describe('findSecret and foreignIds', () => {
  // The provider-key fixture is assembled rather than written out: the repo's
  // own secret scan (scripts/check-secret-patterns.sh) greps every file the
  // public sync would publish for `sk-[A-Za-z0-9]{20,}`, and a literal here
  // trips it. The gate fails closed and aborts the sync, so a test fixture
  // that merely looks like a key would block every publish. Splitting it
  // leaves the value under test identical.
  const skKey = 'sk-' + 'abcdefghijklmnopqrstu'

  it('finds a JWT, an sk- key and a Bearer token nested in arrays', () => {
    expect(findSecret({ a: [{ b: 'eyJhbGciOiJIUzI1NiJ9.x.y' }] })).toBe('a[0].b')
    expect(findSecret([{ note: `use ${skKey}` }])).toBe('[0].note')
    expect(findSecret({ h: 'Bearer abcdefghijklmnopqrstuvwx' })).toBe('h')
    expect(findSecret({ input_tokens: 5, name: 'seed-default' })).toBeNull()
  })
  it('lists UUIDs that are not seed ids', () => {
    expect(
      foreignIds({
        a: '5eed0000-0001-4000-8000-000000000001',
        b: ['x 1b1e9a1e-8b8f-4c3a-9d3e-2f6c7a1b0c9d y'],
      }),
    ).toEqual(['1b1e9a1e-8b8f-4c3a-9d3e-2f6c7a1b0c9d'])
  })
})

describe('login', () => {
  afterEach(() => vi.unstubAllGlobals())
  const respond = (status: number, body: unknown) => {
    const fetchMock = vi.fn(
      async () =>
        new Response(JSON.stringify(body), {
          status,
          headers: { 'content-type': 'application/json' },
        }),
    )
    vi.stubGlobal('fetch', fetchMock)
    return fetchMock
  }

  it('returns the token on success', async () => {
    respond(200, { token: 't0k', user_id: 'u1', is_superuser: true })
    await expect(login('http://localhost:8181', 'admin', 'pw')).resolves.toEqual({
      token: 't0k',
      userId: 'u1',
      isSuperuser: true,
    })
  })
  it('reports an account lock with its retry time and never retries', async () => {
    const f = respond(429, {
      error: 'account temporarily locked',
      code: 'account_locked',
      retry_after_secs: 300,
    })
    await expect(login('http://localhost:8181', 'admin', 'pw')).rejects.toMatchObject({
      stage: 'login',
      code: EXIT.notReady,
      message: expect.stringContaining('retry in 300 s'),
    })
    expect(f).toHaveBeenCalledTimes(1)
  })
  it('tells a rate limit apart from a lock', async () => {
    respond(429, { error: 'too many requests' })
    await expect(login('http://localhost:8181', 'admin', 'pw')).rejects.toMatchObject({
      message: 'login rate-limited',
    })
  })
  it('includes remaining attempts on a 401 and never retries', async () => {
    const f = respond(401, { error: 'invalid credentials', remaining_attempts: 2 })
    await expect(login('http://localhost:8181', 'admin', 'pw')).rejects.toMatchObject({
      message: expect.stringContaining('2 attempts left'),
    })
    expect(f).toHaveBeenCalledTimes(1)
  })
})

describe('error table', () => {
  const samples = {
    stackDown: ERRORS.stackDown('http://localhost:8181'),
    portInUse: ERRORS.portInUse(8181),
    locked: ERRORS.locked('.live/record.lock'),
    markerMissing: ERRORS.markerMissing(),
    nonSeed: ERRORS.nonSeed('3 rows'),
    dbRefused: ERRORS.dbRefused('nasiko_dev'),
    dirty: ERRORS.dirty('../nasiko-cloud-rs', 2),
    loginFailed: ERRORS.loginFailed('admin', 401, ''),
    accountLocked: ERRORS.accountLocked('admin', 120),
    rateLimited: ERRORS.rateLimited(),
    pgvector: ERRORS.pgvector(),
    tempo: ERRORS.tempo('no image'),
    eeIsOss: ERRORS.eeIsOss('http://localhost:9090'),
    unknownFeature: ERRORS.unknownFeature('x', ['tokenops']),
    secret: ERRORS.secret('data[0].token'),
    serverExited: ERRORS.serverExited('.live/contract-server.log'),
  }
  it('covers every entry', () => {
    expect(Object.keys(samples).sort()).toEqual(Object.keys(ERRORS).sort())
  })
  it.each(Object.entries(samples))('%s names a problem, a cause and a runnable command', (_, e) => {
    expect(e.problem.length).toBeGreaterThan(5)
    expect(e.cause.length).toBeGreaterThan(5)
    expect(e.fix).toMatch(
      /^(npm run|node scripts\/|git -C|lsof|rm |docker |ADMIN_USERNAME=|sleep \d+ &&|tail |add the key)/,
    )
  })
})

describe('seed-live guards', () => {
  it("defaults to nasiko_dev with no marker (today's seed:live)", () => {
    const a = parseSeedArgs([])
    expect(a).toMatchObject({ database: 'nasiko_dev', edition: 'oss', reset: false })
    expect(sqlFlags(a as Exclude<typeof a, 'help'>)).toEqual([])
  })
  it('adds the marker for the contract and EE databases', () => {
    expect(
      sqlFlags(
        parseSeedArgs([
          '--database',
          'nasiko_contract',
          '--anchor',
          '2026-09-28T12:00:00Z',
        ]) as never,
      ),
    ).toEqual(['--anchor', '2026-09-28T12:00:00Z', '--marker'])
    expect(sqlFlags(parseSeedArgs(['--edition', 'ee']) as never)).toEqual(['--marker'])
  })
  it('refuses unknown databases and EE on nasiko_dev', () => {
    expect(() => parseSeedArgs(['--database', 'nasiko_prod'])).toThrow(/refusing database/)
    expect(() => parseSeedArgs(['--edition', 'ee', '--database', 'nasiko_dev'])).toThrow(
      /requires --database nasiko_ee/,
    )
    expect(() => parseSeedArgs(['--anchor', 'yesterday'])).toThrow(/ISO/)
  })
  it('passes --reset alone', () => {
    expect(sqlFlags(parseSeedArgs(['--reset', '--database', 'nasiko_contract']) as never)).toEqual([
      '--reset',
    ])
  })
})

describe('scripts on unhappy paths', () => {
  afterEach(() => vi.unstubAllGlobals())

  describe('login: the remaining failures (never retried)', () => {
    it('a refused connection, a lock with no wait, a plain-text 500 and a 200 without a token each fail at login', async () => {
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => {
          throw new Error('ECONNREFUSED')
        }),
      )
      await expect(login('http://localhost:8181', 'admin', 'pw')).rejects.toMatchObject({
        stage: 'login',
        code: EXIT.notReady,
        message: expect.stringContaining('ECONNREFUSED'),
        fix: expect.stringMatching(/^npm run record:live/),
      })

      const respond = (status: number, text: string) =>
        vi.stubGlobal(
          'fetch',
          vi.fn(async () => new Response(text, { status })),
        )
      respond(429, JSON.stringify({ code: 'account_locked' }))
      await expect(login('http://localhost:8181', 'admin', 'pw')).rejects.toMatchObject({
        message: 'account admin is locked after too many failed logins',
        fix: expect.stringMatching(/^sleep 60 && /),
      })
      respond(500, 'internal error')
      await expect(login('http://localhost:8181', 'admin', 'pw')).rejects.toMatchObject({
        message: 'login as admin failed: HTTP 500',
      })
      respond(200, JSON.stringify({ user_id: 'u1' }))
      await expect(login('http://localhost:8181', 'admin', 'pw')).rejects.toMatchObject({
        message: 'login answered without a token',
      })
    })
  })

  describe('pure guards and encoders', () => {
    it("scrub keeps a credential key's JSON type; the stack refuses the wrong database or Redis DB 0 before any docker call; OTLP attributes encode by type", () => {
      expect(
        scrubBody({
          token: 42,
          password: null,
          access_key: { nested: 'x' },
          Authorization: 'Bearer x',
        }),
      ).toEqual({ token: 0, password: null, access_key: '<redacted>', Authorization: '<redacted>' })

      expect(() => recreateThrowaway('nasiko_dev')).toThrow(/refusing to drop nasiko_dev/)
      expect(() => dropThrowaway('nasiko_ee')).toThrow(/refusing to drop nasiko_ee/)
      expect(() => ensureDatabase('nasiko_contract')).toThrow(/refusing to create nasiko_contract/)
      expect(() => flushRedisDb(0)).toThrow(/Redis DB 0/)
      expect(() => composeFile(join(tmpdir(), 'no-such-cloud-rs'))).toThrow(
        expect.objectContaining({
          stage: 'stack',
          fix: expect.stringMatching(/^export NASIKO_CLOUD_RS=/),
        }),
      )

      expect([attr('a', null), attr('a', undefined), attr('a', '')]).toEqual([null, null, null])
      expect(attr('b', false)).toEqual({ key: 'b', value: { boolValue: false } })
      expect(attr('i', 7)).toEqual({ key: 'i', value: { intValue: '7' } })
      expect(attr('d', 0.5)).toEqual({ key: 'd', value: { doubleValue: 0.5 } })
      expect(attr('o', [{ role: 'user' }])).toEqual({
        key: 'o',
        value: { stringValue: '[{"role":"user"}]' },
      })
    })
  })

  describe('seed-live arguments', () => {
    const A = '2026-09-28T12:00:00Z'
    it('--otlp is for contract/EE databases, needs --anchor and a localhost URL; bad edition, unknown flags and --help', () => {
      expect(() => parseSeedArgs(['--otlp', 'http://127.0.0.1:14318'])).toThrow(
        /contract and EE databases only/,
      )
      expect(() =>
        parseSeedArgs(['--database', 'nasiko_contract', '--otlp', 'http://127.0.0.1:14318']),
      ).toThrow(/needs --anchor/)
      expect(() =>
        parseSeedArgs([
          '--database',
          'nasiko_contract',
          '--anchor',
          A,
          '--otlp',
          'http://example.com',
        ]),
      ).toThrow(/not localhost/)
      expect(
        parseSeedArgs([
          '--database',
          'nasiko_contract',
          '--anchor',
          A,
          '--otlp',
          'http://127.0.0.1:14318',
        ]),
      ).toMatchObject({ otlp: 'http://127.0.0.1:14318', url: undefined })
      expect(() => parseSeedArgs(['--edition', 'cloud'])).toThrow(
        expect.objectContaining({ code: EXIT.usage, message: expect.stringMatching(/oss or ee/) }),
      )
      expect(() => parseSeedArgs(['--bogus'])).toThrow(
        expect.objectContaining({ code: EXIT.usage }),
      )
      expect(parseSeedArgs(['--help'])).toBe('help')
    })

    it('postTraces posts to /v1/traces and counts traces; a failed POST names the --no-traces fix', async () => {
      const f = vi.fn(async () => new Response('', { status: 200 }))
      vi.stubGlobal('fetch', f)
      const n = await postTraces('http://127.0.0.1:14318', A)
      expect(n).toBeGreaterThan(0)
      expect(f).toHaveBeenCalled()
      expect(
        f.mock.calls.every(
          (c) => String((c as unknown[])[0]) === 'http://127.0.0.1:14318/v1/traces',
        ),
      ).toBe(true)
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => new Response('tempo says no', { status: 500 })),
      )
      await expect(postTraces('http://127.0.0.1:14318', A)).rejects.toMatchObject({
        stage: 'otlp',
        message: expect.stringMatching(/answered 500: tempo says no/),
        fix: expect.stringContaining('--no-traces'),
      })
    })
  })
})
