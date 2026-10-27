/** Previous/Next on a chat: the rail's order (docs/superpowers/plans/2026-10-06-prev-next.md). */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { renderApp } from '@nasiko/testing/renderApp'
import { clearDrafts } from './drafts'
import { useRailOrder } from './railOrder'
import { clearChatRegistry } from './registry'

afterEach(() => {
  clearChatRegistry()
  clearDrafts()
  // The store is module state (a sign-out's full page load clears it in the app); don't leak it between tests.
  useRailOrder.setState({ ids: [] })
})

describe('chat Previous/Next', () => {
  it("steps through the rail's chats in the order shown; a click lands in the composer, J/K doesn't", async () => {
    const { router } = renderApp('/chat')
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    const rows = await within(rail).findAllByTestId('rail-row')
    const paths = rows.map((r) => r.getAttribute('href')!.split('?')[0])
    expect(paths.length).toBeGreaterThan(1)
    // A mouse click on a rail row opens the chat with the composer focused.
    await userEvent.click(rows[0]!)
    await screen.findByRole('group', { name: `chat 1 of ${paths.length}` })
    await waitFor(() => expect(screen.getByRole('textbox')).toHaveFocus())
    expect(screen.getByRole('button', { name: 'Previous chat' })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
    // So does a click on an arrow.
    await userEvent.click(screen.getByRole('button', { name: 'Next chat' }))
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[1]))
    await screen.findByRole('group', { name: `chat 2 of ${paths.length}` })
    await waitFor(() => expect(screen.getByRole('textbox')).toHaveFocus())
    // In the composer J/K is text, not a step.
    const composer = screen.getByRole('textbox')
    await userEvent.type(composer, 'j')
    expect(composer).toHaveValue('j')
    expect(router.state.location.pathname).toBe(paths[1])
    await userEvent.clear(composer)
    // Out of the composer, J/K steps, and the opened chat leaves focus alone so the next key steps again.
    composer.blur()
    await userEvent.keyboard('k')
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[0]))
    await screen.findByRole('group', { name: `chat 1 of ${paths.length}` })
    expect(screen.getByRole('textbox')).not.toHaveFocus()
    await userEvent.keyboard('j')
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[1]))
  })

  it('from an open chat, a rail click focuses the composer and Enter keeps focus in the rail', async () => {
    const { router } = renderApp('/chat')
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    await userEvent.click((await within(rail).findAllByTestId('rail-row'))[0]!)
    await screen.findByRole('group', { name: /^chat 1 of / })
    // The rail remounts when the first chat opens; between chats it stays.
    const rows = within(screen.getByRole('navigation', { name: 'Chats' })).getAllByTestId(
      'rail-row',
    )
    const pathOf = (i: number) => rows[i]!.getAttribute('href')!.split('?')[0]
    await userEvent.click(rows[1]!)
    await waitFor(() => expect(router.state.location.pathname).toBe(pathOf(1)))
    await screen.findByRole('group', { name: /^chat 2 of / })
    await waitFor(() => expect(screen.getByRole('textbox')).toHaveFocus())
    rows[0]!.focus()
    await userEvent.keyboard('{Enter}')
    await waitFor(() => expect(router.state.location.pathname).toBe(pathOf(0)))
    await screen.findByRole('group', { name: /^chat 1 of / })
    expect(rows[0]).toHaveFocus()
  })

  it('the Waiting view has its own order, so the arrows go', async () => {
    renderApp('/chat')
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    const rows = await within(rail).findAllByTestId('rail-row')
    await userEvent.click(rows[0]!)
    await screen.findByRole('group', { name: /^chat 1 of / })
    // The rail remounts when a chat opens: query it again.
    const openRail = screen.getByRole('navigation', { name: 'Chats' })
    await userEvent.click(await within(openRail).findByRole('radio', { name: /^Waiting/ }))
    await waitFor(() => expect(screen.queryByRole('group', { name: /^chat / })).toBeNull())
    expect(screen.queryByRole('button', { name: 'Next chat' })).toBeNull()
  })
})
