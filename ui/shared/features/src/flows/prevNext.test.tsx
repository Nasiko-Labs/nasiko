/** Previous/Next on a flow (docs/superpowers/plans/2026-10-06-prev-next.md). */
import { cleanup, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, flowsMockState } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { flowKeys } from './api'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const flowLinks = async () =>
  within(await screen.findByRole('table', { name: copy.list.tableLabel }, { timeout: 8000 }))
    .getAllByRole('link')
    .filter((l) => l.getAttribute('href')?.startsWith('/flows/'))
const pathOf = (l: HTMLElement) => l.getAttribute('href')!.split('?')[0]!
const position = (n: number, of: number) =>
  screen.getByRole('group', { name: `flow ${n} of ${of}` })

describe('flow Previous/Next', () => {
  it('steps through the table rows with the buttons and K', async () => {
    const { router } = renderApp('/flows')
    const links = await flowLinks()
    const paths = links.map(pathOf)
    await userEvent.click(links[0]!)
    await userEvent.click(
      await screen.findByRole('button', { name: 'Next flow' }, { timeout: 8000 }),
    )
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[1]))
    await waitFor(() => expect(position(2, paths.length)).toBeInTheDocument())
    // The new page's arrow has focus again after the remount.
    await waitFor(() => expect(screen.getByRole('button', { name: 'Next flow' })).toHaveFocus())
    await userEvent.keyboard('k')
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[0]))
    // Replaced, not pushed: Back is the list.
    router.history.back()
    await waitFor(() => expect(router.state.location.pathname).toBe('/flows'))
  })

  it('keeps the list across a step selection, and J on a focused swimlane bar steps the flow', async () => {
    const { router } = renderApp('/flows')
    const links = await flowLinks()
    const paths = links.map(pathOf)
    const showcase = flowsMockState().flows.find((f) => f.title?.startsWith('Compare 2026'))!.id
    const i = paths.indexOf(`/flows/${showcase}`)
    expect(i).toBeGreaterThanOrEqual(0)
    expect(i).toBeLessThan(paths.length - 1)
    await userEvent.click(links[i]!)
    const panel = (
      await screen.findByRole('heading', { name: copy.panel.title }, { timeout: 8000 })
    ).closest('section')!
    await waitFor(() => expect(within(panel).getByText(copy.panel.merged)).toBeInTheDocument(), {
      timeout: 8000,
    })
    const chart = within(panel).getByRole('group', { name: /^Timeline of/ })
    within(chart)
      .getAllByRole('button')
      .find((b) => b.tabIndex === 0)!
      .focus()
    await userEvent.keyboard('{Enter}')
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ step: expect.any(String) }),
    )
    expect(position(i + 1, paths.length)).toBeInTheDocument()
    // J/K aren't the swimlane's keys (arrows, Home, End, Enter, Esc), so J on a bar steps the flow.
    await userEvent.keyboard('j')
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[i + 1]))
    expect((router.state.location.search as Record<string, unknown>).step).toBeUndefined()
    await waitFor(() => expect(position(i + 2, paths.length)).toBeInTheDocument())
  })

  it('keeps the arrows on a flow that is gone', async () => {
    const { router, queryClient } = renderApp('/flows')
    const links = await flowLinks()
    const paths = links.map(pathOf)
    const gone = decodeURIComponent(paths[1]!.split('/').pop()!)
    server.use(
      // The server's not-found is an empty 404 (as the mock's).
      http.get(
        `/api/flows/${encodeURIComponent(gone)}`,
        () => new HttpResponse(null, { status: 404 }),
      ),
    )
    queryClient.removeQueries({ queryKey: flowKeys.detail(gone) })
    await userEvent.click(links[0]!)
    await userEvent.click(
      await screen.findByRole('button', { name: 'Next flow' }, { timeout: 8000 }),
    )
    expect(await screen.findByText(copy.page.notFound)).toBeInTheDocument()
    expect(position(2, paths.length)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Next flow' }))
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[2]))
  })

  it('shows no arrows on a deep link', async () => {
    renderApp('/flows')
    const links = await flowLinks()
    const path = pathOf(links[0]!)
    // A fresh app at the flow's URL: no list in history state.
    cleanup()
    renderApp(path)
    await screen.findByRole('heading', { level: 1 }, { timeout: 8000 })
    expect(screen.queryByRole('button', { name: 'Next flow' })).toBeNull()
  })
})
