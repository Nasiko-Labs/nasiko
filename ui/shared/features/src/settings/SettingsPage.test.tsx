import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { copy as shellCopy } from '@nasiko/app-core/shell/copy'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { recordRequestBodies, server } from '@nasiko/testing/setup'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 12000 }
const f = copy.fields
const sectionNav = () => screen.findByRole('navigation', { name: copy.nav.label }, T)
/** A settings row (`SettingRow`'s grid), found by its label. */
const rowOf = (label: string) => screen.getByText(label).closest<HTMLElement>('.grid')!

describe('Settings: workspace sections (plans/feat-settings.md §1.1)', () => {
  it('opens from Organization → Settings on General, with the legacy module nav (no SSO in OSS)', async () => {
    const { router } = renderApp('/')
    const main = await screen.findByRole('navigation', { name: 'Main' }, T)
    await userEvent.click(within(main).getByRole('link', { name: 'Settings' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/settings'))
    expect(await screen.findByRole('heading', { level: 1, name: 'General' }, T)).toBeInTheDocument()
    // The models and default provider are the server's, shown read-only, each under its own label: no field to edit.
    await screen.findByText(copy.running.orchestrator_model.label, {}, T)
    for (const [label, value] of [
      [copy.running.orchestrator_model.label, 'gpt-4o-mini'],
      [copy.running.router_model.label, 'gpt-4.1-mini'],
      [copy.running.default_provider.label, 'openai'],
    ]) {
      const row = within(rowOf(label))
      expect(row.getByText(value)).toBeInTheDocument()
      expect(row.getByText(copy.running.setByServer)).toBeInTheDocument()
    }
    expect(screen.queryByRole('textbox', { name: /model/i })).not.toBeInTheDocument()
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument()
    const nav = await sectionNav()
    expect(
      within(nav)
        .getAllByRole('link')
        .map((l) => l.textContent),
      // Workspace → Optimization tiers: plans/feat-context-optimization.md (eng E4). Appearance and Password moved to
      // the account menu's /account (intended nav changes).
    ).toEqual([
      'General',
      'Flow limits',
      'Optimization tiers',
      'Registry',
      'MCP toolkits',
      'Secrets',
    ])
    expect(within(nav).getByRole('link', { name: 'General' })).toHaveAttribute(
      'aria-current',
      'page',
    )
  })

  it('keeps edits across sections and saves them with one Save, every column from a fresh read', async () => {
    const bodies = recordRequestBodies()
    const { router } = renderApp('/settings')
    const tabs = await screen.findByLabelText(f.catalog_tabs.label, {}, T)
    await userEvent.type(tabs, 'devops, finance')
    // A section is a view of one form: switching doesn't ask to leave, and the edit survives (so Save stays).
    await userEvent.click(within(await sectionNav()).getByRole('link', { name: 'Flow limits' }))
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Flow limits' }),
    ).toBeInTheDocument()
    expect(router.state.location.search).toEqual({ section: 'limits' })
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument()
    expect(screen.getByText(copy.unsaved)).toBeInTheDocument()
    // Another admin saves the registry meanwhile: the PUT sends theirs back, not the page's stale copy.
    await fetch(new URL('/api/settings', location.origin), {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ registry_url: 'https://registry.example.com', catalog_tabs: null }),
    })
    await userEvent.click(screen.getByRole('button', { name: copy.save }))
    expect(await screen.findByText(copy.saved, {}, T)).toBeInTheDocument()
    await bodies.flush()
    const put = bodies.requests
      .filter((r) => r.method === 'PUT' && r.url.pathname === '/api/settings')
      .at(-1)
    // Only what is stored: the running values are never sent.
    expect(put?.body).toEqual({
      registry_url: 'https://registry.example.com',
      catalog_tabs: 'devops, finance',
    })
    bodies.stop()
    // The fresh form shows the saved row, and saves again.
    await userEvent.click(within(await sectionNav()).getByRole('link', { name: 'Registry' }))
    expect(await screen.findByLabelText(f.registry_url.label)).toHaveValue(
      'https://registry.example.com',
    )
  })

  it('shows the flow limits the server runs, read-only, with nothing to save', async () => {
    renderApp('/settings?section=limits')
    await screen.findByText(copy.running.flow_timeout_secs.label, {}, T)
    for (const [label, value] of [
      [copy.running.max_flow_depth.label, '5'],
      [copy.running.max_flow_fan_out.label, '20'],
      [copy.running.max_flow_tokens.label, '100,000'],
      [copy.running.flow_timeout_secs.label, '600'],
    ]) {
      const row = within(rowOf(label))
      expect(row.getByText(value)).toBeInTheDocument()
      expect(row.getByText(copy.running.setByServer)).toBeInTheDocument()
    }
    expect(screen.queryByRole('spinbutton')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: copy.save })).not.toBeInTheDocument()
  })

  it('shows nothing it runs from a server that does not report the running values', async () => {
    // An older server answers its stored column (or the old hard-coded default) and no `orchestrator_model`.
    server.use(
      http.get('/api/settings', () =>
        HttpResponse.json({
          router_model: 'deepseek-v4-pro',
          default_provider: 'openai',
          max_flow_depth: 5,
          max_flow_fan_out: 20,
          max_flow_tokens: 100000,
          flow_timeout_secs: 120,
          registry_url: null,
          catalog_tabs: null,
        }),
      ),
    )
    renderApp('/settings')
    expect(await screen.findByLabelText(f.catalog_tabs.label, {}, T)).toBeInTheDocument()
    expect(screen.queryByText(copy.running.router_model.label)).not.toBeInTheDocument()
    expect(screen.queryByText(copy.running.default_provider.label)).not.toBeInTheDocument()
    expect(screen.queryByText('deepseek-v4-pro')).not.toBeInTheDocument()
    // Its stored timeout (the old hard-coded 120) is not what runs, so Flow limits says so instead of showing it.
    await userEvent.click(within(await sectionNav()).getByRole('link', { name: 'Flow limits' }))
    expect(await screen.findByText(copy.running.notReported)).toBeInTheDocument()
    expect(screen.queryByText('120')).not.toBeInTheDocument()
  })

  it('shows the section of a field that fails, instead of saving nothing silently', async () => {
    const { router } = renderApp('/settings?section=registry')
    await userEvent.type(await screen.findByLabelText(f.registry_url.label, {}, T), 'not a host')
    await userEvent.click(within(await sectionNav()).getByRole('link', { name: 'General' }))
    await userEvent.click(await screen.findByRole('button', { name: copy.save }))
    await waitFor(() => expect(router.state.location.search).toEqual({ section: 'registry' }))
    expect(await screen.findByText(f.registry_url.invalid)).toBeInTheDocument()
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

  it('sends a member to their secrets, with only Secrets in the nav', async () => {
    configureMocks({ superuser: false })
    const { router } = renderApp('/settings')
    await waitFor(() => expect(router.state.location.pathname).toBe('/settings/secrets'), T)
    const nav = await sectionNav()
    expect(
      within(nav)
        .getAllByRole('link')
        .map((l) => l.textContent),
    ).toEqual(['Secrets'])
  })

  it('the account menu’s Settings opens Account settings: Appearance, Password and Keyboard shortcuts', async () => {
    const { router } = renderApp('/')
    await userEvent.click(await screen.findByRole('button', { name: /^Account: / }, T))
    await userEvent.click(
      await screen.findByRole('menuitem', { name: shellCopy.account.settings }, T),
    )
    await waitFor(() => expect(router.state.location.pathname).toBe('/account/appearance'))
    const nav = await sectionNav()
    expect(within(nav).getByText('Account settings')).toBeInTheDocument()
    expect(
      within(nav)
        .getAllByRole('link')
        .map((l) => l.textContent),
    ).toEqual(['Appearance', 'Password', 'Keyboard shortcuts'])
  })

  it('Keyboard shortcuts lists the keys, ⌘/Ctrl spelled for this platform', async () => {
    renderApp('/account/shortcuts')
    expect(
      await screen.findByRole('heading', { name: 'Keyboard shortcuts', level: 1 }, T),
    ).toBeInTheDocument()
    expect(screen.getByText('Collapse or expand the sidebar')).toBeInTheDocument()
    expect(screen.getByText('Ctrl')).toBeInTheDocument()
    expect(screen.queryByText('Mod')).not.toBeInTheDocument()
  })

  it('an old Settings → Password link lands on Account settings', async () => {
    const { router } = renderApp('/settings/password')
    await waitFor(() => expect(router.state.location.pathname).toBe('/account/password'), T)
  })

  it('an old Settings → Chat context link lands on Optimization (fb633d4)', async () => {
    const { router } = renderApp('/settings/chat-context')
    // The page drops `#settings` itself once it has jumped there, so only the path is stable.
    await waitFor(() => expect(router.state.location.pathname).toBe('/optimization'), {
      timeout: 12_000,
    })
  })
})
