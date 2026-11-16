/** The API client: ApiError message extraction, abort/envelope handling, the deadline, Retry-After and the API base. */
import { delay, http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { server } from '@nasiko/testing/setup'
import { ApiError, apiData, apiFetch, apiUrl, fetchApi, setApiBase } from './client'

describe('ApiError.serverMessage', () => {
  it('prefers trimmed text, then JSON message, then JSON error, else null', () => {
    expect(new ApiError(400, '  bad agent  ', '/p', 'm').serverMessage).toBe('bad agent')
    expect(new ApiError(400, '   ', '/p', 'm').serverMessage).toBeNull()
    expect(new ApiError(500, { message: 'boom', error: 'ignored' }, '/p', 'm').serverMessage).toBe(
      'boom',
    )
    expect(new ApiError(401, { error: 'invalid credentials' }, '/p', 'm').serverMessage).toBe(
      'invalid credentials',
    )
    expect(new ApiError(500, { message: 42 }, '/p', 'm').serverMessage).toBeNull()
    expect(new ApiError(500, null, '/p', 'm').serverMessage).toBeNull()
    expect(new ApiError(502, '<!doctype html><p>bad gateway', '/p', 'm').serverMessage).toBeNull()
  })
})

describe('ApiError.code', () => {
  it('reads {code}, then a JSON-RPC data.code, then its code; numbers as strings', () => {
    expect(new ApiError(400, { error: 'x', code: 'agent_not_shareable' }, '/p', 'm').code).toBe(
      'agent_not_shareable',
    )
    expect(
      new ApiError(
        400,
        { error: { code: -32602, message: 'm', data: { code: 'bad_part' } } },
        '/p',
        'm',
      ).code,
    ).toBe('bad_part')
    expect(new ApiError(503, { error: { code: -32609, message: 'm' } }, '/p', 'm').code).toBe(
      '-32609',
    )
    expect(new ApiError(500, 'plain', '/p', 'm').code).toBeNull()
    expect(new ApiError(500, null, '/p', 'm').code).toBeNull()
  })
})

describe('apiFetch / apiData', () => {
  it('rethrows an abort as-is; any other fetch failure becomes a 502 network error', async () => {
    // Stub fetch: under jsdom, Node's fetch rejects with a DOMException from another realm,
    // so this exercises the branch as a browser would hit it.
    vi.spyOn(globalThis, 'fetch').mockRejectedValueOnce(new DOMException('aborted', 'AbortError'))
    const err = await apiFetch('/api/slow').catch((e: unknown) => e)
    vi.restoreAllMocks()
    expect(err).not.toBeInstanceOf(ApiError)
    expect((err as Error).name).toBe('AbortError')

    // Any other fetch failure is a 502 "network error" (server unreachable).
    vi.spyOn(globalThis, 'fetch').mockRejectedValueOnce(new TypeError('Failed to fetch'))
    const net = await apiFetch('/api/x', { method: 'POST' }).catch((e: unknown) => e)
    vi.restoreAllMocks()
    expect(net).toBeInstanceOf(ApiError)
    expect((net as ApiError).isServerUnreachable).toBe(true)
    expect((net as ApiError).message).toBe("Couldn't reach Open Runtime (POST /api/x)")
  })

  it("says the server's own reason as the error message, else the status and the request", async () => {
    server.use(
      http.get('/api/rpc', () =>
        HttpResponse.json(
          { jsonrpc: '2.0', id: null, error: { code: -32603, message: 'OPENAI_API_KEY not set' } },
          { status: 500 },
        ),
      ),
      http.get('/api/env', () =>
        HttpResponse.json(
          {
            data: null,
            status_code: 502,
            message: 'OpenAI rejected your saved API key',
            code: 'llm_key_rejected',
          },
          { status: 502 },
        ),
      ),
      http.get('/api/html', () => new HttpResponse('<html>Bad gateway</html>', { status: 502 })),
      http.get('/api/long', () => new HttpResponse('x'.repeat(600), { status: 500 })),
    )
    const rpc = (await apiFetch('/api/rpc').catch((e: unknown) => e)) as ApiError
    expect(rpc.message).toBe('OPENAI_API_KEY not set')
    expect(rpc.code).toBe('-32603')
    const env = (await apiFetch('/api/env').catch((e: unknown) => e)) as ApiError
    expect(env.message).toBe('OpenAI rejected your saved API key')
    expect(env.code).toBe('llm_key_rejected')
    const html = (await apiFetch('/api/html').catch((e: unknown) => e)) as ApiError
    expect(html.message).toBe('HTTP 502 from GET /api/html')
    expect(html.serverMessage).toBeNull()
    // A dump is no sentence: the message falls back, the body stays readable through serverMessage.
    const long = (await apiFetch('/api/long').catch((e: unknown) => e)) as ApiError
    expect(long.message).toBe('HTTP 500 from GET /api/long')
    expect(long.serverMessage).toHaveLength(600)
  })

  it.each([
    ['an empty 204', () => new HttpResponse(null, { status: 204 })],
    ['an HTML fallback page', () => new HttpResponse('<!doctype html>', { status: 200 })],
    ['a null data field', () => HttpResponse.json({ data: null, status_code: 200 })],
  ])('apiData fails loudly on %s', async (_, reply) => {
    server.use(http.get('/api/thing', reply))
    const err = await apiData('/api/thing').catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect((err as ApiError).message).toMatch(/no data envelope/)
  })
})

describe('apiFetch deadline and Retry-After', () => {
  it('fails with a TimeoutError when the server takes longer than the deadline', async () => {
    server.use(
      http.get('/api/slow', async () => {
        // Never answers: the deadline or the abort ends the call.
        await delay('infinite')
        return HttpResponse.json({})
      }),
    )
    const err = await apiFetch('/api/slow', { timeout: 20 }).catch((e: unknown) => e)
    expect((err as Error).name).toBe('TimeoutError')
  })

  it("keeps the caller's abort distinct from the deadline", async () => {
    server.use(
      http.get('/api/slow', async () => {
        // Never answers: the deadline or the abort ends the call.
        await delay('infinite')
        return HttpResponse.json({})
      }),
    )
    const ctrl = new AbortController()
    const p = apiFetch('/api/slow', { signal: ctrl.signal }).catch((e: unknown) => e)
    ctrl.abort()
    expect(((await p) as Error).name).toBe('AbortError')
  })

  it('reads Retry-After and marks 403 as forbidden, not retryable', async () => {
    server.use(
      http.get(
        '/api/busy',
        () => new HttpResponse('slow down', { status: 429, headers: { 'Retry-After': '7' } }),
      ),
    )
    const busy = (await apiFetch('/api/busy').catch((e: unknown) => e)) as ApiError
    expect(busy.retryAfterSeconds).toBe(7)
    expect(busy.isRetryable).toBe(true)
    const no = new ApiError(403, 'requires admin role', '/p', 'm')
    expect(no.isForbidden).toBe(true)
    expect(no.isRetryable).toBe(false)
  })
})

describe('the API base', () => {
  afterEach(() => setApiBase(''))

  it('is the page origin until an edition sets one; only another origin gets the cookie cross-origin', async () => {
    const sent: [string, RequestCredentials | undefined][] = []
    vi.spyOn(globalThis, 'fetch').mockImplementation((input, init) => {
      sent.push([String(input), init?.credentials])
      return Promise.resolve(new Response('{}'))
    })
    await apiFetch('/api/me')
    setApiBase('https://acme.app.nasiko.com')
    await apiFetch('/api/me')
    await fetchApi('/api/agents/a/logs/stream')
    // An absolute URL is the edition's own call to the page origin, whatever the base.
    await apiFetch(new URL('/api/me/workspaces', location.origin).href)
    expect(sent).toEqual([
      [`${location.origin}/api/me`, 'same-origin'],
      ['https://acme.app.nasiko.com/api/me', 'include'],
      ['https://acme.app.nasiko.com/api/agents/a/logs/stream', 'include'],
      [`${location.origin}/api/me/workspaces`, 'same-origin'],
    ])
    expect(apiUrl('/api/chat/files/f/download').href).toBe(
      'https://acme.app.nasiko.com/api/chat/files/f/download',
    )
  })
})
