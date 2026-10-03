/**
 * Recovering a routed turn on the page (v1b §5.4, §5.6): Run again (with and without the DS13
 * confirm), Try again after a failed create, a refused send on the empty state, the bounded
 * history re-checks giving up, and Retry after a failed history refetch. Every test asserts the
 * client never POSTed an assistant row.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  CHAT_SCENARIOS,
  ROUTED_TRACE,
  sseResponse,
  type ChatScenario,
  type MockFrame,
} from '@/mocks/chat'
import { configureChatMock } from '@/mocks/chatStore'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, server } from '@/test/setup'
import { clearDrafts } from './drafts'
import { clearChatRegistry } from './registry'
import { tuning } from './tuning'

let rec: ReturnType<typeof recordRequestBodies>
beforeEach(() => {
  rec = recordRequestBodies()
})
afterEach(async () => {
  await rec.flush()
  try {
    // assistantPosts() throws if a body couldn't be read (fails closed); cleanup runs either way.
    expect(rec.assistantPosts()).toEqual([])
  } finally {
    rec.stop()
    clearChatRegistry()
    clearDrafts()
  }
})

async function sendRouted(text: string, scenario?: ChatScenario) {
  if (scenario) configureChatMock({ scenario })
  const view = renderApp('/chat?auto=1')
  await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), `${text}{Enter}`)
  return view
}

const dispatches = async () => {
  await rec.flush()
  return rec.requests.filter(
    (r) => r.url.pathname === '/api/orchestrator/a2a' && r.method === 'POST',
  ).length
}
const userPosts = async () => {
  await rec.flush()
  return rec.requests.filter(
    (r) => /\/api\/chat\/sessions\/[^/]+\/messages$/.test(r.url.pathname) && r.method === 'POST',
  ).length
}

describe('Run again on a routed chat (§5.4, DS13)', () => {
  it('known-empty: Run again still asks first; Cancel runs nothing, the confirm re-dispatches without re-saving the user row', async () => {
    await sendRouted('nothing', 'routed-empty')
    await screen.findByText('The Orchestrator finished without a reply.')
    await waitFor(async () => expect(await dispatches()).toBe(1))
    await userEvent.click(screen.getByRole('button', { name: 'Run again' }))
    let dialog = await screen.findByRole('alertdialog')
    expect(within(dialog).getByText('Run this message again?')).toBeInTheDocument()
    await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    expect(await dispatches()).toBe(1)
    await userEvent.click(screen.getByRole('button', { name: 'Run again' }))
    dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Run again' }))
    await waitFor(async () => expect(await dispatches()).toBe(2), { timeout: 8000 })
    expect(
      await screen.findByText('The Orchestrator finished without a reply.', {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    expect(await userPosts()).toBe(1)
  })

  it('failed (may have run): the confirm dialog Refresh status re-reads history and runs nothing', async () => {
    await sendRouted('fail', 'routed-failed')
    await screen.findByText('The Orchestrator reported an error.')
    await waitFor(async () => expect(await dispatches()).toBe(1))
    await rec.flush()
    const historyBefore = rec.requests.filter(
      (r) => /\/messages$/.test(r.url.pathname) && r.method === 'GET',
    ).length
    await userEvent.click(
      within(screen.getByTestId('routed-live')).getByRole('button', { name: 'Run again' }),
    )
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Refresh status' }))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    await waitFor(async () => {
      await rec.flush()
      expect(
        rec.requests.filter((r) => /\/messages$/.test(r.url.pathname) && r.method === 'GET').length,
      ).toBeGreaterThan(historyBefore)
    })
    expect(await dispatches()).toBe(1)
  })
})

describe('a routed send that never started (§5.6)', () => {
  it('a failed create gives the text back and Try again creates the chat', async () => {
    let creates = 0
    server.use(
      http.post('/api/chat/sessions', () =>
        ++creates === 1 ? HttpResponse.text('database unavailable', { status: 500 }) : undefined,
      ),
    )
    const { router } = await sendRouted('first try')
    const notice = await screen.findByTestId('error-notice', {}, { timeout: 8000 })
    expect(screen.getByLabelText('Ask the Orchestrator')).toHaveValue('first try')
    await userEvent.click(within(notice).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/), {
      timeout: 8000,
    })
    expect(creates).toBe(2)
    await waitFor(async () => expect(await dispatches()).toBe(1))
  })

  it('a refused send on the empty state shows its error inside it, right under the composer (ISSUE-002)', async () => {
    const cap = tuning.MAX_LIVE_TURNS
    tuning.MAX_LIVE_TURNS = 1
    try {
      server.use(http.post('/api/orchestrator/a2a', () => new Promise<Response>(() => undefined)))
      const { router } = renderApp('/chat?auto=1')
      await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), 'first{Enter}')
      await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
      router.history.push('/chat?auto=1')
      await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), 'second{Enter}')
      const error = await screen.findByTestId('send-error')
      expect(error).toHaveTextContent(/already in progress/)
      // Inside the empty state (with the composer), not after the page body.
      expect(screen.getByTestId('new-chat').contains(error)).toBe(true)
    } finally {
      tuning.MAX_LIVE_TURNS = cap
    }
  })
})

describe('history after a routed turn (§5.4)', () => {
  it('a complete reply the server never saves is re-checked with backoff, then shown as unconfirmed', async () => {
    const [ms, n] = [tuning.ROUTED_RECHECK_MS, tuning.ROUTED_RECHECKS]
    tuning.ROUTED_RECHECK_MS = 5
    tuning.ROUTED_RECHECKS = 2
    try {
      // Bypass the mock's save-at-Done: the stream completes, history never gets the reply.
      const frames: MockFrame[] = CHAT_SCENARIOS['routed-plain'].map((f) =>
        f.data === undefined
          ? f
          : {
              ...f,
              data: JSON.parse(
                JSON.stringify(f.data)
                  .replaceAll(ROUTED_TRACE, '5eedf0000000000000000000000cafe1')
                  .replaceAll('@agent-1@', 'seed-agent'),
              ) as unknown,
            },
      )
      server.use(http.post('/api/orchestrator/a2a', () => sseResponse(frames)))
      await sendRouted('unsaved')
      expect(
        await screen.findByText('Partial reply — completion unconfirmed', {}, { timeout: 8000 }),
      ).toBeInTheDocument()
      await rec.flush()
      const historyGets = rec.requests.filter(
        (r) => /\/messages$/.test(r.url.pathname) && r.method === 'GET',
      ).length
      // The first refetch plus ROUTED_RECHECKS backed-off re-checks.
      expect(historyGets).toBeGreaterThanOrEqual(1 + tuning.ROUTED_RECHECKS)
      // Then it stops: proves an absence (no re-check after the budget), over twice the last backoff.
      await new Promise((r) =>
        setTimeout(r, tuning.ROUTED_RECHECK_MS * 2 ** (tuning.ROUTED_RECHECKS + 2)),
      )
      await rec.flush()
      expect(
        rec.requests.filter((r) => /\/messages$/.test(r.url.pathname) && r.method === 'GET').length,
      ).toBe(historyGets)
    } finally {
      tuning.ROUTED_RECHECK_MS = ms
      tuning.ROUTED_RECHECKS = n
    }
  })

  it('a failed history refetch shows "Couldn\'t load the saved reply" with Retry, and Retry settles it', async () => {
    configureChatMock({ scenario: 'routed-plain' })
    let fail = false
    const { router } = renderApp('/chat?auto=1')
    await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), 'hi{Enter}')
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
    const sid = router.state.location.pathname.split('/').pop()!
    // Break history before the turn ends, so the post-end refetch fails.
    fail = true
    server.use(
      http.get(`/api/chat/sessions/${sid}/messages`, () =>
        fail ? new HttpResponse('internal error', { status: 500 }) : undefined,
      ),
    )
    expect(
      await within(await screen.findByTestId('routed-live')).findByText(
        "Couldn't load the saved reply",
        {},
        { timeout: 8000 },
      ),
    ).toBeInTheDocument()
    fail = false
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(screen.queryByTestId('routed-live')).toBeNull(), { timeout: 8000 })
    expect(screen.getByText(/Answered by the Orchestrator, using /)).toBeInTheDocument()
  })
})
