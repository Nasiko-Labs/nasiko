/**
 * The login page: signing in (the timeout cap, one message for a wrong password or a disabled account, the sign-in
 * generation, a session lock that never comes, stopping turns an expired session left running), the sign-in message
 * other tabs get, the error mapping, and the failed-logout notice with its Try again.
 * (How another tab's messages update this tab's /login: sessionSync.test.ts.)
 */
import { QueryClient } from '@tanstack/react-query'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest'
import { chatRegistry, clearChatRegistry } from '@/features/chat/registry'
import { ApiError, apiFetch } from '@/lib/api/client'
import {
  onSessionMessage,
  SESSION_CHANNEL,
  SIGNED_OUT_KEY,
  SIGNIN_GENERATION_KEY,
  type SessionMessage,
} from '@/lib/session'
import { configureMocks } from '@/mocks/handlers'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'
import { LOGIN_TIMEOUT_MS } from './signOut'
import { setAccent } from './theme'

setupPinnedSeed()

const channels: BroadcastChannel[] = []
const otherTab = () => {
  const c = new BroadcastChannel(SESSION_CHANNEL)
  channels.push(c)
  return c
}
afterEach(() => channels.splice(0).forEach((c) => c.close()))
afterEach(() => vi.restoreAllMocks())
afterEach(() => clearChatRegistry())

/** Resolves once this tab's session listeners have seen the next message (listeners run in order). */
const delivered = () =>
  new Promise<SessionMessage>((resolve) => {
    const off = onSessionMessage((m) => {
      off()
      resolve(m)
    })
  })

const rejectingLocks = () =>
  vi.stubGlobal('navigator', {
    ...navigator,
    locks: { request: () => Promise.reject(new DOMException('timed out', 'TimeoutError')) },
  })

describe('sign-in', () => {
  it(`gives up on a sign-in slower than ${LOGIN_TIMEOUT_MS / 1000} s without starting a session`, async () => {
    configureMocks({ loggedIn: false })
    const user = userEvent.setup()
    const timers: AbortController[] = []
    const real = AbortSignal.timeout.bind(AbortSignal)
    vi.spyOn(AbortSignal, 'timeout').mockImplementation((ms) => {
      if (ms !== LOGIN_TIMEOUT_MS) return real(ms)
      const c = new AbortController()
      timers.push(c)
      return c.signal
    })
    server.use(http.post('/api/auth/login', () => new Promise<never>(() => {}), { once: true }))
    const got: SessionMessage[] = []
    const tab = otherTab()
    tab.onmessage = (e: MessageEvent<SessionMessage>) => got.push(e.data)
    renderApp('/login')
    await user.type(await screen.findByLabelText('Username'), 'admin')
    await user.type(screen.getByLabelText('Password'), 'x')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    await vi.waitFor(() => expect(timers).toHaveLength(1))
    timers[0]!.abort(new DOMException('timed out', 'TimeoutError'))
    // Our own cap, not a stopped server: "slow", not "can't reach".
    expect(await screen.findByRole('alert')).toHaveTextContent('The server took too long to answer')
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeEnabled()
    expect(localStorage.getItem(SIGNIN_GENERATION_KEY)).toBeNull()
    expect(got).toEqual([])
  })

  it('a disabled account gets the same message as a wrong password', async () => {
    configureMocks({ loggedIn: false })
    const user = userEvent.setup()
    server.use(
      http.post('/api/auth/login', () =>
        HttpResponse.json({ error: 'account disabled' }, { status: 401 }),
      ),
    )
    renderApp('/login')
    await user.type(await screen.findByLabelText('Username'), 'riley')
    await user.type(screen.getByLabelText('Password'), 'x')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(await screen.findByText('Wrong username or password.')).toBeTruthy()
  })

  it('bumps the sign-in generation; a failed sign-in does not', async () => {
    server.use(
      http.post(
        '/api/auth/login',
        () => HttpResponse.json({ error: 'invalid credentials' }, { status: 401 }),
        { once: true },
      ),
    )
    renderApp('/login')
    await userEvent.click(await screen.findByRole('button', { name: 'Sign in' }))
    await screen.findByText('Wrong username or password.')
    expect(localStorage.getItem(SIGNIN_GENERATION_KEY)).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    await waitFor(() => expect(localStorage.getItem(SIGNIN_GENERATION_KEY)).not.toBeNull())
  })

  it('a session lock that never comes says Login failed, bumps nothing and tells no tab', async () => {
    const tab = otherTab()
    const got: SessionMessage[] = []
    tab.onmessage = (e: MessageEvent<SessionMessage>) => got.push(e.data)
    rejectingLocks()
    const { router } = renderApp('/login')
    await userEvent.click(await screen.findByRole('button', { name: 'Sign in' }))
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Login failed. Check the server logs.',
    )
    expect(localStorage.getItem(SIGNIN_GENERATION_KEY)).toBeNull()
    expect(router.state.location.pathname).toBe('/login')
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeEnabled()
    // Real wait, kept: BroadcastChannel delivery is not timer-driven, so fake timers can't prove no message came.
    await new Promise((r) => setTimeout(r, 30))
    expect(got).toEqual([])
  })

  it('a sign-in on the login page stops turns left running by an expired session, before sending credentials', async () => {
    configureMocks({ loggedIn: false })
    const user = userEvent.setup()
    const old = chatRegistry(new QueryClient(), 'previous-user')
    const clearAll = vi.spyOn(old, 'clearAll')
    // The old turn must be stopped before login sets the new cookie, or its reply would save under it.
    let stoppedBeforeLogin: boolean | undefined
    const onRequest = ({ request }: { request: Request }) => {
      if (new URL(request.url).pathname === '/api/auth/login')
        stoppedBeforeLogin ??= clearAll.mock.calls.length > 0
    }
    server.events.on('request:start', onRequest)
    // Removed even if an assertion fails, and only this listener (recordRequests() adds its own).
    onTestFinished(() => void server.events.removeListener('request:start', onRequest))
    renderApp('/login')
    await user.type(await screen.findByLabelText('Username'), 'admin')
    await user.type(screen.getByLabelText('Password'), 'x')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    await vi.waitFor(() => expect(clearAll).toHaveBeenCalled())
    await vi.waitFor(() => expect(stoppedBeforeLogin).toBe(true))
  })
})

describe('the sign-in message', () => {
  it('drops an empty sub, so the receiver treats it as an unknown account', async () => {
    const got = delivered()
    otherTab().postMessage({ type: 'signed-in', sub: '' })
    expect(await got).toEqual({ type: 'signed-in' })
  })

  it("carries the login response's user_id", async () => {
    configureMocks({ loggedIn: false })
    const user = userEvent.setup()
    const got: SessionMessage[] = []
    otherTab().onmessage = (e: MessageEvent<SessionMessage>) => got.push(e.data)
    renderApp('/login')
    await user.type(await screen.findByLabelText('Username'), 'admin')
    await user.type(screen.getByLabelText('Password'), 'x')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    await vi.waitFor(() =>
      expect(got).toEqual([{ type: 'signed-in', sub: '5eed0000-0000-4000-8000-00000000a001' }]),
    )
  })

  it('a sign-in tells the other tabs; a failed one says nothing', async () => {
    const tab = otherTab()
    const got: SessionMessage[] = []
    tab.onmessage = (e: MessageEvent<SessionMessage>) => got.push(e.data)
    server.use(
      http.post(
        '/api/auth/login',
        () => HttpResponse.json({ error: 'invalid credentials' }, { status: 401 }),
        { once: true },
      ),
    )
    renderApp('/login')
    await userEvent.click(await screen.findByRole('button', { name: 'Sign in' }))
    await screen.findByText('Wrong username or password.')
    // Real wait, kept: BroadcastChannel delivery is not timer-driven, so fake timers can't prove no message came.
    await new Promise((r) => setTimeout(r, 50))
    expect(got).toEqual([])
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    await vi.waitFor(() => expect(got).toEqual([{ type: 'signed-in', sub: expect.any(String) }]))
  })
})

describe('login error mapping', () => {
  it("apiFetch rethrows a caller's own timeout (TimeoutError) as-is, not as a 502 network error", async () => {
    vi.spyOn(globalThis, 'fetch').mockRejectedValueOnce(
      new DOMException('timed out', 'TimeoutError'),
    )
    const err = await apiFetch('/api/slow').catch((e: unknown) => e)
    expect(err).not.toBeInstanceOf(ApiError)
    expect((err as Error).name).toBe('TimeoutError')
  })

  it('a network failure still says unreachable, not the timeout message', async () => {
    configureMocks({ loggedIn: false })
    const user = userEvent.setup()
    server.use(http.post('/api/auth/login', () => HttpResponse.error(), { once: true }))
    renderApp('/login')
    await user.type(await screen.findByLabelText('Username'), 'admin')
    await user.type(screen.getByLabelText('Password'), 'x')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(await screen.findByRole('alert')).toHaveTextContent("Can't reach nasiko-server")
    expect(screen.getByRole('alert')).not.toHaveTextContent('took too long')
  })

  it('an unreachable server and an unexpected failure get distinct messages', async () => {
    configureMocks({ loggedIn: false })
    const user = userEvent.setup()
    server.use(http.post('/api/auth/login', () => new HttpResponse('bad gateway', { status: 502 })))
    renderApp('/login')
    await user.click(await screen.findByRole('button', { name: 'Sign in' }))
    expect(await screen.findByRole('alert')).toHaveTextContent("Can't reach nasiko-server")
    server.use(http.post('/api/auth/login', () => new HttpResponse('boom', { status: 500 })))
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(await screen.findByText('Login failed. Check the server logs.')).toBeInTheDocument()
  })
})

describe('the failed-logout notice', () => {
  it('still shows when storage is blocked (the barrier cannot be read)', async () => {
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
    })
    renderApp('/login?signout=failed')
    expect(await screen.findByRole('button', { name: 'Try again' })).toBeInTheDocument()
  })

  it('Try again gives the form back and says it failed when the lock never comes', async () => {
    localStorage.setItem(SIGNED_OUT_KEY, '1')
    rejectingLocks()
    renderApp('/login?signout=failed')
    await userEvent.click(await screen.findByRole('button', { name: 'Try again' }))
    expect(await screen.findByText('Still no answer from the server.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled()
  })

  describe('against a newer sign-in', () => {
    /** In-memory Storage the tests can inspect directly. */
    function memoryStorage(): Storage & { map: Map<string, string> } {
      const map = new Map<string, string>()
      return {
        map,
        get length() {
          return map.size
        },
        clear: () => map.clear(),
        getItem: (k) => map.get(k) ?? null,
        key: (i) => [...map.keys()][i] ?? null,
        removeItem: (k) => void map.delete(k),
        setItem: (k, v) => void map.set(k, String(v)),
      }
    }

    let store: ReturnType<typeof memoryStorage>
    beforeEach(() => {
      store = memoryStorage()
      vi.stubGlobal('localStorage', store)
    })

    it('a stale ?signout=failed after a newer sign-in shows no notice', async () => {
      // No barrier stored: someone signed in since (the barrier would be '1' otherwise).
      renderApp('/login?signout=failed')
      await screen.findByRole('heading', { name: 'Sign in to Nasiko' })
      expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull()
    })

    it('Try again skips the logout if a sign-in cleared the barrier meanwhile', async () => {
      store.setItem(SIGNED_OUT_KEY, '1')
      const reqs = recordRequests()
      try {
        const { router } = renderApp('/login?signout=failed')
        const retry = await screen.findByRole('button', { name: 'Try again' })
        store.removeItem(SIGNED_OUT_KEY) // another tab signed in
        await userEvent.click(retry)
        await waitFor(() => expect(router.state.location.search).not.toHaveProperty('signout'))
        expect(reqs.urls.filter((u) => u.pathname === '/api/auth/logout')).toHaveLength(0)
      } finally {
        reqs.stop()
      }
    })

    it('a successful Try again clears the barrier and tells the other tabs', async () => {
      store.setItem(SIGNED_OUT_KEY, '1')
      const tab = otherTab()
      const got: SessionMessage[] = []
      tab.onmessage = (e: MessageEvent<SessionMessage>) => got.push(e.data)
      renderApp('/login?signout=failed')
      await userEvent.click(await screen.findByRole('button', { name: 'Try again' }))
      await vi.waitFor(() =>
        expect(got).toEqual([
          { type: 'signed-out', failed: false, generation: expect.any(String) },
        ]),
      )
      expect(store.map.has(SIGNED_OUT_KEY)).toBe(false)
    })
  })
})

describe('layout', () => {
  it('keeps the decorative showcase out of the accessibility tree and off narrow screens', async () => {
    configureMocks({ loggedIn: false })
    renderApp('/login')
    await screen.findByRole('heading', { name: 'Sign in to Nasiko' })
    const panel = screen.getByTestId('login-showcase')
    expect(panel).toHaveAttribute('aria-hidden', 'true')
    expect(panel).toHaveClass('hidden', 'md:flex')
    // The showcase leads the DOM (left column), the form follows; the form's lockup is the one named logo (the mark on
    // the showcase's top plate is decorative).
    const form = screen.getByLabelText('Username').closest('form')!
    expect(panel.compareDocumentPosition(form) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(panel.querySelector('svg[role="img"]')).toBeNull()
    expect(panel.querySelector('svg')).not.toBeNull()
    expect(screen.getByRole('img', { name: 'Nasiko' })).toBeInTheDocument()
  })

  it('offers no password reset or remember-me the server cannot honour', async () => {
    configureMocks({ loggedIn: false })
    renderApp('/login')
    await screen.findByRole('heading', { name: 'Sign in to Nasiko' })
    expect(screen.queryByText(/forgot password/i)).toBeNull()
    expect(screen.queryByText(/keep me signed in/i)).toBeNull()
  })

  it('shows and hides the password', async () => {
    configureMocks({ loggedIn: false })
    const user = userEvent.setup()
    renderApp('/login')
    const field = await screen.findByLabelText('Password')
    const toggle = screen.getByRole('button', { name: 'Show password' })
    expect(field).toHaveAttribute('type', 'password')
    await user.click(toggle)
    expect(field).toHaveAttribute('type', 'text')
    expect(toggle).toHaveAttribute('aria-pressed', 'true')
    await user.click(toggle)
    expect(field).toHaveAttribute('type', 'password')
  })

  it('has no mode switch: the page follows the stored choice or the system', async () => {
    configureMocks({ loggedIn: false })
    renderApp('/login')
    await screen.findByRole('heading', { name: 'Sign in to Nasiko' })
    expect(screen.queryByRole('radiogroup', { name: 'Mode' })).toBeNull()
  })

  it('lands on the Overview when no redirect is given', async () => {
    configureMocks({ loggedIn: false })
    const user = userEvent.setup()
    const { router } = renderApp('/login')
    await user.type(await screen.findByLabelText('Username'), 'admin')
    await user.type(screen.getByLabelText('Password'), 'x')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/'))
  })

  it('shows Carbon on the sign-in screen and the stored theme again after signing in', async () => {
    configureMocks({ loggedIn: false })
    setAccent('plum')
    const user = userEvent.setup()
    const { router } = renderApp('/login')
    await user.type(await screen.findByLabelText('Username'), 'admin')
    await user.type(screen.getByLabelText('Password'), 'x')
    expect(document.documentElement).toHaveAttribute('data-theme', 'carbon')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    await waitFor(() => expect(router.state.location.pathname).not.toBe('/login'))
    await waitFor(() => expect(document.documentElement).toHaveAttribute('data-theme', 'plum'))
  })
})
