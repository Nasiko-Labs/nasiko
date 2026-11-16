import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Breadcrumb, ErrorEvent } from '@sentry/react'
import { ApiError } from '@nasiko/data-access/client'
import {
  attachSink,
  captureEarlyErrors,
  report,
  resetForTests,
  stopReporting,
  type CapturedError,
} from './queue'
import { parseSettings } from './settings'
import {
  isExpectedFailure,
  redactText,
  redactUrl,
  scrubBreadcrumb,
  scrubEvent,
  scrubLog,
  scrubSpan,
  type LogRecord,
  type StreamedSpan,
} from './scrub'

afterEach(() => resetForTests())

describe('the early-error queue', () => {
  it('holds failures until the SDK starts, then replays them in order and reports straight through', () => {
    report({ error: new Error('first'), handled: false })
    report({ error: new Error('second'), handled: true })
    const seen: CapturedError[] = []
    attachSink((c) => seen.push(c))
    report({ error: new Error('third'), handled: false })
    expect(seen.map((c) => (c.error as Error).message)).toEqual(['first', 'second', 'third'])
  })

  it('holds at most 20, so an error loop before the SDK loads cannot grow memory', () => {
    for (let i = 0; i < 50; i++) report({ error: i, handled: false })
    const seen: CapturedError[] = []
    attachSink((c) => seen.push(c))
    expect(seen).toHaveLength(20)
  })

  it('drops and stops holding once reporting is off', () => {
    report({ error: 'before', handled: false })
    stopReporting()
    report({ error: 'after', handled: false })
    const seen: CapturedError[] = []
    attachSink((c) => seen.push(c))
    expect(seen).toEqual([])
  })

  it('catches page-wide errors and rejections, and lets go of them when the SDK takes over', () => {
    const target = new EventTarget() as Window
    captureEarlyErrors(target)
    const boom = new Error('boom')
    target.dispatchEvent(Object.assign(new Event('error'), { error: boom, message: 'boom' }))
    target.dispatchEvent(Object.assign(new Event('unhandledrejection'), { reason: 'nope' }))
    const seen: CapturedError[] = []
    attachSink((c) => seen.push(c))
    expect(seen.map((c) => c.error)).toEqual([boom, 'nope'])
    // The SDK has its own listeners now; these must not report a second time.
    target.dispatchEvent(Object.assign(new Event('error'), { error: new Error('later') }))
    expect(seen).toHaveLength(2)
  })
})

describe('parseSettings', () => {
  const on = {
    data: {
      enabled: true,
      dsn: 'https://k@o1.ingest.de.sentry.io/2',
      environment: 'production',
      tunnel: '/api/telemetry/sentry',
      traces_sample_rate: 0.1,
      replays_session_sample_rate: 0,
      replays_on_error_sample_rate: 1,
    },
  }

  it('reads what the server sends', () => {
    expect(parseSettings(on)).toEqual({
      dsn: 'https://k@o1.ingest.de.sentry.io/2',
      environment: 'production',
      tunnel: '/api/telemetry/sentry',
      tracesSampleRate: 0.1,
      replaysSessionSampleRate: 0,
      replaysOnErrorSampleRate: 1,
      logs: false,
      workspace: null,
    })
  })

  it('reads whether logs are on and the workspace, and ignores anything else in their place', () => {
    const s = parseSettings({ data: { ...on.data, logs: true, workspace: 't-123' } })
    expect(s?.logs).toBe(true)
    expect(s?.workspace).toBe('t-123')
    const odd = parseSettings({ data: { ...on.data, logs: 'yes', workspace: 42 } })
    expect(odd?.logs).toBe(false)
    expect(odd?.workspace).toBeNull()
  })

  it('is off for "off", for anything malformed, and for a tunnel outside the API', () => {
    expect(parseSettings({ data: { enabled: false } })).toBeNull()
    expect(parseSettings(null)).toBeNull()
    expect(parseSettings('<html>')).toBeNull()
    expect(parseSettings({ data: { ...on.data, dsn: 42 } })).toBeNull()
    expect(
      parseSettings({ data: { ...on.data, tunnel: 'https://evil.example/collect' } }),
    ).toBeNull()
  })

  it('reads a rate outside 0..1 as 0, never as "record everything"', () => {
    expect(parseSettings({ data: { ...on.data, traces_sample_rate: 5 } })?.tracesSampleRate).toBe(0)
  })
})

describe('scrubbing', () => {
  it('redacts credentials in paths and query values, keeping the rest', () => {
    expect(redactUrl('https://cp.example.com/api/mcp/s/agent-secret?x=1')).toBe(
      'https://cp.example.com/api/mcp/s/%7Btoken%7D',
    )
    expect(redactUrl('/login?redirect=%2Fagents&token=abc.def')).toBe(
      '/login?redirect=%2Fagents&token=[REDACTED]',
    )
    expect(redactUrl('/agents/42?tab=builds#top')).toBe('/agents/42?tab=builds')
    expect(redactUrl('not a url at all')).toBe('not a url at all')
  })

  it('treats a 4xx the page explains, aborts and an unreachable network as expected, never a 5xx or 429', () => {
    expect(isExpectedFailure(new ApiError(404, null, '/api/x', 'GET /api/x → 404'))).toBe(true)
    expect(isExpectedFailure(new ApiError(502, null, '/api/x', 'network error'))).toBe(true)
    expect(isExpectedFailure(new DOMException('aborted', 'AbortError'))).toBe(true)
    expect(isExpectedFailure(new ApiError(429, null, '/api/x', 'slow down'))).toBe(false)
    expect(isExpectedFailure(new ApiError(500, { error: 'x' }, '/api/x', 'boom'))).toBe(false)
    expect(isExpectedFailure(new TypeError('x is undefined'))).toBe(false)
  })

  it('drops expected failures and strips cookies, bodies and headers from the rest', () => {
    const event = {
      request: {
        url: 'https://cp.example.com/settings?code=oauth',
        cookies: { access_token: 'secret' },
        data: '{"password":"hunter2"}',
        headers: {
          'User-Agent': 'UA',
          Referer: 'https://cp.example.com/login?token=t',
          Authorization: 'Bearer x',
        },
      },
    } as unknown as ErrorEvent
    expect(
      scrubEvent(event, { originalException: new ApiError(403, null, '/api/x', 'no') }),
    ).toBeNull()
    const scrubbed = scrubEvent(event, { originalException: new TypeError('bug') })!
    expect(scrubbed.request).toEqual({
      url: 'https://cp.example.com/settings?code=[REDACTED]',
      query_string: undefined,
      cookies: undefined,
      data: undefined,
      headers: { 'User-Agent': 'UA', Referer: 'https://cp.example.com/login?token=[REDACTED]' },
    })
  })

  it('redacts URLs in a streamed span name and attributes, and keeps only the user agent header', () => {
    const span = {
      trace_id: 't',
      span_id: 's',
      name: 'GET https://cp.example.com/api/mcp/s/agent-secret?x=1',
      start_timestamp: 0,
      status: 'ok',
      is_segment: false,
      attributes: {
        'url.full': 'https://cp.example.com/api/auth/sso/session?token=jwt&page=2',
        'url.query': '?code=oauth&tab=a',
        'http.query': 'state=s',
        'http.request.header.user_agent': 'UA',
        'http.request.header.authorization': 'Bearer x',
        'http.response.header.set_cookie': 'access_token=x',
        'http.response.status_code': 200,
        'sentry.op': 'http.client',
      },
    } as StreamedSpan
    const scrubbed = scrubSpan(span)
    expect(scrubbed.name).toBe('GET https://cp.example.com/api/mcp/s/%7Btoken%7D')
    expect(scrubbed.attributes).toEqual({
      'url.full': 'https://cp.example.com/api/auth/sso/session?token=[REDACTED]&page=2',
      'url.query': '?code=[REDACTED]&tab=a',
      'http.query': 'state=[REDACTED]',
      'http.request.header.user_agent': 'UA',
      'http.response.status_code': 200,
      'sentry.op': 'http.client',
    })
  })

  it('redacts navigation and request breadcrumbs, and cuts console lines to their first line', () => {
    const nav = scrubBreadcrumb({
      category: 'navigation',
      data: { from: '/a?token=1', to: '/b' },
    } as Breadcrumb)
    expect(nav?.data).toEqual({ from: '/a?token=[REDACTED]', to: '/b' })
    const consoleLine = scrubBreadcrumb({
      category: 'console',
      message: 'first\nsecond',
      data: { arguments: [1] },
    })
    expect(consoleLine).toEqual({ category: 'console', message: 'first', data: undefined })
  })
})

describe('console lines sent as logs', () => {
  it('redacts URLs and email addresses in free text', () => {
    expect(redactText('load /api/auth/sso/session?token=jwt&page=2 for ada@acme.com failed')).toBe(
      'load /api/auth/sso/session?token=[REDACTED]&page=2 for [email] failed',
    )
    expect(redactText('plain words stay')).toBe('plain words stay')
  })

  it("cleans the message and string attributes, keeping the SDK's own and the user id", () => {
    const log = {
      level: 'warn',
      message: 'retrying https://x.example/cb?code=abc',
      attributes: {
        'sentry.release': 'r@1',
        'user.id': 'u-1',
        auth_token: 'abc',
        url: '/p?session=s&x=1',
        attempt: 3,
      },
    } as LogRecord
    const out = scrubLog(log)
    expect(out.message).toBe('retrying https://x.example/cb?code=[REDACTED]')
    expect(out.attributes).toEqual({
      'sentry.release': 'r@1',
      'user.id': 'u-1',
      auth_token: '[REDACTED]',
      url: '/p?session=[REDACTED]&x=1',
      attempt: 3,
    })
  })
})

describe('startErrorReporting', () => {
  it('never asks the server in the dev server or mock mode', async () => {
    vi.resetModules()
    const apiFetch = vi.fn()
    vi.doMock('@nasiko/data-access/client', async (orig) => ({
      ...(await orig<object>()),
      apiFetch,
    }))
    const { startErrorReporting } = await import('./start')
    startErrorReporting({} as never, 'oss')
    await new Promise((r) => setTimeout(r, 10))
    expect(apiFetch).not.toHaveBeenCalled()
    vi.doUnmock('@nasiko/data-access/client')
  })
})

describe('startErrorReporting in a live build', () => {
  async function startWith(body: unknown) {
    vi.resetModules()
    vi.stubEnv('DEV', false)
    vi.stubGlobal('requestIdleCallback', (task: () => void) => task())
    const startSentry = vi.fn()
    vi.doMock('@nasiko/app-core/env', async (orig) => ({
      ...(await orig<object>()),
      env: { mode: 'live' },
    }))
    vi.doMock('@nasiko/data-access/client', async (orig) => ({
      ...(await orig<object>()),
      apiFetch: vi.fn(() => Promise.resolve(body)),
    }))
    vi.doMock('./sentry', () => ({ startSentry }))
    const { startErrorReporting } = await import('./start')
    startErrorReporting({} as never, 'ee')
    await new Promise((r) => setTimeout(r, 10))
    return startSentry
  }

  afterEach(() => {
    vi.unstubAllEnvs()
    vi.unstubAllGlobals()
    vi.doUnmock('@nasiko/app-core/env')
    vi.doUnmock('@nasiko/data-access/client')
    vi.doUnmock('./sentry')
  })

  it('loads the SDK only when the server says reporting is on', async () => {
    expect(await startWith({ data: { enabled: false } })).not.toHaveBeenCalled()
    const startSentry = await startWith({
      data: {
        enabled: true,
        dsn: 'https://k@o1.ingest.sentry.io/2',
        environment: 'prod',
        tunnel: '/api/telemetry/sentry',
      },
    })
    expect(startSentry).toHaveBeenCalledWith(
      expect.objectContaining({ environment: 'prod' }),
      {},
      'ee',
    )
  })
})
