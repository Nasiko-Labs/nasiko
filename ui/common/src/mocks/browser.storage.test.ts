/** Mock mode re-reads its session when another tab clears all storage (ship coverage audit). */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { setupPinnedSeed } from '@/test/pinnedSeed'
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
