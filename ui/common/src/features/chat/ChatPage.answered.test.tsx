/**
 * Routed requests on the page (v1b §5.4; ship review D1, D3): a request names the agent that asked,
 * a chained pause too, and ends in one server-saved reply; an answered request never offers Run
 * again and holds the composer only while its reply can still arrive; a paused chat shows history's
 * request, starts new chats routed, and lets go of a request handled elsewhere. Every test asserts
 * the client never POSTed an assistant row.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ChatScenario } from '@/mocks/chat'
import {
  chatMockMessages,
  chatMockReconnects,
  chatMockRequests,
  configureChatMock,
} from '@/mocks/chatStore'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, server } from '@/test/setup'
import { copy, errorCopy } from './copy'
import { clearDrafts } from './drafts'
import { chatRegistry, clearChatRegistry } from './registry'
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
    vi.useRealTimers()
    clearChatRegistry()
    clearDrafts()
  }
})

type User = ReturnType<typeof userEvent.setup>

async function sendRouted(
  text: string,
  scenario: ChatScenario,
  user: Pick<User, 'type'> = userEvent,
) {
  configureChatMock({ scenario })
  const view = renderApp('/chat?auto=1')
  await user.type(await screen.findByLabelText('Ask the Orchestrator'), `${text}{Enter}`)
  return view
}

const settled = () =>
  waitFor(() => expect(screen.queryByTestId('routed-live')).toBeNull(), { timeout: 8000 })
const sessionIdOf = (router: { state: { location: { pathname: string } } }) =>
  router.state.location.pathname.split('/').pop()!
const resolves = async () => {
  await rec.flush()
  return rec.requests.filter(
    (r) => r.method === 'POST' && /\/api\/hitl\/[^/]+\/resolve$/.test(r.url.pathname),
  ).length
}

/**
 * Click the option `label` until the `n`th resolve has been sent (the stream's card is replaced
 * by history's once the request is saved, so a click can land on the one being replaced).
 */
async function answer(label: string, n = 1, user: Pick<User, 'click'> = userEvent) {
  await screen.findByRole('button', { name: label }, { timeout: 8000 })
  await waitFor(
    async () => {
      if ((await resolves()) >= n) return
      const btn = screen
        .queryAllByRole('button', { name: label })
        .find((b) => !b.hasAttribute('disabled'))
      if (btn) await user.click(btn)
      throw new Error('no resolve yet')
    },
    { timeout: 8000, interval: 150 },
  )
}

/** Every "Requested by …" line the page ever rendered, so a transient card can't hide a wrong name. */
function watchRequestedBy() {
  const seen = new Set<string>()
  const spoken = new Set<string>()
  /** `question :: Requested by …`, per card rendered. */
  const cards = new Set<string>()
  const scan = () => {
    for (const el of document.querySelectorAll('[role="status"]'))
      if (el.textContent) spoken.add(el.textContent.trim())
    for (const card of document.querySelectorAll('[data-testid="request-card"]')) {
      for (const el of card.querySelectorAll('div')) {
        const t = el.textContent ?? ''
        if (!/^Requested by /.test(t) || el.children.length > 1) continue
        seen.add(t.replace(/ · .*$/, ''))
        cards.add(`${card.querySelector('p')?.textContent ?? ''} :: ${t.replace(/ · .*$/, '')}`)
      }
    }
  }
  const obs = new MutationObserver(scan)
  obs.observe(document.body, { childList: true, subtree: true, characterData: true })
  return { seen, spoken, cards, stop: () => obs.disconnect(), scan }
}

describe('who asked (ISSUE-001, ship review D1)', () => {
  it('a routed request names the agent that asked, not OpenRuntime', async () => {
    configureChatMock({ scenario: 'routed-hitl' })
    renderApp('/chat?auto=1')
    await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), 'deploy it{Enter}')
    const card = await screen.findByTestId('request-card', {}, { timeout: 8000 })
    expect(card).not.toHaveTextContent(/Requested by (OpenRuntime|the Orchestrator)/)
    const requestedBy = within(card).getByText(/^Requested by/).parentElement!
    // The link goes to the asking agent's page, and its label names that agent.
    const link = within(requestedBy).getByRole('link')
    expect(link.getAttribute('href')).toMatch(/^\/agents\/5eed/)
    expect(link.textContent).not.toMatch(/^(OpenRuntime|Orchestrator|the Orchestrator)$/)
  })

  it('a chained pause: the second request names the sub-agent, the announcement too; answering it saves exactly one server reply', async () => {
    const watch = watchRequestedBy()
    try {
      const { router } = await sendRouted('deploy it', 'routed-hitl-chained')
      await answer('us-east-1', 1)
      // The resumed sub-agent asks again: a second card, for the same agent.
      await screen.findByRole('button', { name: 'now' }, { timeout: 8000 })
      const card = screen
        .getByRole('button', { name: 'now' })
        .closest('[data-testid="request-card"]') as HTMLElement
      expect(card).toHaveTextContent('Deploy to us-east-1 now or at the next window?')
      expect(card).not.toHaveTextContent(/Requested by (OpenRuntime|the Orchestrator)/)
      const link = within(within(card).getByText(/^Requested by/).parentElement!).getByRole('link')
      expect(link.getAttribute('href')).toMatch(/^\/agents\/5eed/)
      expect(link.textContent).not.toBe(copy.orchestratorName)
      // The chained pause's announcement names the agent of the resumed request, not OpenRuntime.
      await waitFor(() =>
        expect(
          screen
            .getAllByRole('status')
            .map((s) => s.textContent ?? '')
            .join('|'),
        ).toMatch(/Approval requested by /),
      )
      expect(
        screen
          .getAllByRole('status')
          .map((s) => s.textContent ?? '')
          .join('|'),
      ).not.toMatch(/Approval requested by (OpenRuntime|the Orchestrator)/)
      // Nothing saved yet: the chained continuation has no orchestrator turn.
      const sid = sessionIdOf(router)
      expect(chatMockMessages(sid).filter((m) => m.role === 'assistant')).toHaveLength(0)

      await answer('now', 2)
      expect(
        await screen.findByText('Done: deployed to now.', {}, { timeout: 8000 }),
      ).toBeInTheDocument()
      await settled()
      // Wait for the saved rendering: the saved reply has its own Copy button.
      await waitFor(() =>
        expect(
          screen
            .getByText('Done: deployed to now.')
            .closest('[data-testid="turn"]')!
            .querySelector('button'),
        ).not.toBeNull(),
      )
      expect(chatMockMessages(sid).filter((m) => m.role === 'assistant')).toHaveLength(1)
      expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()
      watch.scan()
      expect([...watch.seen].length).toBeGreaterThan(0)
      expect([...watch.seen]).not.toContain(`Requested by ${copy.orchestratorName}`)
      // Both pauses were announced, and neither named OpenRuntime as the asker.
      expect(
        [...watch.spoken].filter((t) => /Approval requested by/.test(t)).length,
      ).toBeGreaterThan(0)
      expect([...watch.spoken].join('|')).not.toMatch(
        /Approval requested by (OpenRuntime|the Orchestrator)/,
      )
    } finally {
      watch.stop()
    }
  })

  it("a chained pause before history has the new request: the card built from the resumed stream's hitl frame (no agent) names the resumed request's agent", async () => {
    // History never lists the chained request, so the only card for it is the stream frame's.
    server.use(
      http.get('/api/chat/sessions/:id/messages', async ({ request }) => {
        if (request.headers.get('x-chained-inner')) return undefined
        const res = await fetch(request.url, { headers: { 'x-chained-inner': '1' } })
        const body = (await res.json()) as { hitl?: { question?: { message?: string } | null }[] }
        return HttpResponse.json(
          {
            ...body,
            hitl: (body.hitl ?? []).filter((h) => !/^Deploy to /.test(h.question?.message ?? '')),
          },
          { status: res.status },
        )
      }),
    )
    const watch = watchRequestedBy()
    try {
      await sendRouted('deploy it', 'routed-hitl-chained')
      await answer('us-east-1', 1)
      await waitFor(
        () =>
          expect(
            [...watch.cards].some((c) =>
              c.startsWith('Deploy to us-east-1 now or at the next window?'),
            ),
          ).toBe(true),
        { timeout: 8000 },
      )
      const chained = [...watch.cards].filter((c) => c.startsWith('Deploy to us-east-1'))
      expect(chained.every((c) => !c.endsWith(`Requested by ${copy.orchestratorName}`))).toBe(true)
      // The same agent that asked the first question (as its saved request names it).
      const first = [...watch.cards]
        .filter((c) => c.startsWith('Which region'))
        .map((c) => c.split(' :: ')[1])
      for (const c of chained) expect(first).toContain(c.split(' :: ')[1])
    } finally {
      watch.stop()
    }
  })
})

describe('an answered routed request whose reply can still arrive', () => {
  it.each([
    [
      'routed-reconnect-403',
      "This reply was started by another sign-in and can't be resumed here.",
    ],
    ['routed-reconnect-400', 'The reply may still be arriving.'],
  ] as const)(
    '%s: no Run again; the composer waits for the reply with Start a new chat (D3)',
    async (sc, notice) => {
      const { router } = await sendRouted('deploy', sc)
      await answer('us-east-1', 1)
      expect(await screen.findByText(notice, {}, { timeout: 8000 })).toBeInTheDocument()
      await waitFor(() => expect(screen.getByText(copy.lockedResumeArriving)).toBeInTheDocument())
      expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()
      expect(screen.getByLabelText('Ask the Orchestrator')).toBeInTheDocument()
      const sid = sessionIdOf(router)
      // Start a new chat from the lock stays routed.
      await userEvent.click(screen.getByRole('button', { name: copy.startNewChat }))
      await waitFor(() => expect(router.state.location.pathname).toBe('/chat'))
      expect((router.state.location.search as { auto?: unknown }).auto).toBe(1)
      expect(await screen.findByRole('heading', { name: 'Orchestrate a task' })).toBeInTheDocument()
      expect(chatMockMessages(sid).filter((m) => m.role === 'assistant')).toHaveLength(0)
    },
  )

  it('without the live turn (a reload, another tab): the quiet waiting row, Refresh status only, composer held', async () => {
    const { router, queryClient } = await sendRouted('deploy', 'routed-reconnect-403')
    await answer('us-east-1')
    await waitFor(() => expect(screen.getByText(copy.lockedResumeArriving)).toBeInTheDocument(), {
      timeout: 8000,
    })
    chatRegistry(queryClient, queryClient.getQueryData<{ sub: string }>(['me'])!.sub).forget(
      sessionIdOf(router),
    )

    const row = await screen.findByTestId('resume-waiting', {}, { timeout: 8000 })
    expect(row).toHaveTextContent(copy.resumeWaiting)
    expect(
      within(row)
        .getAllByRole('button')
        .map((b) => b.textContent),
    ).toEqual([copy.refreshStatus])
    expect(screen.queryByText(errorCopy.routedAnsweredNoReply.problem)).toBeNull()
    expect(screen.queryByTestId('checking-reply')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()
    expect(screen.getByText(copy.lockedResumeArriving)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: copy.startNewChat })).toBeInTheDocument()

    // Refresh status re-reads history.
    await rec.flush()
    const gets = () =>
      rec.requests.filter((r) => r.method === 'GET' && /\/messages$/.test(r.url.pathname)).length
    const before = gets()
    await userEvent.click(within(row).getByRole('button', { name: copy.refreshStatus }))
    await waitFor(async () => {
      await rec.flush()
      expect(gets()).toBeGreaterThan(before)
    })
  })

  it("past the window the lock lifts, history's notice offers Refresh status only, and Run again never shows (D3)", async () => {
    const lost = tuning.LOST_REPLY_AFTER_MS
    const delivery = tuning.RESUME_DELIVERY_MS
    // routed-reconnect-403 never delivers the answer (resume_status stays not_started): that's the delivery window.
    tuning.LOST_REPLY_AFTER_MS = 20_000
    tuning.RESUME_DELIVERY_MS = 20_000
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    try {
      const { router, queryClient } = await sendRouted('deploy', 'routed-reconnect-403', user)
      await answer('us-east-1', 1, user)
      await waitFor(() => expect(screen.getByText(copy.lockedResumeArriving)).toBeInTheDocument(), {
        timeout: 8000,
      })
      expect(screen.getByRole('button', { name: copy.startNewChat })).toBeInTheDocument()

      // The page's clock ticks every 15 s while nothing is live: two ticks pass the window.
      await vi.advanceTimersByTimeAsync(tuning.LOST_REPLY_AFTER_MS + 16_000)
      await waitFor(() => expect(screen.queryByText(copy.lockedResumeArriving)).toBeNull())
      expect(screen.queryByRole('button', { name: copy.startNewChat })).toBeNull()
      expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()

      // Without the live turn (a reload, another tab), history's notice speaks: Refresh status only.
      const me = queryClient.getQueryData<{ sub: string }>(['me'])!
      chatRegistry(queryClient, me.sub).forget(sessionIdOf(router))
      await waitFor(() => expect(screen.queryByTestId('routed-live')).toBeNull())
      const notice = (
        await screen.findByText(errorCopy.routedAnsweredNoReply.problem, {}, { timeout: 8000 })
      ).closest('[data-testid="error-notice"]') as HTMLElement
      expect(notice).not.toBeNull()
      expect(within(notice).getByRole('button', { name: 'Refresh status' })).toBeInTheDocument()
      expect(
        within(notice)
          .getAllByRole('button')
          .map((b) => b.textContent),
      ).not.toContain('Run again')
      expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()
      expect(screen.queryByText(copy.lockedResumeArriving)).toBeNull()
    } finally {
      tuning.LOST_REPLY_AFTER_MS = lost
      tuning.RESUME_DELIVERY_MS = delivery
    }
  })

  it('a resume the server gave up on does not hold the composer and never offers Run again (D3, round 2)', async () => {
    const { router, queryClient } = await sendRouted('deploy', 'routed-reconnect-400')
    await answer('us-east-1', 1)
    const sid = sessionIdOf(router)
    for (const h of chatMockRequests(sid)) if (h.status === 'resolved') h.resume_status = 'failed'
    await queryClient.invalidateQueries({ queryKey: ['chat'] })
    await waitFor(() =>
      expect(chatMockRequests(sid).some((h) => h.resume_status === 'failed')).toBe(true),
    )
    await waitFor(() => expect(screen.getByLabelText('Ask the Orchestrator')).toBeEnabled(), {
      timeout: 8000,
    })
    expect(screen.queryByText(copy.lockedResumeArriving)).toBeNull()
    expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()

    // Without the live turn, history's notice says nothing is coming, with Refresh status only.
    chatRegistry(queryClient, queryClient.getQueryData<{ sub: string }>(['me'])!.sub).forget(sid)
    const notice = (
      await screen.findByText(errorCopy.routedAnsweredNoReply.problem, {}, { timeout: 8000 })
    ).closest('[data-testid="error-notice"]') as HTMLElement
    expect(
      within(notice)
        .getAllByRole('button')
        .map((b) => b.textContent),
    ).toEqual(['Refresh status'])
    expect(screen.queryByText(copy.checkingForReply)).toBeNull()
    // No 10-minute "checking" lock either: the server said no reply is coming.
    expect(screen.getByLabelText('Ask the Orchestrator')).toBeEnabled()
    expect(screen.queryByText(copy.lockedChecking)).toBeNull()
    expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()
  })
})

describe('a resume this tab saw finish, whose reply history never shows', () => {
  it('never holds the composer: nothing more is coming', async () => {
    const saved = { ms: tuning.ROUTED_RECHECK_MS, n: tuning.ROUTED_RECHECKS }
    tuning.ROUTED_RECHECK_MS = 5
    tuning.ROUTED_RECHECKS = 1
    // History never returns the saved reply (the server saved it, the page never sees it).
    server.use(
      http.get('/api/chat/sessions/:id/messages', async ({ request }) => {
        if (request.headers.get('x-answered-inner')) return undefined
        const res = await fetch(request.url, { headers: { 'x-answered-inner': '1' } })
        const body = (await res.json()) as { data?: { role?: string }[] }
        return HttpResponse.json(
          { ...body, data: (body.data ?? []).filter((m) => m.role !== 'assistant') },
          { status: res.status },
        )
      }),
    )
    try {
      const { router, queryClient } = await sendRouted('deploy it', 'routed-hitl')
      await answer('us-east-1')
      // The resumed reply streams to its end, then the re-checks give up on history.
      expect(
        await screen.findByText('Done: deployed to us-east-1.', {}, { timeout: 8000 }),
      ).toBeInTheDocument()
      const reg = chatRegistry(queryClient, queryClient.getQueryData<{ sub: string }>(['me'])!.sub)
      await waitFor(() => expect(reg.get(sessionIdOf(router))?.phase).toBe('lost'), {
        timeout: 8000,
      })
      expect(screen.queryByText(copy.lockedResumeArriving)).toBeNull()
      expect(screen.getByLabelText('Ask the Orchestrator')).toBeEnabled()

      // After a reload the turn end still says it finished: no waiting row, no held composer.
      reg.forget(sessionIdOf(router))
      await waitFor(() => expect(screen.queryByTestId('routed-live')).toBeNull())
      expect(
        await screen.findByText(errorCopy.routedAnsweredNoReply.problem, {}, { timeout: 8000 }),
      ).toBeInTheDocument()
      expect(screen.queryByTestId('resume-waiting')).toBeNull()
      expect(screen.queryByText(copy.lockedResumeArriving)).toBeNull()
      expect(screen.getByLabelText('Ask the Orchestrator')).toBeEnabled()
      expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()
    } finally {
      tuning.ROUTED_RECHECK_MS = saved.ms
      tuning.ROUTED_RECHECKS = saved.n
    }
  })
})

describe('a paused routed chat', () => {
  it("a drained first send that history shows paused forgets the live block and shows history's request card, never a lost or empty reply (D3 part 2)", async () => {
    const saved = {
      cap: tuning.MAX_TURN_BYTES,
      ms: tuning.ROUTED_RECHECK_MS,
      n: tuning.ROUTED_RECHECKS,
    }
    // routed-hitl's first frame alone is over this: the stream drains, so its hitl frame is never decoded.
    tuning.MAX_TURN_BYTES = 150
    // Short re-checks, so a wrong settle (lost / known-empty) would surface within the test.
    tuning.ROUTED_RECHECK_MS = 5
    tuning.ROUTED_RECHECKS = 2
    try {
      const { router, queryClient } = await sendRouted('deploy it', 'routed-hitl')
      const card = await screen.findByTestId('request-card', {}, { timeout: 8000 })
      expect(within(card).getByRole('button', { name: 'us-east-1' })).toBeInTheDocument()
      await settled()
      // Positive signal first: the registry forgot the turn, so no settle (lost / known-empty) can follow.
      await waitFor(
        () =>
          expect(
            chatRegistry(queryClient, queryClient.getQueryData<{ sub: string }>(['me'])!.sub).get(
              sessionIdOf(router),
            ),
          ).toBeUndefined(),
        { timeout: 8000 },
      )
      expect(screen.queryByTestId('routed-live')).toBeNull()
      expect(screen.getByTestId('request-card')).toBeInTheDocument()
      expect(screen.queryByText('Partial reply — completion unconfirmed')).toBeNull()
      expect(screen.queryByText('The Orchestrator finished without a reply.')).toBeNull()
      expect(screen.queryByText(copy.replyTooLarge)).toBeNull()
      expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()
      expect(screen.getByRole('button', { name: copy.goToRequest })).toBeInTheDocument()
      expect(
        chatMockMessages(sessionIdOf(router)).filter((m) => m.role === 'assistant'),
      ).toHaveLength(0)
    } finally {
      tuning.MAX_TURN_BYTES = saved.cap
      tuning.ROUTED_RECHECK_MS = saved.ms
      tuning.ROUTED_RECHECKS = saved.n
    }
  })

  it('Start a new chat goes to the routed empty state (/chat?auto=1), not the direct chooser', async () => {
    const { router } = await sendRouted('deploy it', 'routed-hitl')
    await screen.findByTestId('request-card', {}, { timeout: 8000 })
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
    await userEvent.click(await screen.findByRole('button', { name: copy.startNewChat }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/chat'))
    const search = router.state.location.search as { auto?: unknown; agent?: unknown }
    expect(search.auto).toBe(1)
    expect(search.agent).toBeUndefined()
    expect(await screen.findByRole('heading', { name: 'Orchestrate a task' })).toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'Start a chat' })).toBeNull()
  })

  it('dismissed in another tab: history is the truth, the live block goes, and nothing reconnects (§5.4)', async () => {
    const { queryClient } = await sendRouted('deploy it', 'routed-hitl')
    await screen.findByTestId('request-card', {}, { timeout: 8000 })
    const pending = (await (await fetch(new URL('/api/hitl/pending', location.origin))).json()) as {
      data: { id: string }[]
    }
    const id = pending.data[0]!.id
    expect(
      (await fetch(new URL(`/api/hitl/${id}/cancel`, location.origin), { method: 'POST' })).ok,
    ).toBe(true)
    await queryClient.invalidateQueries({ queryKey: ['chat'] })
    await waitFor(() => expect(screen.queryByTestId('routed-live')).toBeNull(), { timeout: 8000 })
    expect(screen.queryByTestId('request-card')).toBeNull()
    // Proves an absence: a resume is queued on a later tick, so let it run before saying none happened.
    await new Promise((r) => setTimeout(r, 50))
    expect(chatMockReconnects()).toEqual([])
  })
})
