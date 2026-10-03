/**
 * Truth-check and polling wiring (plan §10: "truth-check timing", "polling start/stop"), with
 * fake timers: the restart outcome waits for Running past the grace period; Your agents polls
 * only while a row is Deploying or watched, and keeps checking a row that left the visible tab;
 * failed polls end a watch at the cap instead of leaving "Restarting…" up forever.
 */
import { act, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ADMIN_ID } from '@/mocks/seed-harness'
import { FIXED, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'
import { copy } from './copy'
import { GRACE_MS, POLL_MS, WATCH_CAP_MS } from './tuning'

setupPinnedSeed()

beforeEach(() => {
  vi.useFakeTimers({
    toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date'],
    now: FIXED,
    shouldAdvanceTime: true,
  })
})
afterEach(() => {
  // Back to the pinned-seed default (Date only).
  vi.useFakeTimers({ toFake: ['Date'], now: FIXED })
})

const user = () => userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms))
const running = seed.agents[0]!

function ownedRow(status: string, id = '0000beef-0000-4000-8000-000000000a01') {
  return {
    id,
    name: 'flow-bot',
    display_name: 'Flow Bot',
    description: '',
    status,
    owner_id: ADMIN_ID,
    version: '1.0.0',
    tags: [],
    skills: [],
    metadata: null,
    created_at: '2026-03-01T00:00:00Z',
    updated_at: new Date(Date.now()).toISOString(),
  }
}

describe('restart truth check (detail)', () => {
  it('says Restarted only after Running holds past the grace period', async () => {
    renderApp(`/agents/${running.id}`)
    await user().click(await screen.findByRole('button', { name: 'Restart' }))
    expect(await screen.findByText(copy.restarting)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Restart' })).toBeDisabled()
    // Polls at 5 s (inside the grace period, ignored) and 10 s (first Running reading).
    await advance(GRACE_MS)
    expect(screen.queryByText(copy.restarted)).toBeNull()
    await advance(POLL_MS)
    expect(await screen.findByText(copy.restarted)).toBeInTheDocument()
  })

  it('failed status polls end the watch at the cap ("Still starting")', async () => {
    renderApp(`/agents/${running.id}`)
    await user().click(await screen.findByRole('button', { name: 'Restart' }))
    await screen.findByText(copy.restarting)
    server.use(http.get('/api/agents', () => new HttpResponse('boom', { status: 500 })))
    await advance(WATCH_CAP_MS + POLL_MS)
    expect(await screen.findByText(copy.stillStarting)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Restart' })).toBeEnabled()
  })
})

describe('Your agents polling', () => {
  it('polls while a row is Deploying and stops once it is not', async () => {
    let n = 0
    server.use(
      http.get('/api/agents', ({ request }) => {
        const u = new URL(request.url)
        if (u.searchParams.get('owner') !== ADMIN_ID || u.searchParams.get('offset') !== '0')
          return HttpResponse.json([])
        n++
        return HttpResponse.json([ownedRow(n === 1 ? 'deploying' : 'running')])
      }),
    )
    renderApp('/agents/mine')
    await screen.findByText('Flow Bot')
    expect(n).toBe(1)
    await advance(POLL_MS)
    await waitFor(() => expect(n).toBe(2))
    await advance(3 * POLL_MS)
    expect(n).toBe(2)
  })

  it('a row stuck Deploying past the cap does not keep the page polling', async () => {
    let n = 0
    server.use(
      http.get('/api/agents', ({ request }) => {
        const u = new URL(request.url)
        if (u.searchParams.get('owner') !== ADMIN_ID || u.searchParams.get('offset') !== '0')
          return HttpResponse.json([])
        n++
        return HttpResponse.json([
          {
            ...ownedRow('deploying'),
            updated_at: new Date(Date.now() - WATCH_CAP_MS - 1).toISOString(),
          },
        ])
      }),
    )
    renderApp('/agents/mine')
    await screen.findByText('Flow Bot')
    await advance(3 * POLL_MS)
    expect(n).toBe(1)
  })

  it('a restarted row that leaves the visible tab is still checked, and polling then stops', async () => {
    const crashed = seed.agents[2]!
    const reqs = recordRequests()
    try {
      const polls = () =>
        reqs.urls.filter(
          (u) =>
            u.pathname === '/api/agents' &&
            u.searchParams.get('owner') === ADMIN_ID &&
            u.searchParams.get('offset') === '0',
        ).length
      renderApp('/agents/mine?tab=attention')
      const rows = await screen.findAllByRole('button', { name: 'Restart' })
      // The pinned section lists the crashed agents first; restart the EE-crashed one.
      const row = rows.find((b) => b.closest('li')?.textContent?.includes(crashed.display_name))!
      await user().click(row)
      // The mock writes `running` at once: the row leaves the Needs attention tab on the next render.
      await advance(GRACE_MS + 2 * POLL_MS)
      const settled = polls()
      // It kept polling while the watch ran, although the row was no longer on screen...
      expect(settled).toBeGreaterThan(2)
      await advance(4 * POLL_MS)
      // The watch finished (two Running readings past the grace period): no more polling.
      expect(polls()).toBe(settled)
    } finally {
      reqs.stop()
    }
  })
})
