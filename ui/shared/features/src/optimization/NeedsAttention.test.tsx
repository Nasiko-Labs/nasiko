/**
 * Needs attention on /optimization (plans/feat-optimization-page.md T3: P5, R2C, R2F, R6x; eng E2, C7).
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { delay, http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, mockCtx } from '@nasiko/mocks/handlers'
import { ADMIN_ID } from '@nasiko/mocks/seed-harness'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 12_000 }
const live = () => mockCtx.agents().agents.filter((a) => !a.deleted)
const offMine = () => live().filter((a) => a.owner_id === ADMIN_ID && !a.compress)
const offOthers = () => live().filter((a) => a.owner_id !== ADMIN_ID && !a.compress)
const strip = () => screen.findByRole('region', { name: copy.attention.title }, T)

describe('Needs attention', () => {
  it('a superuser sees two lines: your agents (your chats) and other owners’ (theirs) (E2)', async () => {
    const mine = offMine().length
    const others = offOthers().length
    expect(mine).toBeGreaterThan(1)
    expect(others).toBeGreaterThan(1)
    renderApp('/optimization')
    const s = await strip()
    expect(
      within(s).getByText(
        new RegExp(
          `^${mine} of your agents have Token optimization off, so their chats aren’t optimized\\.`,
        ),
      ),
    ).toBeInTheDocument()
    expect(
      within(s).getByText(new RegExp(`^${others} other agents in the workspace have it off`)),
    ).toBeInTheDocument()
    expect(
      within(s).getByRole('button', { name: copy.attention.turnOnMine(mine) }),
    ).toBeInTheDocument()
    expect(
      within(s).getByRole('button', { name: copy.attention.turnOnAll(others) }),
    ).toBeInTheDocument()
    // Each agent is named with a link to it.
    const first = offMine().sort((a, b) => a.display_name.localeCompare(b.display_name))[0]!
    expect(within(s).getByRole('link', { name: first.display_name })).toHaveAttribute(
      'href',
      `/agents/${first.id}`,
    )
  })

  it('a member gets only the line about their own agents', async () => {
    configureMocks({ superuser: false })
    renderApp('/optimization')
    const s = await strip()
    expect(within(s).getByText(/of your agents have Token optimization off/)).toBeInTheDocument()
    expect(within(s).queryByText(/other agents? in the workspace/)).toBeNull()
  })

  it('turns your agents on after a confirm that names them and the side effects, 4 at a time, then drops the line (R2F, C7)', async () => {
    const ids = new Set(offMine().map((a) => a.id))
    let inflight = 0
    let peak = 0
    server.use(
      http.put('/api/agents/:id', async ({ params }) => {
        if (!ids.has(String(params.id))) return
        inflight++
        peak = Math.max(peak, inflight)
        await delay(20)
        inflight--
        // Falls through to the agents mock, which writes the flag.
      }),
    )
    renderApp('/optimization')
    const s = await strip()
    await userEvent.click(
      within(s).getByRole('button', { name: copy.attention.turnOnMine(ids.size) }),
    )
    const dialog = await screen.findByRole('alertdialog')
    expect(within(dialog).getByText(copy.attention.confirmMine)).toBeInTheDocument()
    for (const a of offMine()) expect(within(dialog).getByText(a.display_name)).toBeInTheDocument()
    await userEvent.click(
      within(dialog).getByRole('button', { name: copy.attention.confirm(ids.size) }),
    )
    await waitFor(
      () => expect(screen.queryByText(/of your agents have Token optimization off/)).toBeNull(),
      T,
    )
    expect(peak).toBeLessThanOrEqual(4)
    expect(peak).toBeGreaterThan(1)
    expect(offMine()).toHaveLength(0)
    expect(await screen.findByText(copy.attention.announce(ids.size, 0), {}, T)).toBeInTheDocument()
  })

  it('a failure stays with the server’s reason and Retry; the rest drop off (R2F)', async () => {
    const failing = offOthers()[0]!
    let fail = true
    server.use(
      http.put('/api/agents/:id', ({ params }) => {
        if (params.id === failing.id && fail)
          return new HttpResponse('database busy', { status: 500 })
      }),
    )
    const total = offOthers().length
    renderApp('/optimization')
    const s = await strip()
    await userEvent.click(within(s).getByRole('button', { name: copy.attention.turnOnAll(total) }))
    const dialog = await screen.findByRole('alertdialog')
    expect(within(dialog).getByText(copy.attention.confirmOthers)).toBeInTheDocument()
    expect(within(dialog).getAllByText(/· owner /)).toHaveLength(total)
    await userEvent.click(
      within(dialog).getByRole('button', { name: copy.attention.confirm(total) }),
    )
    expect(
      await screen.findByText(copy.attention.failed(failing.display_name, 'database busy'), {}, T),
    ).toBeInTheDocument()
    expect(
      await screen.findByText(copy.attention.announce(total - 1, 1), {}, T),
    ).toBeInTheDocument()
    expect(offOthers().map((a) => a.id)).toEqual([failing.id])
    fail = false
    await userEvent.click(screen.getByRole('button', { name: copy.attention.retryFailed }))
    await waitFor(() => expect(screen.queryByText(/other agents? in the workspace/)).toBeNull(), T)
    expect(offOthers()).toHaveLength(0)
  })

  it('a failed agents list says so with Retry, never an all-clear (R2C)', async () => {
    server.use(
      http.get('/api/agents', ({ request }) =>
        new URL(request.url).searchParams.get('owner')
          ? new HttpResponse('internal error', { status: 500 })
          : undefined,
      ),
    )
    renderApp('/optimization')
    const s = await strip()
    expect(within(s).getByText(copy.attention.failedCheck)).toBeInTheDocument()
    expect(within(s).getByRole('button', { name: copy.retry })).toBeInTheDocument()
  })

  it('is absent when every agent has it on', async () => {
    for (const a of mockCtx.agents().agents) a.compress = true
    const { queryClient } = renderApp('/optimization')
    await screen.findByRole('heading', { level: 2, name: /^Saving/ }, T)
    // Only once both agent lists have loaded is "no strip" an answer, not "not yet" (review: testing).
    await waitFor(() => {
      expect(queryClient.getQueryState(['agents', 'owned', ADMIN_ID])?.status).toBe('success')
      expect(queryClient.getQueryState(['agents'])?.status).toBe('success')
    }, T)
    expect(screen.queryByRole('region', { name: copy.attention.title })).toBeNull()
  })

  it('a 401 during the bulk turn-on takes the expiry path, not a per-agent failure (review: testing)', async () => {
    const target = offMine()[0]!
    server.use(
      http.put('/api/agents/:id', ({ params }) =>
        params.id === target.id
          ? HttpResponse.json(
              { data: null, status_code: 401, message: 'missing or invalid token' },
              { status: 401 },
            )
          : undefined,
      ),
    )
    const { router } = renderApp('/optimization')
    const s = await strip()
    const n = offMine().length
    await userEvent.click(within(s).getByRole('button', { name: copy.attention.turnOnMine(n) }))
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: copy.attention.confirm(n) }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'), T)
    expect(router.state.location.search).toMatchObject({ expired: true })
  })

  it('a superuser whose fleet list fails gets the failed check with Retry, which recovers (R2C)', async () => {
    let fail = true
    server.use(
      http.get('/api/agents', ({ request }) =>
        fail && !new URL(request.url).searchParams.get('owner')
          ? new HttpResponse('internal error', { status: 500 })
          : undefined,
      ),
    )
    renderApp('/optimization')
    const s = await strip()
    expect(
      within(s).getByRole('heading', { level: 2, name: copy.attention.title }),
    ).toBeInTheDocument()
    expect(within(s).getByText(copy.attention.failedCheck)).toBeInTheDocument()
    fail = false
    await userEvent.click(within(s).getByRole('button', { name: copy.retry }))
    expect(await screen.findByText(/other agents? in the workspace/, {}, T)).toBeInTheDocument()
  })
  it('a sign-out mid-batch stops the writes not yet started (review: Codex P1)', async () => {
    // More than the 4 writes in flight, so some are queued.
    for (const a of live()) if (a.owner_id === ADMIN_ID && !a.harness) a.compress = false
    const ids = new Set(offMine().map((a) => a.id))
    expect(ids.size).toBeGreaterThan(4)
    let writes = 0
    server.use(
      http.put('/api/agents/:id', async ({ params }) => {
        if (!ids.has(String(params.id))) return
        writes++
        await delay(60)
      }),
    )
    const { queryClient } = renderApp('/optimization')
    const s = await strip()
    await userEvent.click(
      within(s).getByRole('button', { name: copy.attention.turnOnMine(ids.size) }),
    )
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(
      within(dialog).getByRole('button', { name: copy.attention.confirm(ids.size) }),
    )
    await waitFor(() => expect(writes).toBeGreaterThan(0))
    // Sign-out clears the cache: the queued writes must not run with whatever session comes next.
    queryClient.clear()
    await delay(300)
    expect(writes).toBeLessThanOrEqual(4)
  })
})
