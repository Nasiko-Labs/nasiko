import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { copy as shellCopy } from '@/app/shell/copy'
import { configureMocks } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { recordRequestBodies } from '@/test/setup'
import { renderApp } from '@/test/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 5000 }
const f = copy.fields
const sectionNav = () => screen.findByRole('navigation', { name: copy.nav.label }, T)

describe('Settings: workspace sections (plans/feat-settings.md §1.1)', () => {
  it('opens from the account menu on General, with the legacy module nav (no SSO in OSS)', async () => {
    const { router } = renderApp('/')
    await userEvent.click(await screen.findByRole('button', { name: /^Account: / }, T))
    await userEvent.click(
      await screen.findByRole('menuitem', { name: shellCopy.account.settings }, T),
    )
    await waitFor(() => expect(router.state.location.pathname).toBe('/settings'))
    expect(await screen.findByRole('heading', { level: 1, name: 'General' }, T)).toBeInTheDocument()
    expect(await screen.findByLabelText(f.router_model.label, {}, T)).toHaveValue('deepseek-v4-pro')
    const nav = await sectionNav()
    expect(
      within(nav)
        .getAllByRole('link')
        .map((l) => l.textContent),
    ).toEqual([
      'General',
      'Flow limits',
      'Registry',
      'Secrets',
      'Chat context',
      'Appearance',
      'Password',
    ])
    expect(within(nav).getByRole('link', { name: 'General' })).toHaveAttribute(
      'aria-current',
      'page',
    )
  })

  it('keeps edits across sections and saves them with one Save, every column from a fresh read', async () => {
    const bodies = recordRequestBodies()
    const { router } = renderApp('/settings')
    const model = await screen.findByLabelText(f.router_model.label, {}, T)
    await userEvent.clear(model)
    await userEvent.type(model, 'gpt-4o')
    // A section is a view of one form: switching doesn't ask to leave, and the edit survives.
    await userEvent.click(within(await sectionNav()).getByRole('link', { name: 'Flow limits' }))
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Flow limits' }),
    ).toBeInTheDocument()
    expect(router.state.location.search).toEqual({ section: 'limits' })
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument()
    const depth = screen.getByLabelText(f.max_flow_depth.label)
    await userEvent.clear(depth)
    await userEvent.type(depth, '8')
    // Another admin saves the registry meanwhile: the PUT sends theirs back, not the page's stale copy.
    await fetch(new URL('/api/settings', location.origin), {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        router_model: 'x',
        default_provider: 'openai',
        max_flow_depth: 5,
        max_flow_fan_out: 20,
        max_flow_tokens: 100000,
        flow_timeout_secs: 120,
        registry_url: 'https://registry.example.com',
        catalog_tabs: null,
      }),
    })
    await userEvent.click(screen.getByRole('button', { name: copy.save }))
    expect(await screen.findByText(copy.saved, {}, T)).toBeInTheDocument()
    await bodies.flush()
    const put = bodies.requests
      .filter((r) => r.method === 'PUT' && r.url.pathname === '/api/settings')
      .at(-1)
    expect(put?.body).toEqual({
      router_model: 'gpt-4o',
      default_provider: 'openai',
      max_flow_depth: 8,
      max_flow_fan_out: 20,
      max_flow_tokens: 100000,
      flow_timeout_secs: 120,
      registry_url: 'https://registry.example.com',
      catalog_tabs: null,
    })
    bodies.stop()
    // The fresh form shows the saved row, and saves again.
    await userEvent.click(within(await sectionNav()).getByRole('link', { name: 'Registry' }))
    expect(await screen.findByLabelText(f.registry_url.label)).toHaveValue(
      'https://registry.example.com',
    )
  })

  it('shows the section of a field that fails, instead of saving nothing silently', async () => {
    const { router } = renderApp('/settings?section=limits')
    const timeout = await screen.findByLabelText(f.flow_timeout_secs.label, {}, T)
    await userEvent.clear(timeout)
    await userEvent.click(within(await sectionNav()).getByRole('link', { name: 'Registry' }))
    await userEvent.click(await screen.findByRole('button', { name: copy.save }))
    await waitFor(() => expect(router.state.location.search).toEqual({ section: 'limits' }))
    expect(await screen.findByText(copy.positiveInt)).toBeInTheDocument()
  })

  it('asks before leaving with unsaved edits', async () => {
    renderApp('/settings')
    await userEvent.type(await screen.findByLabelText(f.catalog_tabs.label, {}, T), 'devops')
    await userEvent.click(within(await sectionNav()).getByRole('link', { name: 'Secrets' }))
    expect(
      await screen.findByRole('alertdialog', { name: 'Leave without saving?' }),
    ).toBeInTheDocument()
  })

  it('an unknown section falls back to General', async () => {
    renderApp('/settings?section=sso')
    expect(await screen.findByRole('heading', { level: 1, name: 'General' }, T)).toBeInTheDocument()
  })

  it('sends a member to their secrets, with only Secrets, Chat context, Appearance and Password in the nav', async () => {
    configureMocks({ superuser: false })
    const { router } = renderApp('/settings')
    await waitFor(() => expect(router.state.location.pathname).toBe('/settings/secrets'), T)
    const nav = await sectionNav()
    expect(
      within(nav)
        .getAllByRole('link')
        .map((l) => l.textContent),
    ).toEqual(['Secrets', 'Chat context', 'Appearance', 'Password'])
  })
})
