/** Previous/Next on the session trace (docs/superpowers/plans/2026-10-06-prev-next.md). */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { copy } from '@nasiko/features/observability/copy'
import { sessionKeys } from '@nasiko/features/sessions/api'
import { configureMocks } from '@nasiko/mocks/handlers'
import { SHOWCASE_SESSION } from '@nasiko/mocks/observability'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'

setupPinnedSeed()
afterEach(() => configureMocks({ variant: null }))

const rowLinks = async () =>
  within(await screen.findByRole('list', { name: 'Sessions' }))
    .getAllByRole('link')
    .filter((l) => l.getAttribute('href')?.startsWith('/sessions/'))
const pathOf = (href: string | null) => new URL(href!, 'http://x').pathname
const position = (n: number, of: number) =>
  screen.getByRole('group', { name: `session ${n} of ${of}` })

describe('session Previous/Next', () => {
  it('steps through the loaded rows, keeping the window and dropping the trace and span', async () => {
    const { router } = renderApp('/sessions?live=paused')
    const links = await rowLinks()
    const paths = links.map((l) => pathOf(l.getAttribute('href')))
    await userEvent.click(links[0]!)
    await userEvent.click(await screen.findByRole('button', { name: 'Next session' }))
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[1]))
    await waitFor(() => expect(position(2, paths.length)).toBeInTheDocument())
    // The new page's arrow has focus again after the remount.
    await waitFor(() => expect(screen.getByRole('button', { name: 'Next session' })).toHaveFocus())
    const search = router.state.location.search as Record<string, unknown>
    expect(search.trace).toBeUndefined()
    expect(search.span).toBeUndefined()
    expect(search.live).toBe('paused')
    await userEvent.keyboard('k')
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[0]))
    // Replaced, not pushed: Back is the list.
    router.history.back()
    await waitFor(() => expect(router.state.location.pathname).toBe('/sessions'))
  })

  it('keeps the list across a span step, and J/K steps sessions, not spans', async () => {
    const { router } = renderApp('/sessions?live=paused')
    const links = await rowLinks()
    const paths = links.map((l) => pathOf(l.getAttribute('href')))
    await userEvent.click(links[0]!)
    const nextSpan = await screen.findByRole('button', { name: 'Next span' }, { timeout: 8000 })
    await userEvent.click(nextSpan)
    await waitFor(() =>
      expect((router.state.location.search as Record<string, unknown>).span).toBeDefined(),
    )
    expect(position(1, paths.length)).toBeInTheDocument()
    await userEvent.keyboard('j')
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[1]))
    expect((router.state.location.search as Record<string, unknown>).span).toBeUndefined()
    await waitFor(() => expect(position(2, paths.length)).toBeInTheDocument())
  })

  it('stops at the last loaded row', async () => {
    renderApp('/sessions?live=paused')
    const links = await rowLinks()
    await userEvent.click(links.at(-1)!)
    expect(
      await screen.findByRole('group', { name: `session ${links.length} of ${links.length}` }),
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Next session' })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
  })

  it('keeps the arrows on a session that fails to load', async () => {
    const { router, queryClient } = renderApp('/sessions?live=paused')
    const links = await rowLinks()
    const paths = links.map((l) => pathOf(l.getAttribute('href')))
    const gone = decodeURIComponent(paths[1]!.split('/').pop()!)
    server.use(
      http.get(
        `/api/observability/session/${encodeURIComponent(gone)}`,
        () => new HttpResponse('not found', { status: 404 }),
      ),
    )
    // The list's status checks already read it; the trace page must read it again.
    queryClient.removeQueries({ queryKey: sessionKeys.detail(gone) })
    await userEvent.click(links[0]!)
    await userEvent.click(await screen.findByRole('button', { name: 'Next session' }))
    expect(await screen.findByText(copy.notFoundSession)).toBeInTheDocument()
    expect(position(2, paths.length)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Next session' }))
    await waitFor(() => expect(router.state.location.pathname).toBe(paths[2]))
  })

  it('shows no arrows on a deep link', async () => {
    renderApp(`/sessions/${SHOWCASE_SESSION}`)
    await screen.findByRole('heading', { level: 1, name: /^Session ·/ })
    expect(screen.queryByRole('button', { name: 'Next session' })).toBeNull()
  })
})
