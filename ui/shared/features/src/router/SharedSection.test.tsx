/**
 * "Shared with you" on the LLM router page: the agents the viewer can use but does not own, each with a select of
 * the viewer's own configs. Picking one PUTs it, Default DELETEs; the select shows the server's choice, and a failed
 * write leaves it as it was.
 */
import { act, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { delay, http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { apiFetch } from '@nasiko/data-access/client'
import { directoryQuery } from '@nasiko/features/agents/api'
import type { Agent } from '@nasiko/features/agents/types'
import { configureMocks, routerMockState } from '@nasiko/mocks/handlers'
import { configId } from '@nasiko/mocks/router'
import { ADMIN_ID } from '@nasiko/mocks/seed-harness'
import { fmtInt } from '@nasiko/ui/lib/format'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequestBodies, recordRequests, server } from '@nasiko/testing/setup'
import { copy } from './copy'
import { SHARED_VISIBLE } from './tuning'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, superuser: null, routerVariants: [] }))

const TITLE = 'Shared with you'
const DEFAULT = 'Default'
const RESEARCH_TIERS = configId(2)
const mineUrl = (id: string) => `/api/agents/${id}/my-llm-config`
const isMine = (u: URL) => u.pathname.endsWith('/my-llm-config')
const firstPage = () => apiFetch<Agent[]>('/api/agents?limit=100')
/** The directory answers two of the viewer's own agents and `n` of others', so the rows' ids are known up front. */
async function shareWith(n: number) {
  const page = await firstPage()
  const own = page.filter((a) => a.owner_id === ADMIN_ID).slice(0, 2)
  const shared = page.filter((a) => a.owner_id !== ADMIN_ID).slice(0, n)
  expect([own.length, shared.length]).toEqual([2, n])
  server.use(
    http.get('/api/agents', ({ request }) =>
      new URL(request.url).searchParams.has('owner')
        ? undefined
        : HttpResponse.json([...own, ...shared]),
    ),
  )
  return shared
}
const sharedList = () => screen.findByRole('list', { name: TITLE })
/** The section once every row has read its choice. */
const ready = async () => {
  const list = await sharedList()
  await waitFor(() => expect(within(list).queryAllByText(copy.readingRouting)).toHaveLength(0))
  return list
}
const rowIds = (list: HTMLElement) =>
  [...list.querySelectorAll('[data-agent]')].map((li) => li.getAttribute('data-agent'))
const rowOf = (list: HTMLElement, id: string) =>
  list.querySelector<HTMLElement>(`[data-agent="${id}"]`)!
const selectOf = (row: HTMLElement) => within(row).getByRole('combobox')
const choose = async (select: HTMLElement, option: string) => {
  await userEvent.click(select)
  await userEvent.click(await screen.findByRole('option', { name: option }))
}

describe('which agents', () => {
  it('lists the agents the viewer does not own below Your agents, and reads only those', async () => {
    const shared = await shareWith(3)
    const calls = recordRequests()
    renderApp('/router')
    const list = await ready()
    expect(rowIds(list).sort()).toEqual(shared.map((a) => a.id).sort())
    const table = screen.getByRole('table', { name: copy.agentsTitle })
    expect(table.compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(
      calls.urls
        .filter(isMine)
        .map((u) => u.pathname)
        .sort(),
    ).toEqual(shared.map((a) => mineUrl(a.id)).sort())
    calls.stop()
  })

  it('shows the first agents only until Show all, so the page reads a bounded number', async () => {
    let inFlight = 0
    let peak = 0
    // Count, then fall through to the mock (a resolver that returns nothing passes the request on).
    server.use(
      http.get('/api/agents/:id/my-llm-config', async () => {
        peak = Math.max(peak, ++inFlight)
        await delay(20)
        inFlight--
      }),
    )
    const calls = recordRequests()
    const { queryClient } = renderApp('/router')
    const list = await ready()
    const others = queryClient
      .getQueryData<Agent[]>(directoryQuery.queryKey)!
      .filter((a) => a.owner_id !== ADMIN_ID)
    expect(others.length).toBeGreaterThan(SHARED_VISIBLE)
    expect(rowIds(list)).toHaveLength(SHARED_VISIBLE)
    expect(calls.urls.filter(isMine)).toHaveLength(SHARED_VISIBLE)
    // Through the router's limiter, as the routing reads are: at most 4 at a time.
    expect(peak).toBeGreaterThan(1)
    expect(peak).toBeLessThanOrEqual(4)
    await userEvent.click(screen.getByRole('button', { name: `Show all ${fmtInt(others.length)}` }))
    expect(rowIds(list)).toHaveLength(others.length)
    calls.stop()
  })

  it('is hidden when the viewer owns everything they can see', async () => {
    const owned = (await firstPage()).filter((a) => a.owner_id === ADMIN_ID)
    server.use(http.get('/api/agents', () => HttpResponse.json(owned)))
    const { queryClient } = renderApp('/router')
    await screen.findByRole('table', { name: copy.agentsTitle })
    await waitFor(() =>
      expect(queryClient.getQueryState(directoryQuery.queryKey)?.status).toBe('success'),
    )
    expect(screen.queryByRole('heading', { name: TITLE })).toBeNull()
  })

  it('says so, with Retry, when the agent list cannot be read', async () => {
    let fail = true
    server.use(
      http.get('/api/agents', ({ request }) =>
        fail && !new URL(request.url).searchParams.has('owner')
          ? new HttpResponse('internal error', { status: 500 })
          : undefined,
      ),
    )
    renderApp('/router')
    const failed = await screen.findByText('Couldn’t list the agents shared with you')
    fail = false
    await userEvent.click(
      within(failed.closest('section')!).getByRole('button', { name: copy.retry }),
    )
    await ready()
  })
})

describe('the choice', () => {
  it('shows the server’s current choice, and Default for an agent with none', async () => {
    const [first, second] = await shareWith(2)
    routerMockState().mine.set(first!.id, RESEARCH_TIERS)
    renderApp('/router')
    const list = await ready()
    expect(selectOf(rowOf(list, first!.id))).toHaveTextContent('research-tiers')
    expect(selectOf(rowOf(list, second!.id))).toHaveTextContent(DEFAULT)
  })

  it('shows Default for a config the viewer no longer has, not its id', async () => {
    const [target] = await shareWith(1)
    server.use(
      http.get(mineUrl(target!.id), () =>
        HttpResponse.json({
          data: { agent_id: target!.id, llm_config_id: configId(99), llm_config: null },
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    renderApp('/router')
    const list = await ready()
    expect(selectOf(rowOf(list, target!.id))).toHaveTextContent(DEFAULT)
  })

  it('picking a config sends a PUT with its id, and only that row is read again', async () => {
    const [target] = await shareWith(2)
    // The re-read after the write is held, to see what the select shows while it is outstanding.
    let gets = 0
    let release = () => {}
    const held = new Promise<void>((r) => (release = r))
    server.use(
      http.get(mineUrl(target!.id), async () => {
        if (++gets === 2) await held
      }),
    )
    const rec = recordRequestBodies()
    renderApp('/router')
    const list = await ready()
    const select = selectOf(rowOf(list, target!.id))
    await choose(select, 'research-tiers')
    await waitFor(() => expect(gets).toBe(2))
    await act(async () => {})
    expect(select).toHaveTextContent('research-tiers')
    expect(select).toBeDisabled()
    release()
    await waitFor(() => expect(select).toBeEnabled())
    expect(select).toHaveTextContent('research-tiers')
    await rec.flush()
    const puts = rec.requests.filter((r) => r.method === 'PUT')
    expect(puts.map((r) => [r.url.pathname, r.body])).toEqual([
      [mineUrl(target!.id), { llm_config_id: RESEARCH_TIERS }],
    ])
    expect(routerMockState().mine.get(target!.id)).toBe(RESEARCH_TIERS)
    const reads = rec.requests.filter((r) => r.method === 'GET' && isMine(r.url))
    expect(reads.filter((r) => r.url.pathname === mineUrl(target!.id))).toHaveLength(2)
    expect(reads).toHaveLength(2 + 1)
    rec.stop()
  })

  it('Default sends a DELETE and the row shows Default', async () => {
    const [target] = await shareWith(1)
    routerMockState().mine.set(target!.id, RESEARCH_TIERS)
    const rec = recordRequestBodies()
    renderApp('/router')
    const list = await ready()
    await choose(selectOf(rowOf(list, target!.id)), DEFAULT)
    await waitFor(() => expect(selectOf(rowOf(list, target!.id))).toHaveTextContent(DEFAULT))
    await rec.flush()
    expect(rec.requests.filter((r) => r.method === 'DELETE').map((r) => r.url.pathname)).toEqual([
      mineUrl(target!.id),
    ])
    expect(routerMockState().mine.has(target!.id)).toBe(false)
    rec.stop()
  })

  it.each([
    { method: 'PUT', stored: undefined, pick: 'research-tiers', shown: DEFAULT },
    { method: 'DELETE', stored: RESEARCH_TIERS, pick: DEFAULT, shown: 'research-tiers' },
  ])('a failed $method shows the error and keeps the previous value', async (c) => {
    const [target] = await shareWith(1)
    if (c.stored) routerMockState().mine.set(target!.id, c.stored)
    server.use(
      http.all(mineUrl(target!.id), ({ request }) =>
        request.method === c.method
          ? new HttpResponse('internal error', { status: 500 })
          : undefined,
      ),
    )
    renderApp('/router')
    const row = rowOf(await ready(), target!.id)
    await choose(selectOf(row), c.pick)
    expect(await within(row).findByRole('alert')).toHaveTextContent(
      copy.errUnmapped('internal error', 500),
    )
    await waitFor(() => expect(selectOf(row)).toBeEnabled())
    expect(selectOf(row)).toHaveTextContent(c.shown)
    expect(routerMockState().mine.get(target!.id)).toBe(c.stored)
  })

  it('a row that cannot be read says so and reads again on Retry', async () => {
    const [target] = await shareWith(1)
    let fail = true
    server.use(
      http.get(mineUrl(target!.id), () =>
        fail ? new HttpResponse('internal error', { status: 500 }) : undefined,
      ),
    )
    renderApp('/router')
    const row = rowOf(await sharedList(), target!.id)
    expect(await within(row).findByText(copy.couldntRead)).toBeInTheDocument()
    expect(within(row).queryByRole('combobox')).toBeNull()
    fail = false
    await userEvent.click(within(row).getByRole('button', { name: copy.retry }))
    expect(await within(row).findByRole('combobox')).toHaveTextContent(DEFAULT)
  })
})
