/** The API client: ApiError message extraction, abort/envelope handling, the deadline and Retry-After. */
import { delay, http, HttpResponse } from 'msw'
import { describe, expect, it, vi } from 'vitest'
import { server } from '@/test/setup'
import { ApiError, apiData, apiFetch } from './client'

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
    expect((net as ApiError).message).toBe('POST /api/x → network error')
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
