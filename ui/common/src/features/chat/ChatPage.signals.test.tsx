/**
 * Chat v1c M3, page level (plans/feat-chat-v1c.md §5.8, §7 test 13 and the page parts of tests 20 and E2):
 * a reply that finishes while you're elsewhere spins, then dots its rail row; Reply ready; the hidden tab;
 * the phone trigger; the tab title.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import axe from 'axe-core'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureChatMock } from '@/mocks/chatStore'
import { renderApp } from '@/test/renderApp'
import { clearDrafts } from './drafts'
import { chatRegistry, clearChatRegistry } from './registry'

const DONE = '5eedc000-0000-4000-8000-00000000c001'
/** A finished direct chat (the observability showcase, M2); c002 would ask before sending (its last message has no reply). */
const SHOWCASE = '5eed-sess-pr-481'
const ROUTED = '5eedc000-0000-4000-8000-00000000c003'

let visibility: DocumentVisibilityState = 'visible'
function setVisibility(v: DocumentVisibilityState) {
  visibility = v
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => visibility })
  document.dispatchEvent(new Event('visibilitychange'))
}

afterEach(() => {
  setVisibility('visible')
  clearChatRegistry()
  clearDrafts()
  vi.unstubAllGlobals()
})

const rail = () => screen.getByRole('navigation', { name: 'Chats' })
const rowOf = (id: string) =>
  within(rail())
    .getAllByTestId('rail-row')
    .find((r) => r.getAttribute('href')?.startsWith(`/chat/${id}`))!
const markOf = (id: string) =>
  within(rowOf(id)).getByTestId('row-indicator').firstElementChild?.getAttribute('data-mark') ??
  null

async function sendIn(id: string, text: string) {
  const box = await screen.findByLabelText(/^Message /)
  await userEvent.type(box, text)
  // The chat's details load before Send unlocks.
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled())
  await userEvent.keyboard('{Enter}')
  await waitFor(() => expect(box).toHaveValue(''))
  void id
}

describe('background-turn signals (test 13)', () => {
  it('leave mid-stream: the row spins, then shows a new-reply dot; Reply ready links to it; opening the chat clears both', async () => {
    configureChatMock({ scenario: 'direct-slow' })
    const { router } = renderApp(`/chat/${DONE}`)
    await screen.findByRole('navigation', { name: 'Chats' })
    await sendIn(DONE, 'again')
    await waitFor(() => expect(markOf(DONE)).toBe('live'))
    expect(rowOf(DONE)).toHaveAccessibleName(/reply in progress/)
    await router.navigate({
      to: '/chat/$sessionId',
      params: { sessionId: ROUTED },
      search: {} as never,
    })
    await waitFor(() => expect(markOf(DONE)).toBe('unseen'), { timeout: 4000 })
    expect(rowOf(DONE)).toHaveAccessibleName(/new reply/)
    const ready = screen.getByTestId('reply-ready')
    expect(ready).toHaveTextContent('Reply ready in Summarise last week’s incidents')
    await userEvent.click(within(ready).getByRole('link'))
    await waitFor(() => expect(router.state.location.pathname).toBe(`/chat/${DONE}`))
    await waitFor(() => expect(markOf(DONE)).toBeNull())
    expect(screen.queryByTestId('reply-ready')).toBeNull()
  }, 15_000)

  it('Reply ready shows the newer of two finishes only; dismiss hides it and the dots stay', async () => {
    configureChatMock({ scenario: 'direct-slow' })
    const { router } = renderApp(`/chat/${DONE}`)
    await sendIn(DONE, 'one')
    await router.navigate({
      to: '/chat/$sessionId',
      params: { sessionId: SHOWCASE },
      search: {} as never,
    })
    await waitFor(() => expect(markOf(DONE)).toBe('unseen'), { timeout: 4000 })
    await sendIn(SHOWCASE, 'two')
    await router.navigate({
      to: '/chat/$sessionId',
      params: { sessionId: ROUTED },
      search: {} as never,
    })
    await waitFor(() => expect(markOf(SHOWCASE)).toBe('unseen'), { timeout: 4000 })
    const ready = screen.getByTestId('reply-ready')
    expect(ready).toHaveTextContent('Reply ready in Review PR #481')
    await userEvent.click(within(ready).getByRole('button', { name: 'Dismiss' }))
    expect(screen.queryByTestId('reply-ready')).toBeNull()
    expect(markOf(DONE)).toBe('unseen')
    expect(markOf(SHOWCASE)).toBe('unseen')
  }, 20_000)

  it('a finish while on another page still dots the row on return (E2)', async () => {
    configureChatMock({ scenario: 'direct-slow' })
    const { router, queryClient } = renderApp(`/chat/${DONE}`)
    await sendIn(DONE, 'while away')
    await router.navigate({ to: '/agents' })
    // The reply finishes while away: wait for its recorded end, not a fixed 2 s.
    const reg = chatRegistry(queryClient, queryClient.getQueryData<{ sub: string }>(['me'])!.sub)
    await waitFor(() => expect(reg.directEnds().some((e) => e.sessionId === DONE)).toBe(true), {
      timeout: 8000,
    })
    await router.navigate({
      to: '/chat/$sessionId',
      params: { sessionId: ROUTED },
      search: {} as never,
    })
    await waitFor(() => expect(markOf(DONE)).toBe('unseen'))
  }, 15_000)

  it('a reply that lands while the tab is hidden: the open row dots, the title counts it, and both clear once visible', async () => {
    configureChatMock({ scenario: 'direct-slow' })
    renderApp(`/chat/${DONE}`)
    await sendIn(DONE, 'hidden')
    setVisibility('hidden')
    await waitFor(() => expect(markOf(DONE)).toBe('unseen'), { timeout: 4000 })
    await waitFor(() => expect(document.title).toMatch(/^\(1\) /))
    setVisibility('visible')
    await waitFor(() => expect(markOf(DONE)).toBeNull())
    expect(document.title).not.toMatch(/^\(\d+\) /)
  }, 15_000)

  it('on a phone, the Chats trigger carries a dot and names the new reply', async () => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    configureChatMock({ scenario: 'direct-slow' })
    const { router } = renderApp(`/chat/${DONE}`)
    await sendIn(DONE, 'phone')
    await router.navigate({
      to: '/chat/$sessionId',
      params: { sessionId: ROUTED },
      search: {} as never,
    })
    const trigger = await screen.findByRole(
      'button',
      { name: 'Chats, 1 new reply' },
      { timeout: 4000 },
    )
    expect(within(trigger).getByTestId('trigger-dot')).toBeInTheDocument()
  }, 15_000)
})

describe('axe (test 16, M3 screens)', () => {
  const violations = async () => {
    const result = await axe.run(document.body, {
      rules: { 'color-contrast': { enabled: false }, region: { enabled: false } },
    })
    return result.violations.map(
      (v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`,
    )
  }

  it('the rail with a live spinner, then with a new-reply dot and Reply ready', async () => {
    configureChatMock({ scenario: 'direct-slow' })
    const { router } = renderApp(`/chat/${DONE}`)
    await screen.findByRole('navigation', { name: 'Chats' })
    await sendIn(DONE, 'again')
    await waitFor(() => expect(markOf(DONE)).toBe('live'))
    expect(await violations()).toEqual([])
    await router.navigate({
      to: '/chat/$sessionId',
      params: { sessionId: ROUTED },
      search: {} as never,
    })
    await waitFor(() => expect(markOf(DONE)).toBe('unseen'), { timeout: 4000 })
    await screen.findByTestId('reply-ready')
    expect(await violations()).toEqual([])
  }, 15_000)
})
