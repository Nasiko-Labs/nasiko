/** Mock mode re-reads its session when another tab clears all storage (ship coverage audit). */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { configureMocks } from './handlers'
import { startMocks } from './browser'

vi.mock('msw/browser', () => ({ setupWorker: () => ({ start: async () => {} }) }))
setupPinnedSeed()

const KEY = 'ui-lab:mock-signed-out'
const url = (p: string) => new URL(p, globalThis.location.origin).toString()
afterEach(() => configureMocks({ persistLogin: null, loggedIn: true }))

describe('storage cleared in another tab (key null)', () => {
  it('re-reads the stored mock sign-out instead of assuming either way', async () => {
    await startMocks({ mode: 'mock', partialMocks: [] } as never)
    expect((await fetch(url('/api/me'))).status).toBe(200)
    localStorage.setItem(KEY, '1')
    window.dispatchEvent(new StorageEvent('storage', { key: null }))
    expect((await fetch(url('/api/me'))).status).toBe(401)
    localStorage.clear()
    window.dispatchEvent(new StorageEvent('storage', { key: null }))
    expect((await fetch(url('/api/me'))).status).toBe(200)
  })

  it('ignores unrelated keys', async () => {
    await startMocks({ mode: 'mock', partialMocks: [] } as never)
    localStorage.setItem(KEY, '1')
    window.dispatchEvent(
      new StorageEvent('storage', { key: 'openruntime.theme', newValue: 'dark' }),
    )
    expect((await fetch(url('/api/me'))).status).toBe(200)
  })
})

describe('a mocked sign-in round trip (?mock=signed-in)', () => {
  it('ends a stored mock sign-out, as a real sign-in sets the cookie again', async () => {
    localStorage.setItem(KEY, '1')
    // The page this load landed on (setupPinnedSeed unstubs it after each test).
    const { origin } = globalThis.location
    vi.stubGlobal('location', {
      origin,
      href: `${origin}/?mock=billing-grace,signed-in`,
      search: '?mock=billing-grace,signed-in',
    })
    await startMocks({ mode: 'mock', partialMocks: [] } as never)
    expect((await fetch(url('/api/me'))).status).toBe(200)
    expect(localStorage.getItem(KEY)).toBeNull()
  })

  it('keeps a stored sign-out on an ordinary load', async () => {
    localStorage.setItem(KEY, '1')
    await startMocks({ mode: 'mock', partialMocks: [] } as never)
    expect((await fetch(url('/api/me'))).status).toBe(401)
  })
})
