/**
 * Chat v1c M2, Sessions → Open chat (plans/feat-chat-v1c.md §5.10, §7 test 15): the Sessions row detail and
 * the trace header link the showcase chat; a 404 hides the link; `probe-500` offers Retry; `weave_` ids are
 * never probed; the slot is reserved while probing; deleting the chat drops its probe.
 */
import { QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import axe from 'axe-core'
import { http } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { SHOWCASE_SESSION } from '@/mocks/observability'
import { createQueryClient } from '@/lib/queryClient'
import { seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { OpenChatLink } from './components/OpenChatLink'
import { clearChatRegistry } from './registry'

setupPinnedSeed()
afterEach(() => {
  configureMocks({ variant: null })
  server.events.removeAllListeners()
  clearChatRegistry()
})

const probes = () => {
  const seen: string[] = []
  server.events.on('request:start', ({ request }) => {
    const u = new URL(request.url)
    if (
      /^\/api\/chat\/sessions\/[^/]+\/messages$/.test(u.pathname) &&
      u.searchParams.get('limit') === '1'
    )
      seen.push(decodeURIComponent(u.pathname.split('/')[4]!))
  })
  return seen
}

describe('Open chat (test 15)', () => {
  it('the trace header links the showcase chat', async () => {
    renderApp(`/sessions/${SHOWCASE_SESSION}`)
    const link = await screen.findByRole('link', { name: 'Open chat' }, { timeout: 8000 })
    expect(link).toHaveAttribute('href', `/chat/${SHOWCASE_SESSION}`)
  })

  it('the Sessions row detail links it while open, and a session with no chat shows nothing once the probe 404s', async () => {
    const seen = probes()
    // The showcase session is the costliest on the seed's spike day.
    renderApp(`/sessions?day=${seed.spikeDate}&sort=cost`)
    const list = await screen.findByRole('list', { name: 'Sessions' }, { timeout: 8000 })
    const rowLink = within(list)
      .getAllByRole('link')
      .find((el) => el.getAttribute('href')?.startsWith(`/sessions/${SHOWCASE_SESSION}`))!
    expect(seen).toEqual([])
    const li = rowLink.closest('li')!
    await userEvent.click(within(li).getByRole('button', { name: /^Show details for/ }))
    expect(await within(li).findByRole('link', { name: 'Open chat' })).toHaveAttribute(
      'href',
      `/chat/${SHOWCASE_SESSION}`,
    )
    // Another row: its probe 404s and the slot goes.
    const other = [...document.querySelectorAll('li')].find(
      (el) => el !== li && el.querySelector('button[aria-label^="Show details for"]'),
    )!
    await userEvent.click(within(other).getByRole('button', { name: /^Show details for/ }))
    await waitFor(() => expect(seen.length).toBe(2))
    await waitFor(() => expect(within(other).queryByTestId('open-chat-slot')).toBeNull())
    expect(within(other).queryByRole('link', { name: 'Open chat' })).toBeNull()
  }, 20_000)

  it('a failed probe offers Retry, and Retry brings the link back', async () => {
    configureMocks({ variant: 'probe-500' })
    renderApp(`/sessions/${SHOWCASE_SESSION}`)
    const note = await screen.findByTestId('open-chat-error', {}, { timeout: 8000 })
    expect(note).toHaveTextContent("Couldn't check chat availability")
    configureMocks({ variant: null })
    await userEvent.click(within(note).getByRole('button', { name: /Retry/ }))
    expect(await screen.findByRole('link', { name: 'Open chat' })).toBeInTheDocument()
  })

  it('reserves its slot while the probe runs', async () => {
    server.use(
      http.get('/api/chat/sessions/:id/messages', () => new Promise<Response>(() => undefined)),
    )
    renderApp(`/sessions/${SHOWCASE_SESSION}`)
    expect(await screen.findByTestId('open-chat-slot', {}, { timeout: 8000 })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
  })

  it('never probes a weave_ id', async () => {
    const seen = probes()
    render(
      <QueryClientProvider client={createQueryClient(() => undefined, { retry: false })}>
        <OpenChatLink sessionId="weave_abc" />
      </QueryClientProvider>,
    )
    // Proves an absence (no probe request).
    await new Promise((r) => setTimeout(r, 50))
    expect(seen).toEqual([])
    expect(screen.queryByTestId('open-chat-slot')).toBeNull()
  })

  it('deleting the chat drops its probe, so Open chat goes', async () => {
    const { router } = renderApp(`/sessions/${SHOWCASE_SESSION}`)
    await screen.findByRole('link', { name: 'Open chat' }, { timeout: 8000 })
    await router.navigate({ to: '/chat/$sessionId', params: { sessionId: SHOWCASE_SESSION } })
    await userEvent.click(await screen.findByRole('button', { name: 'More chat actions' }))
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }))
    await userEvent.click(
      within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Delete' }),
    )
    await waitFor(() => expect(router.state.location.pathname).toBe('/chat'))
    await router.navigate({
      to: '/sessions/$sessionId',
      params: { sessionId: SHOWCASE_SESSION },
      search: {} as never,
    })
    await screen.findByText(/^Session ·/, {}, { timeout: 8000 })
    await waitFor(() => expect(screen.queryByTestId('open-chat-slot')).toBeNull())
    expect(screen.queryByRole('link', { name: 'Open chat' })).toBeNull()
  })
})

describe('axe (test 16, M2 screens)', () => {
  const violations = async () => {
    const result = await axe.run(document.body, {
      rules: { 'color-contrast': { enabled: false }, region: { enabled: false } },
    })
    return result.violations.map(
      (v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`,
    )
  }

  it('the trace header with Open chat', async () => {
    renderApp(`/sessions/${SHOWCASE_SESSION}`)
    await screen.findByRole('link', { name: 'Open chat' }, { timeout: 8000 })
    expect(await violations()).toEqual([])
  })

  it('a Sessions row detail with Open chat', async () => {
    renderApp(`/sessions?day=${seed.spikeDate}&sort=cost`)
    const list = await screen.findByRole('list', { name: 'Sessions' }, { timeout: 8000 })
    const li = within(list)
      .getAllByRole('link')
      .find((el) => el.getAttribute('href')?.startsWith(`/sessions/${SHOWCASE_SESSION}`))!
      .closest('li')!
    await userEvent.click(within(li).getByRole('button', { name: /^Show details for/ }))
    await within(li).findByRole('link', { name: 'Open chat' })
    expect(await violations()).toEqual([])
  })
})
