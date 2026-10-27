/**
 * /optimization's Workspace footer in the OSS build (plans/feat-optimization-page.md T7: R3A, R5B; §7 Workspace row).
 */
import { screen, within } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 12_000 }
const footer = async () =>
  (await screen.findByRole('heading', { level: 2, name: copy.workspace.title }, T)).closest(
    'section',
  ) as HTMLElement

describe('Workspace footer', () => {
  it('a superuser sees the tiers with Edit tiers; the OSS build says nothing about other editions', async () => {
    renderApp('/optimization')
    const f = await footer()
    expect(
      await within(f).findByText(copy.workspace.tierValues(500, 1000, 5000), {}, T),
    ).toBeInTheDocument()
    expect(within(f).getByText(copy.workspace.tiers)).toBeInTheDocument()
    expect(within(f).getByRole('link', { name: copy.workspace.editTiers })).toHaveAttribute(
      'href',
      '/settings/optimization-tiers',
    )
    expect(within(f).getAllByRole('listitem')).toHaveLength(1)
    expect(f.textContent).not.toMatch(/organization|enterprise|policy/i)
  })

  it("today's server: the values are labelled defaults (2B)", async () => {
    configureMocks({ variant: 'optimization-classic' })
    renderApp('/optimization')
    const f = await footer()
    expect(await within(f).findByText(copy.workspace.defaults, {}, T)).toBeInTheDocument()
    expect(within(f).getByText(copy.workspace.tierValues(500, 1000, 5000))).toBeInTheDocument()
  })

  it('a failed tiers read says so with Retry', async () => {
    server.use(
      http.get(
        '/api/settings/context-tiers',
        () => new HttpResponse('internal error', { status: 500 }),
      ),
    )
    renderApp('/optimization')
    const f = await footer()
    expect(await within(f).findByText(copy.workspace.tiersFailed, {}, T)).toBeInTheDocument()
    expect(within(f).getByRole('button', { name: copy.retry })).toBeInTheDocument()
  })

  it('members never see it (R3A)', async () => {
    configureMocks({ superuser: false })
    renderApp('/optimization')
    await screen.findByRole('button', { name: copy.save }, T)
    expect(screen.queryByRole('heading', { level: 2, name: copy.workspace.title })).toBeNull()
  })
})
