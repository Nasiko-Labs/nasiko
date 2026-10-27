/** The top bar's breadcrumb (TopBar.tsx): the nav item first, a level per sub-page, links for every level but the last. */
import { screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'

setupPinnedSeed()

const bar = () => document.querySelector<HTMLElement>('[data-slot="topbar"]')!
/** The breadcrumb's levels as text, with whether each is a link. */
const levels = () =>
  within(bar())
    .getByRole('navigation', { name: 'Breadcrumb' })
    .querySelectorAll('[data-slot="breadcrumb-item"]')
const trail = () =>
  [...levels()].map((li) => `${li.textContent}${li.querySelector('a') ? ' (link)' : ''}`)

async function expectTrail(url: string, want: string[]) {
  renderApp(url)
  await waitFor(() => expect(trail()).toEqual(want))
}

describe('top bar breadcrumb', () => {
  it('a module root is its nav group and item, neither a link', () =>
    expectTrail('/agents', ['Fleet', 'Agents']))

  it('a section page adds its level under a linked nav item', () =>
    expectTrail('/agents/mine', ['Fleet', 'Agents (link)', 'Your agents']))

  it('Settings names the open section', async () => {
    await expectTrail('/settings/secrets', ['Organization', 'Settings (link)', 'Secrets'])
  })

  it('Account settings is the first level of its own pages', () =>
    expectTrail('/account/password', ['Account settings (link)', 'Password']))

  it('Overview and New chat have no bar from md up (phones keep the menu button)', async () => {
    renderApp('/')
    await waitFor(() => expect(bar()).toHaveClass('md:hidden'))
    renderApp('/chat')
    await screen.findByRole('heading', { name: 'New chat' })
    expect(bar()).toHaveClass('md:hidden')
  })
})
