/**
 * The session helpers' fallbacks (plans/feat-app-shell.md review D1, D2): the local sign-out
 * barrier with working, empty and blocked storage, the cross-tab channel when the browser has
 * no BroadcastChannel, the sign-in generation, and the session lock (serialised and bounded).
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  bumpSignInGeneration,
  SESSION_LOCK,
  SESSION_LOCK_WAIT_MS,
  signedOutMark,
  signInGeneration,
  SIGNIN_GENERATION_KEY,
  withSessionLock,
} from './session'

/** In-memory Storage (Node 24's global localStorage shadows jsdom's and isn't usable here). */
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

const blocked = (): Storage => {
  const fail = () => {
    throw new Error('blocked')
  }
  return { length: 0, clear: fail, getItem: fail, key: fail, removeItem: fail, setItem: fail }
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  vi.resetModules()
})

/** A LockManager that serialises per name, like the browser's. */
function fakeLocks() {
  const tails = new Map<string, Promise<unknown>>()
  const names: string[] = []
  return {
    names,
    request: (name: string, _opts: unknown, fn: () => Promise<unknown>) => {
      names.push(name)
      const prev = tails.get(name) ?? Promise.resolve()
      const run = prev.then(
        () => fn(),
        () => fn(),
      )
      tails.set(
        name,
        run.catch(() => {}),
      )
      return run
    },
  }
}

describe('local sign-out barrier (review D1)', () => {
  it('is set by a failed logout and lifted by a sign-in', async () => {
    vi.stubGlobal('localStorage', memoryStorage())
    const s = await import('./session')
    expect(s.isSignedOutLocally()).toBe(false)
    s.markSignedOutLocally()
    expect(localStorage.getItem(s.SIGNED_OUT_KEY)).toBe('1')
    expect(s.isSignedOutLocally()).toBe(true)
    s.clearSignedOutMark()
    expect(s.isSignedOutLocally()).toBe(false)
  })

  it('with storage blocked there is no barrier, and nothing throws', async () => {
    vi.stubGlobal('localStorage', blocked())
    const s = await import('./session')
    expect(() => s.markSignedOutLocally()).not.toThrow()
    expect(s.isSignedOutLocally()).toBe(false)
    expect(() => s.clearSignedOutMark()).not.toThrow()
  })
})

describe('signing-out flag', () => {
  it('reports what sign out set', async () => {
    const s = await import('./session')
    expect(s.isSigningOut()).toBe(false)
    s.setSigningOut(true)
    expect(s.isSigningOut()).toBe(true)
    s.setSigningOut(false)
    expect(s.isSigningOut()).toBe(false)
  })
})

describe('cross-tab channel without BroadcastChannel (review D2)', () => {
  it('broadcasting is a no-op and subscribing returns a working unsubscribe', async () => {
    vi.stubGlobal('BroadcastChannel', undefined)
    const s = await import('./session')
    expect(() => s.broadcastSession({ type: 'signed-in' })).not.toThrow()
    const listener = vi.fn()
    const stop = s.onSessionMessage(listener)
    expect(() => stop()).not.toThrow()
    expect(listener).not.toHaveBeenCalled()
  })

  it('a channel that throws on post is swallowed', async () => {
    class Broken {
      postMessage() {
        throw new Error('closed')
      }
      addEventListener() {}
      removeEventListener() {}
    }
    vi.stubGlobal('BroadcastChannel', Broken)
    const s = await import('./session')
    expect(() => s.broadcastSession({ type: 'signed-out', failed: false })).not.toThrow()
  })
})

describe('sign-in generation', () => {
  it('starts at "0" and changes on every sign-in', () => {
    expect(signInGeneration()).toBe('0')
    bumpSignInGeneration()
    const first = signInGeneration()
    expect(first).not.toBe('0')
    expect(localStorage.getItem(SIGNIN_GENERATION_KEY)).toBe(first)
    bumpSignInGeneration()
    expect(signInGeneration()).not.toBe(first)
  })

  it('reads "0" and bumps silently when storage is blocked; the barrier is unknown (null)', () => {
    vi.stubGlobal('localStorage', blocked())
    expect(() => bumpSignInGeneration()).not.toThrow()
    expect(signInGeneration()).toBe('0')
    expect(signedOutMark()).toBeNull()
  })
})

describe('the session lock', () => {
  it('serialises work under one name: the second waits for the first', async () => {
    const locks = fakeLocks()
    const order: string[] = []
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    const a = withSessionLock(async () => {
      await gate
      order.push('logout')
    }, locks as never)
    const b = withSessionLock(async () => {
      order.push('login')
    }, locks as never)
    // One macrotask, no real delay: every microtask a lock that didn't serialise would run has run by then.
    await new Promise((r) => setImmediate(r))
    expect(order).toEqual([])
    release()
    await Promise.all([a, b])
    expect(order).toEqual(['logout', 'login'])
    expect(locks.names).toEqual([SESSION_LOCK, SESSION_LOCK])
  })

  it('asks for the lock with an abort signal that fires after SESSION_LOCK_WAIT_MS', async () => {
    const timeouts = vi.spyOn(AbortSignal, 'timeout')
    let seen: { signal?: AbortSignal } | undefined
    const locks = {
      request: (_n: string, opts: { signal?: AbortSignal }, fn: () => Promise<unknown>) => (
        (seen = opts),
        fn()
      ),
    }
    await withSessionLock(async () => 'ok', locks as never)
    expect(seen?.signal).toBeInstanceOf(AbortSignal)
    expect(timeouts).toHaveBeenCalledWith(SESSION_LOCK_WAIT_MS)
  })
})
