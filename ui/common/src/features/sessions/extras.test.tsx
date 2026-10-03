import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { presenterStep } from '@/app/shell/context'
import { copy } from '@/features/observability/copy'
import { readAnchor } from '@/mocks/anchor'
import { seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { parseSse } from '@/lib/sse'

setupPinnedSeed()

describe('presenter mode (?demo=1)', () => {
  it('the step follows the URL through the three-click path', () => {
    expect(presenterStep('/tokenops', {})).toBe(0)
    expect(presenterStep('/sessions', { day: '2026-03-11' })).toBe(1)
    expect(presenterStep('/sessions', {})).toBeNull()
    expect(presenterStep('/sessions/5eed-sess-pr-481', {})).toBe(2)
  })

  it('shows the card on TokenOps, advances on Sessions, and can be closed', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?demo=1')
    const card = await screen.findByRole('complementary', { name: 'Demo steps' })
    expect(card).toHaveTextContent('Step 1 of 3')
    await user.click(await screen.findByRole('link', { name: /See sessions/ }))
    await waitFor(() =>
      expect(screen.getByRole('complementary', { name: 'Demo steps' })).toHaveTextContent(
        'Step 2 of 3',
      ),
    )
    expect(router.state.location.search).toMatchObject({ demo: true })
    await user.click(screen.getByRole('button', { name: 'Close demo steps' }))
    await waitFor(() =>
      expect(screen.queryByRole('complementary', { name: 'Demo steps' })).toBeNull(),
    )
  })
})

describe('log drawer', () => {
  it('parses SSE events across chunk boundaries', () => {
    const a = parseSse('data: {"a":1}\n\nevent: close\ndata: bye\n\ndata: {"b"')
    expect(a.events).toEqual([
      { event: 'message', data: '{"a":1}' },
      { event: 'close', data: 'bye' },
    ])
    expect(a.rest).toBe('data: {"b"')
    expect(parseSse(a.rest + ':2}\n\n').events).toEqual([{ event: 'message', data: '{"b":2}' }])
  })

  it('streams the agent\'s lines, ends with "Stream paused", and Reconnect does not duplicate lines', async () => {
    const user = userEvent.setup()
    renderApp(`/sessions?day=${seed.spikeDate}`)
    const first = within(await screen.findByRole('list', { name: 'Sessions' })).getAllByRole(
      'link',
    )[0]
    await user.click(
      first.parentElement!.querySelector('button[aria-expanded]') as HTMLButtonElement,
    )
    await user.click(await screen.findByRole('button', { name: /View Code Reviewer logs/ }))
    const log = await screen.findByRole('log', { name: 'Log lines' })
    await waitFor(() => expect(within(log).getAllByText(/seed-code-reviewer:/)).toHaveLength(30))
    expect(await screen.findByText(copy.streamPaused)).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: /Reconnect/ }))
    await waitFor(() => expect(screen.getByText(copy.streamPaused)).toBeInTheDocument())
    expect(within(log).getAllByText(/seed-code-reviewer:/)).toHaveLength(30)
  })

  it('a 404 says the logs are unavailable', async () => {
    const user = userEvent.setup()
    server.use(
      http.get(
        '/api/observability/agents/:agent/logs/stream',
        () => new HttpResponse('Agent not found', { status: 404 }),
      ),
    )
    renderApp(`/sessions?day=${seed.spikeDate}`)
    const first = within(await screen.findByRole('list', { name: 'Sessions' })).getAllByRole(
      'link',
    )[0]
    await user.click(
      first.parentElement!.querySelector('button[aria-expanded]') as HTMLButtonElement,
    )
    await user.click(await screen.findByRole('button', { name: /View Code Reviewer logs/ }))
    expect(await screen.findByText(copy.logsUnavailable)).toBeInTheDocument()
  })
})

describe('anchor', () => {
  it('reads ?anchor (or the env default) as 15:00 UTC that day; junk is ignored', () => {
    expect(readAnchor('?anchor=2026-09-26', undefined)?.toISOString()).toBe(
      '2026-09-26T15:00:00.000Z',
    )
    expect(readAnchor('', '2026-01-02')?.toISOString()).toBe('2026-01-02T15:00:00.000Z')
    expect(readAnchor('?anchor=2026-02-30', undefined)).toBeNull()
  })
})
