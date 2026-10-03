/**
 * Error rules against the live server's recorded responses (plans/feat-live-contract.md §7.3). Each fixture is served
 * as recorded and fetched through the app's own `apiFetch`, so body parsing is the real one too.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { http, HttpResponse } from 'msw'
import { describe, expect, it, vi } from 'vitest'
import { signOut } from '@/app/shell/signOut'
import { ApiError, apiFetch } from '@/lib/api/client'
import {
  isBudgetsAbsent,
  isDeadSession401,
  isEndpointAbsent,
  isTopTracesAbsent,
  isUnauthorized,
} from '@/lib/api/detect'
import { createQueryClient } from '@/lib/queryClient'
import { server } from '@/test/setup'

interface Fx {
  id: string
  status: number
  content_type: string
  body: unknown
  request: { path: string }
}
const LIVE = join(__dirname, '__live__', 'oss')
const load = (rel: string) => JSON.parse(readFileSync(join(LIVE, rel), 'utf8')) as Fx

/** The error `apiFetch` throws when the server answers with this fixture. */
async function errorFor(fx: Fx): Promise<ApiError> {
  const text =
    typeof fx.body === 'string' ? fx.body : fx.body === null ? '' : JSON.stringify(fx.body)
  server.use(
    http.all(
      fx.request.path,
      () =>
        new HttpResponse(text || null, {
          status: fx.status,
          headers: fx.content_type ? { 'Content-Type': fx.content_type } : {},
        }),
      { once: true },
    ),
  )
  const err = await apiFetch(fx.request.path).then(
    () => null,
    (e: unknown) => e,
  )
  if (!(err instanceof ApiError))
    throw new Error(`${fx.id}: expected an ApiError, got ${String(err)}`)
  return err
}

const unknownRoute = load('errors/errors.unknown-route.json')
const topTraces = load('tokenops/tokenops.top-traces.json')
const budgets = [
  'router/router.budgets.json',
  'router/router.budgets-status.json',
  'router/router.budgets-alerts.json',
].map(load)
const noAuth = load('errors/errors.no-auth.json')
const unknownAgent = load('errors/errors.unknown-agent.json')
const requiresAdmin = load('errors/errors.requires-admin.json')

describe('missing endpoints (the "needs a newer server" states)', () => {
  it("recognises the server's unknown-route 404", async () => {
    expect(unknownRoute.status).toBe(404)
    expect(isEndpointAbsent(await errorFor(unknownRoute))).toBe(true)
  })
  it('TokenOps F4: /finops/top-traces as the pinned server answers it', async () => {
    expect(isTopTracesAbsent(await errorFor(topTraces))).toBe(true)
  })
  it.each(budgets.map((f) => [f.id, f] as const))(
    'Router R2: %s as the pinned server answers it',
    async (_, fx) => {
      expect(isBudgetsAbsent(await errorFor(fx))).toBe(true)
    },
  )
  it('Harnesses: an absent usage endpoint answers like any unknown route', async () => {
    expect(
      isEndpointAbsent(
        await errorFor({
          ...unknownRoute,
          request: { path: '/api/observability/coding-agents/usage' },
        }),
      ),
    ).toBe(true)
  })
  it("a handler's own 404 or other errors never read as a missing endpoint", async () => {
    expect(
      isEndpointAbsent(await errorFor({ ...unknownAgent, status: 404, body: 'agent not found' })),
    ).toBe(false)
    expect(
      isEndpointAbsent(await errorFor({ ...unknownAgent, status: 404, body: 'budget not found' })),
    ).toBe(false)
    expect(
      isEndpointAbsent(await errorFor({ ...unknownAgent, status: 404, body: 'Not Found' })),
    ).toBe(true)
    expect(
      isEndpointAbsent(
        await errorFor({
          ...unknownRoute,
          body: { error: 'unit not found or not visible', code: 'unit_not_visible' },
        }),
      ),
    ).toBe(false)
    for (const fx of [unknownAgent, noAuth, requiresAdmin])
      expect(isEndpointAbsent(await errorFor(fx)), fx.id).toBe(false)
  })
})

describe('401s', () => {
  it("the server's no-session 401 is a dead session and an expiry", async () => {
    const err = await errorFor(noAuth)
    expect(err.serverMessage).toBe('missing or invalid token')
    expect(isDeadSession401(err)).toBe(true)
    expect(isUnauthorized(err)).toBe(true)
  })
  it('a fail-closed 401 in the same envelope is an expiry but not a dead session', async () => {
    const err = await errorFor({
      ...noAuth,
      body: { ...(noAuth.body as object), message: 'token validation unavailable' },
    })
    expect(isUnauthorized(err)).toBe(true)
    expect(isDeadSession401(err)).toBe(false)
  })
  it('the shared query handler sends the recorded 401 to /login?expired=true', async () => {
    const navigate = vi.fn(() => Promise.resolve())
    const router = {
      state: { location: { pathname: '/tokenops', href: '/tokenops?open=all' } },
      navigate,
    }
    const qc = createQueryClient(() => router as never, { retry: false })
    server.use(
      http.get(
        '/api/finops-probe',
        () => HttpResponse.json(noAuth.body as Record<string, unknown>, { status: 401 }),
        { once: true },
      ),
    )
    await qc
      .fetchQuery({ queryKey: ['probe'], queryFn: () => apiFetch('/api/finops-probe') })
      .catch(() => {})
    expect(navigate).toHaveBeenCalledWith({
      to: '/login',
      search: { redirect: '/tokenops?open=all', expired: true },
    })
  })
  it('sign out treats the recorded no-session 401 from logout as already signed out', async () => {
    server.use(
      http.post(
        '/api/auth/logout',
        () => HttpResponse.json(noAuth.body as Record<string, unknown>, { status: 401 }),
        { once: true },
      ),
    )
    const navigate = vi.fn(() => Promise.resolve())
    expect(
      await signOut({
        queryClient: createQueryClient(() => undefined, { retry: false }),
        userId: undefined,
        navigate,
      }),
    ).toBe('signed-out')
  })
})

describe('other recorded errors keep the shapes the UI reads', () => {
  it("unknown agent is a plain-text 400 the page shows as the server's reason", async () => {
    const err = await errorFor(unknownAgent)
    expect(err.status).toBe(400)
    expect(err.serverMessage).toMatch(/^agent '.+' not found$/)
  })
  it('requires admin is a plain-text 403 (Harnesses falls back to /api/me)', async () => {
    const err = await errorFor(requiresAdmin)
    expect([err.status, err.serverMessage]).toEqual([403, 'requires admin role'])
  })
})
