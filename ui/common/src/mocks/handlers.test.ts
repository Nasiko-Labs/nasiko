/**
 * The mock handlers: partial-mock handler selection (live mode + VITE_NASIKO_MOCK), the auth
 * handlers (logout, login and /api/me agreeing on the user, the session persistence hook), and the
 * mock's fallback 500 for an unexpected aggregation failure.
 */
import { afterEach, describe, expect, it, onTestFinished, vi } from 'vitest'
import { apiFetch } from '@/lib/api/client'
import { configureMocks, handlerGroups, handlersFor } from './handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import type { Seed } from './seed'

setupPinnedSeed()

const url = (p: string) => new URL(p, globalThis.location.origin).toString()

afterEach(() => configureMocks({ seed, now, loggedIn: true, persistLogin: null }))

describe('handlersFor (partial mocks)', () => {
  it('returns exactly the handlers of the named groups, in order', () => {
    expect(handlersFor([])).toEqual([])
    expect(handlersFor(['top-traces'])).toEqual(handlerGroups['top-traces'])
    expect(handlersFor(['providers', 'spend-calendar'])).toEqual([
      ...handlerGroups.providers,
      ...handlerGroups['spend-calendar'],
    ])
    const paths = handlersFor(['top-traces']).map((h) => String(h.info.path))
    expect(paths).toEqual(['/api/observability/finops/top-traces'])
  })

  it("puts an edition's handlers for the named groups in front of every core handler", () => {
    const [own] = handlerGroups.dashboard
    const other = handlerGroups['top-traces'][0]!
    expect(
      handlersFor(['providers', 'dashboard'], { dashboard: [own!], 'top-traces': [other] }),
    ).toEqual([own, ...handlerGroups.providers, ...handlerGroups.dashboard])
  })
})

describe('auth handlers', () => {
  it('logout ends the mock session until the next login', async () => {
    configureMocks({ seed, now, loggedIn: true })
    expect((await fetch(url('/api/me'))).status).toBe(200)
    const out = await fetch(url('/api/auth/logout'), { method: 'POST' })
    expect(out.status).toBe(204)
    expect((await fetch(url('/api/me'))).status).toBe(401)
    expect((await fetch(url('/api/observability/finops/dashboard?range=7d'))).status).toBe(401)
    // Empty credentials are rejected like the server; real ones log back in.
    expect((await fetch(url('/api/auth/login'), { method: 'POST', body: '{}' })).status).toBe(401)
    expect(
      (
        await fetch(url('/api/auth/login'), {
          method: 'POST',
          body: JSON.stringify({ username: 'a', password: 'b' }),
        })
      ).status,
    ).toBe(200)
    expect((await fetch(url('/api/me'))).status).toBe(200)
  })

  it('an unexpected aggregation error is a plain-text 500, not a crash', async () => {
    configureMocks({ seed: { ...seed, traces: null } as unknown as Seed, now, loggedIn: true })
    const res = await fetch(url('/api/observability/finops/spend-timeseries?range=7d'))
    expect(res.status).toBe(500)
    expect(res.headers.get('content-type')).toBe('text/plain')
    expect(await res.text()).toBe('internal error')
  })
})

describe('mock login and /api/me agree on the user (the same-account check depends on it)', () => {
  // OSS: the session is the viewer (the seed admin by default, or the persona).
  it.each([null, 'sam'])(
    'persona %s: login user_id and username equal the me claims',
    async (persona) => {
      onTestFinished(() => configureMocks({ persona: null }))
      configureMocks({ persona, loggedIn: false })
      const login = await apiFetch<{ user_id: string; username: string }>('/api/auth/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ username: 'someone', password: 'x' }),
      })
      const me = await apiFetch<{ sub: string; username: string }>('/api/me')
      expect(login.user_id).toBe(me.sub)
      // auth/login.rs returns the DB row's username, not what was typed.
      expect(login.username).toBe(me.username)
    },
  )
})

// Regression: ISSUE-003 (/qa 2026-09-28, .gstack/qa-reports/qa-report-localhost-2026-09-28.md): a mock sign-out
// undid itself on the next full reload.
describe('mock session persistence (browser bootstrap hook)', () => {
  it('reports every sign-out and sign-in, so the browser can keep them across reloads', async () => {
    const persist = vi.fn()
    configureMocks({ seed, now, loggedIn: true, persistLogin: persist })
    expect((await fetch(url('/api/auth/logout'), { method: 'POST' })).status).toBe(204)
    expect(persist).toHaveBeenLastCalledWith(false)
    // A second logout with no session is the server's 401 and changes nothing.
    expect((await fetch(url('/api/auth/logout'), { method: 'POST' })).status).toBe(401)
    expect(persist).toHaveBeenCalledTimes(1)
    const login = await fetch(url('/api/auth/login'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username: 'admin', password: 'x' }),
    })
    expect(login.status).toBe(200)
    expect(persist).toHaveBeenLastCalledWith(true)
  })

  it('starts signed out when the bootstrap says the last session ended', async () => {
    configureMocks({ seed, now, loggedIn: false })
    expect((await fetch(url('/api/me'))).status).toBe(401)
  })
})
