/**
 * signOut outside the page: in-flight mutations settle first and can't write afterwards, data refetched while logout
 * is pending is cleared, the logout 401 reasons are classified, a session lock that never comes fails safe, a newer
 * sign-in is never ended by an older sign-out, and whenSignedOut waits for the running sign-out.
 * (The login page's side: login.test.tsx; the session lock itself: src/lib/session.test.ts.)
 */
import { MutationObserver, QueryClient } from '@tanstack/react-query'
import { waitFor } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { bumpSignInGeneration, isSigningOut, setSigningOut, SIGNED_OUT_KEY } from '@/lib/session'
import { configureMocks } from '@/mocks/handlers'
import { FIXED, setupPinnedSeed } from '@/test/pinnedSeed'
import { recordRequests, server } from '@/test/setup'
import { endServerSession, MUTATION_SETTLE_MS, signOut, whenSignedOut } from './signOut'

setupPinnedSeed()
afterEach(() => setSigningOut(false))
afterEach(() => configureMocks({ variant: null }))

const rejectingLocks = () =>
  vi.stubGlobal('navigator', {
    ...navigator,
    locks: { request: () => Promise.reject(new DOMException('timed out', 'TimeoutError')) },
  })

const envelope = (message: string) =>
  HttpResponse.json({ data: null, status_code: 401, message }, { status: 401 })

describe('signOut', () => {
  it('waits for in-flight mutations so their writes are cleared, not left for the next account', async () => {
    const queryClient = new QueryClient()
    let finish!: () => void
    const mutation = queryClient.getMutationCache().build(queryClient, {
      mutationFn: () => new Promise<string>((r) => (finish = () => r('chat of account A'))),
      onSuccess: (data) => queryClient.setQueryData(['chat', 'sessions'], [data]),
    })
    const running = mutation.execute(undefined)
    const navigate = vi.fn(async () => {})
    // Fake timers: sign out must still be waiting just before its settle cap while the mutation is pending.
    vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout'], now: FIXED })
    let done: Promise<unknown>
    try {
      done = signOut({ queryClient, userId: 'u1', navigate })
      expect(isSigningOut()).toBe(true)
      await vi.advanceTimersByTimeAsync(MUTATION_SETTLE_MS - 1)
      expect(navigate).not.toHaveBeenCalled()
    } finally {
      vi.useFakeTimers({ toFake: ['Date'], now: FIXED })
    }
    finish()
    await running
    await done
    expect(queryClient.getQueryData(['chat', 'sessions'])).toBeUndefined()
    expect(navigate).toHaveBeenCalledWith({ to: '/login', search: {} })
    expect(isSigningOut()).toBe(false)
  })

  it(
    'drops the callbacks of a mutation that outlasts the wait',
    async () => {
      const queryClient = new QueryClient()
      let finish!: () => void
      const mutation = queryClient.getMutationCache().build(queryClient, {
        mutationFn: () => new Promise<string>((r) => (finish = () => r('chat of account A'))),
        onSuccess: (data) => queryClient.setQueryData(['chat', 'sessions'], [data]),
      })
      const running = mutation.execute(undefined)
      await signOut({ queryClient, userId: 'u1', navigate: async () => {} })
      // Signed out after the bounded wait; the late response must not write anything.
      finish()
      await running
      expect(queryClient.getQueryData(['chat', 'sessions'])).toBeUndefined()
    },
    MUTATION_SETTLE_MS + 3_000,
  )

  it('a mounted mutation that re-applies its callbacks still cannot write after sign-out', async () => {
    const queryClient = new QueryClient()
    let finish!: () => void
    const onSuccess = vi.fn(() => queryClient.setQueryData(['chat', 'sessions'], ['account A']))
    const options = {
      mutationFn: () => new Promise<string>((r) => (finish = () => r('A'))),
      onSuccess,
    }
    const observer = new MutationObserver(queryClient, options)
    const unsubscribe = observer.subscribe(() => {})
    const running = observer.mutate().catch(() => {})
    // navigate = the page re-rendering (re-applying its options) and then unmounting.
    const navigate = vi.fn(async () => {
      observer.setOptions(options)
      unsubscribe()
    })
    // The mutation outlasts sign-out's bounded wait.
    await signOut({ queryClient, userId: 'u1', navigate })
    finish()
    await running
    expect(onSuccess).not.toHaveBeenCalled()
    expect(queryClient.getQueryData(['chat', 'sessions'])).toBeUndefined()
  }, 8_000)

  it('drops data refetched while logout was pending', async () => {
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    const queryClient = new QueryClient()
    server.use(
      http.post(
        '/api/auth/logout',
        async () => {
          // A mounted page refetches while logout is in flight.
          queryClient.setQueryData(['probe'], 'refetched mid sign-out')
          await gate
          return new HttpResponse(null, { status: 204 })
        },
        { once: true },
      ),
    )
    const navigate = vi.fn(async () => {})
    const done = signOut({ queryClient, userId: 'u1', navigate })
    await waitFor(() => expect(queryClient.getQueryData(['probe'])).toBe('refetched mid sign-out'))
    release()
    await done
    expect(navigate).toHaveBeenCalled()
    expect(queryClient.getQueryData(['probe'])).toBeUndefined()
  })
})

describe('logout 401s', () => {
  it('a fail-closed 401 (server cannot check the token) is a failed logout', async () => {
    configureMocks({ variant: 'logout-unavailable' })
    expect(await endServerSession()).toBe('logout-failed')
  })

  it('a dead-session 401 is already signed out', async () => {
    configureMocks({ loggedIn: false })
    expect(await endServerSession()).toBe('signed-out')
  })

  it('an unknown 401 reason is treated as failed, never as signed out', async () => {
    server.use(
      http.post('/api/auth/logout', () => new HttpResponse('something new', { status: 401 }), {
        once: true,
      }),
    )
    expect(await endServerSession()).toBe('logout-failed')
  })

  it.each([
    'missing or invalid token',
    'invalid token',
    'token missing jti',
    'token revoked',
    'session user no longer exists',
  ])('envelope "%s" means already signed out', async (message) => {
    server.use(http.post('/api/auth/logout', () => envelope(message), { once: true }))
    expect(await endServerSession()).toBe('signed-out')
  })

  it('envelope "token validation unavailable" is a failed logout', async () => {
    server.use(
      http.post('/api/auth/logout', () => envelope('token validation unavailable'), { once: true }),
    )
    expect(await endServerSession()).toBe('logout-failed')
  })
})

describe('the session lock', () => {
  it('a lock that never comes fails the sign-out safe: barrier set, notice shown', async () => {
    rejectingLocks()
    const navigate = vi.fn(async () => {})
    const result = await signOut({ queryClient: new QueryClient(), userId: 'u1', navigate })
    expect(result).toBe('logout-failed')
    expect(localStorage.getItem(SIGNED_OUT_KEY)).toBe('1')
    expect(navigate).toHaveBeenCalledWith({ to: '/login', search: { signout: 'failed' } })
  })
})

describe('a newer sign-in wins', () => {
  it('a sign-in during this sign-out’s cleanup keeps its session: no logout is sent', async () => {
    const queryClient = new QueryClient()
    // A mutation keeps sign-out in its pre-lock cleanup while "another tab" signs in.
    let finish!: () => void
    const m = queryClient
      .getMutationCache()
      .build(queryClient, { mutationFn: () => new Promise<void>((r) => (finish = r)) })
    const running = m.execute(undefined)
    const reqs = recordRequests()
    try {
      const done = signOut({ queryClient, userId: 'u1', navigate: async () => {} })
      await vi.waitFor(() => expect(typeof finish).toBe('function'))
      bumpSignInGeneration()
      finish()
      await running
      expect(await done).toBe('superseded')
      expect(reqs.urls.filter((u) => u.pathname === '/api/auth/logout')).toHaveLength(0)
      expect(localStorage.getItem(SIGNED_OUT_KEY)).toBeNull()
    } finally {
      reqs.stop()
    }
  })

  it("a lock timeout after a newer sign-in is 'superseded': no barrier, and /login without the failure notice", async () => {
    rejectingLocks()
    const queryClient = new QueryClient()
    // A pending mutation holds sign-out in its pre-lock cleanup while "another tab" signs in.
    let finish!: () => void
    const m = queryClient
      .getMutationCache()
      .build(queryClient, { mutationFn: () => new Promise<void>((r) => (finish = r)) })
    const running = m.execute(undefined)
    const navigate = vi.fn(async () => {})
    const done = signOut({ queryClient, userId: 'u1', navigate })
    await vi.waitFor(() => expect(typeof finish).toBe('function'))
    bumpSignInGeneration()
    finish()
    await running
    expect(await done).toBe('superseded')
    expect(localStorage.getItem(SIGNED_OUT_KEY)).toBeNull()
    expect(navigate).toHaveBeenCalledWith({ to: '/login', search: {} })
  })
})

describe('whenSignedOut', () => {
  it('waits for the sign-out running in this tab', async () => {
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    let logoutSent = false
    server.use(
      http.post(
        '/api/auth/logout',
        async () => {
          logoutSent = true
          await gate
          return new HttpResponse(null, { status: 204 })
        },
        { once: true },
      ),
    )
    const done = signOut({ queryClient: new QueryClient(), userId: 'u1', navigate: async () => {} })
    let waited = false
    const waiting = whenSignedOut().then(() => (waited = true))
    // Sign-out is parked on the logout request: whenSignedOut must still be waiting.
    await vi.waitFor(() => expect(logoutSent).toBe(true))
    expect(waited).toBe(false)
    release()
    expect(await done).toBe('signed-out')
    await waiting
    expect(waited).toBe(true)
  })

  it('a failed sign-out does not fail the next sign-in', async () => {
    // A sign-out whose navigation throws: its promise rejects, but a sign-in waiting on it must not.
    const done = signOut({
      queryClient: new QueryClient(),
      userId: 'u1',
      navigate: () => Promise.reject(new Error('nav')),
    })
    const waiting = whenSignedOut()
    await expect(done).rejects.toThrow('nav')
    await expect(waiting).resolves.toBeUndefined()
  })
})
