import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureMocks, mcpMockState } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { recordRequestBodies } from '@/test/setup'
import { renderApp } from '@/test/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 8000 }
/** A catalog card, found by its title. */
const card = async (label: string) =>
  (await screen.findByText(label, { selector: 'a, span.font-medium' }, T)).closest(
    '[data-slot="card"]',
  ) as HTMLElement

describe('MCP catalog (plans/feat-mcp.md §2)', () => {
  it('lists custom servers and toolkits with counts, and filters by scope, tab and search', async () => {
    renderApp('/mcp')
    expect(await screen.findByRole('link', { name: 'Docs search' }, T)).toHaveAttribute(
      'href',
      expect.stringMatching(/^\/mcp\/5eed000e-/),
    )
    // A toolkit has no page: plain text, not a link.
    expect(screen.queryByRole('link', { name: 'Gmail' })).toBeNull()
    expect(within(await card('Gmail')).getByText(copy.toolkit)).toBeInTheDocument()
    expect(within(await card('Finance ledger')).getByText(/^Shared by \S+/)).toBeInTheDocument()

    await userEvent.click(screen.getByRole('button', { name: copy.views.toolkits }))
    await waitFor(() => expect(screen.queryByText('Docs search')).toBeNull())
    expect(screen.getByText('Slack')).toBeInTheDocument()

    await userEvent.click(screen.getByRole('button', { name: copy.views.toolkits }))
    await userEvent.click(await screen.findByRole('tab', { name: /Connected/ }))
    await waitFor(() => expect(screen.queryByText('Slack')).toBeNull())
    expect(screen.getByText('GitHub')).toBeInTheDocument()

    await userEvent.click(screen.getByRole('tab', { name: /^All/ }))
    await userEvent.type(screen.getByRole('searchbox', { name: copy.searchLabel }), 'ledger')
    await waitFor(() => expect(screen.queryByText('GitHub')).toBeNull())
    expect(screen.getByText('Finance ledger')).toBeInTheDocument()
  })

  it('shows build states on upload cards, with no Connect control', async () => {
    renderApp('/mcp')
    const building = await card('CRM sync')
    expect(within(building).getByText(copy.building)).toBeInTheDocument()
    expect(within(building).queryByRole('button', { name: /Connect/ })).toBeNull()
    const failed = await card('SQL runner')
    expect(within(failed).getByText(copy.buildFailedHint)).toBeInTheDocument()
    expect(within(failed).queryByRole('button', { name: /Connect/ })).toBeNull()
  })

  it('connects a no-auth server at once, and disconnects behind a confirm', async () => {
    renderApp('/mcp')
    const wiki = await card('Wiki reader')
    await userEvent.click(
      within(wiki).getByRole('button', { name: copy.connectNamed('Wiki reader') }),
    )
    expect(await screen.findByText(copy.connectedToast('Wiki reader'), {}, T)).toBeInTheDocument()
    const off = await within(wiki).findByRole(
      'button',
      { name: copy.disconnectNamed('Wiki reader') },
      T,
    )
    await userEvent.click(off)
    const dialog = await screen.findByRole('alertdialog')
    expect(within(dialog).getByText(copy.disconnectBody)).toBeInTheDocument()
    await userEvent.click(within(dialog).getByRole('button', { name: copy.disconnect }))
    expect(
      await within(wiki).findByRole('button', { name: copy.connectNamed('Wiki reader') }, T),
    ).toBeInTheDocument()
  })

  it('asks for the key on an API-key server and sends it as credentials.value', async () => {
    const bodies = recordRequestBodies()
    renderApp('/mcp')
    const ledger = await card('Finance ledger')
    await userEvent.click(
      within(ledger).getByRole('button', { name: copy.connectNamed('Finance ledger') }),
    )
    const dialog = await screen.findByRole('dialog')
    await userEvent.click(within(dialog).getByRole('button', { name: copy.connect }))
    expect(await within(dialog).findByText(copy.required)).toBeInTheDocument()
    await userEvent.type(within(dialog).getByLabelText(copy.keyLabel), 'sk-ledger')
    await userEvent.click(within(dialog).getByRole('button', { name: copy.connect }))
    expect(
      await screen.findByText(copy.connectedToast('Finance ledger'), {}, T),
    ).toBeInTheDocument()
    await bodies.flush()
    expect(bodies.requests.find((r) => r.url.pathname === '/api/mcp/connect')?.body).toEqual({
      connector_id: expect.stringMatching(/^5eed000e-/),
      credentials: { value: 'sk-ledger' },
    })
    bodies.stop()
  })

  it('opens a toolkit’s OAuth page in a popup and refreshes when it closes', async () => {
    const popup = { closed: false }
    const open = vi.spyOn(window, 'open').mockReturnValue(popup as Window)
    renderApp('/mcp')
    const gmail = await card('Gmail')
    await userEvent.click(within(gmail).getByRole('button', { name: copy.connectNamed('Gmail') }))
    await waitFor(() =>
      expect(open).toHaveBeenCalledWith(
        'about:blank#composio-oauth',
        'mcp-oauth',
        'width=600,height=720',
      ),
    )
    popup.closed = true
    expect(
      await within(gmail).findByRole('button', { name: copy.disconnectNamed('Gmail') }, T),
    ).toBeInTheDocument()
  })

  it('offers both actions on an empty catalog, and not twice', async () => {
    configureMocks({ variant: 'mcp-empty' })
    renderApp('/mcp')
    expect(await screen.findByText(copy.empty.all.title, {}, T)).toBeInTheDocument()
    expect(screen.getAllByRole('button', { name: copy.register })).toHaveLength(1)
    expect(screen.getAllByRole('button', { name: copy.upload })).toHaveLength(1)
  })

  it('still lists custom servers when the toolkit list fails, and says so', async () => {
    configureMocks({ variant: 'mcp-toolkits-fail' })
    renderApp('/mcp')
    expect(await screen.findByText(copy.toolkitsFailed, {}, T)).toBeInTheDocument()
    expect(screen.getByText('Docs search')).toBeInTheDocument()
  })

  it('registers a server after probing it, then opens its page', async () => {
    const bodies = recordRequestBodies()
    const { router } = renderApp('/mcp')
    await userEvent.click(await screen.findByRole('button', { name: copy.register }, T))
    const dialog = await screen.findByRole('dialog')
    await userEvent.type(within(dialog).getByLabelText(copy.fields.name.label), 'tickets')
    await userEvent.type(
      within(dialog).getByLabelText(copy.fields.url.label),
      'https://tickets.example.com/api/mcp',
    )
    await userEvent.click(within(dialog).getByRole('button', { name: copy.probe }))
    expect(await within(dialog).findByText(copy.probeResult('API key'))).toBeInTheDocument()
    // Probe set the auth type, so the bearer field shows.
    expect(
      within(dialog).getByLabelText(copy.fields.credential_header_name.label),
    ).toBeInTheDocument()
    await userEvent.click(within(dialog).getByRole('button', { name: copy.registerTitle }))
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/mcp\/5eed000e-/), T)
    await bodies.flush()
    expect(
      bodies.requests.find((r) => r.method === 'POST' && r.url.pathname === '/api/mcp/connectors')
        ?.body,
    ).toEqual({
      name: 'tickets',
      url: 'https://tickets.example.com/api/mcp',
      auth_type: 'bearer',
    })
    bodies.stop()
  })

  it('shows a name clash from the server on the register form', async () => {
    renderApp('/mcp')
    await userEvent.click(await screen.findByRole('button', { name: copy.register }, T))
    const dialog = await screen.findByRole('dialog')
    await userEvent.type(within(dialog).getByLabelText(copy.fields.name.label), 'docs-search')
    await userEvent.type(
      within(dialog).getByLabelText(copy.fields.url.label),
      'https://x.example.com/mcp',
    )
    await userEvent.click(within(dialog).getByRole('button', { name: copy.registerTitle }))
    expect(
      await within(dialog).findByText(
        copy.registerFailed("conflict: you already have a connector named 'docs-search'"),
      ),
    ).toBeInTheDocument()
  })

  it('imports from GitHub: the new server shows as building', async () => {
    renderApp('/mcp')
    await userEvent.click(await screen.findByRole('button', { name: copy.upload }, T))
    await userEvent.click(await screen.findByRole('button', { name: new RegExp(copy.githubTitle) }))
    const dialog = await screen.findByRole('dialog')
    await userEvent.type(within(dialog).getByLabelText(copy.uploadName.label), 'repo-tools')
    await userEvent.type(
      within(dialog).getByLabelText(copy.githubUrl.label),
      'https://gitlab.com/x/y',
    )
    await userEvent.click(within(dialog).getByRole('button', { name: copy.uploadSubmit }))
    expect(
      await within(dialog).findByText(
        copy.uploadFailed('bad request: only https://github.com repositories can be cloned'),
      ),
    ).toBeInTheDocument()
    await userEvent.clear(within(dialog).getByLabelText(copy.githubUrl.label))
    await userEvent.type(
      within(dialog).getByLabelText(copy.githubUrl.label),
      'https://github.com/x/y',
    )
    await userEvent.click(within(dialog).getByRole('button', { name: copy.uploadSubmit }))
    expect(await screen.findByText(copy.uploadQueued('repo-tools'), {}, T)).toBeInTheDocument()
    const c = await card('repo-tools')
    expect(within(c).getByText(copy.building)).toBeInTheDocument()
    expect(mcpMockState().connectors.some((x) => x.name === 'repo-tools')).toBe(true)
  })
})
