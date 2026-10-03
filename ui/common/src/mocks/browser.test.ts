/**
 * The browser mock bootstrap (QA ISSUE-003): a mock sign-out survives a reload through
 * localStorage, blocked storage starts signed in, and partial-live mode never persists a session.
 * The service worker itself is stubbed; the handlers are the same ones msw/node serves in tests.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { configureMocks } from './handlers'
import { startMocks } from './browser'

const start = vi.fn(async () => {})
const setupWorker = vi.fn((..._handlers: unknown[]) => ({ start }))
vi.mock('msw/browser', () => ({ setupWorker: (...h: unknown[]) => setupWorker(...h) }))

setupPinnedSeed()

const KEY = 'ui-lab:mock-signed-out'
const url = (p: string) => new URL(p, globalThis.location.origin).toString()

function memoryStorage(): Storage {
  const m = new Map<string, string>()
  return {
    get length() {
      return m.size
    },
    clear: () => m.clear(),
    getItem: (k) => m.get(k) ?? null,
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, String(v)),
  }
}

afterEach(() => {
  configureMocks({ persistLogin: null, persona: null, lockPersona: false })
  setupWorker.mockClear()
  start.mockClear()
})

describe('startMocks', () => {
  it('starts signed out after a stored mock sign-out, and records the next sign-in', async () => {
    const store = memoryStorage()
    store.setItem(KEY, '1')
    vi.stubGlobal('localStorage', store)
    await startMocks({ mode: 'mock', partialMocks: [], legacyUiUrl: null, waitlistUrl: null })
    expect(setupWorker).toHaveBeenCalledTimes(1)
    expect(start).toHaveBeenCalledWith(expect.objectContaining({ onUnhandledRequest: 'bypass' }))
    expect((await fetch(url('/api/me'))).status).toBe(401)
    await fetch(url('/api/auth/login'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username: 'admin', password: 'x' }),
    })
    expect(store.getItem(KEY)).toBeNull()
    await fetch(url('/api/auth/logout'), { method: 'POST' })
    expect(store.getItem(KEY)).toBe('1')
  })

  it('starts signed in when storage is blocked, and a sign-out never throws', async () => {
    const fail = () => {
      throw new Error('blocked')
    }
    vi.stubGlobal('localStorage', {
      length: 0,
      clear: fail,
      getItem: fail,
      key: fail,
      removeItem: fail,
      setItem: fail,
    } satisfies Storage)
    await startMocks({ mode: 'mock', partialMocks: [], legacyUiUrl: null, waitlistUrl: null })
    expect((await fetch(url('/api/me'))).status).toBe(200)
    expect((await fetch(url('/api/auth/logout'), { method: 'POST' })).status).toBe(204)
  })

  it('does nothing in live mode with no partial mocks', async () => {
    await startMocks({ mode: 'live', partialMocks: [], legacyUiUrl: null, waitlistUrl: null })
    expect(setupWorker).not.toHaveBeenCalled()
  })

  it('partial-live mode starts only the requested groups and never persists a session', async () => {
    const store = memoryStorage()
    vi.stubGlobal('localStorage', store)
    await startMocks({
      mode: 'live',
      partialMocks: ['top-traces'],
      legacyUiUrl: null,
      waitlistUrl: null,
    })
    expect(setupWorker).toHaveBeenCalledTimes(1)
    const handlers = setupWorker.mock.calls[0]!
    expect(handlers.length).toBeGreaterThan(0)
    // No auth handlers in partial mode, and no /health (it isn't a MOCKABLE key, eng D4).
    const paths = handlers.map((h) => String((h as { info: { path: unknown } }).info.path))
    expect(paths.some((p) => p.includes('/api/auth'))).toBe(false)
    expect(paths).not.toContain('/health')
    await fetch(url('/api/auth/logout'), { method: 'POST' })
    expect(store.getItem(KEY)).toBeNull()
  })
})
