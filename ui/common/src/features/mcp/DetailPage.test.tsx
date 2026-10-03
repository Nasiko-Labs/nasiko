import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureMocks, mcpMockState, mockCtx } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { recordRequestBodies, server } from '@/test/setup'
import { renderApp } from '@/test/renderApp'
import { copy, STANCE } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 8000 }
const idOf = (name: string) => mcpMockState().connectors.find((c) => c.name === name)!.id
/** The agent the seed gives docs-search, weather-api and github access rows. */
const seededAgent = () => [...mcpMockState().access.keys()][0]!.split(':')[0]!
const panel = async (heading: string) =>
  (await screen.findByRole('heading', { name: heading }, T)).closest('section') as HTMLElement

describe('MCP server page (plans/feat-mcp.md §5)', () => {
  it('shows facts and tools, and a missing or invalid id as not found', async () => {
    renderApp(`/mcp/${idOf('docs-search')}`)
    expect(await screen.findByRole('heading', { name: /Docs search/ }, T)).toBeInTheDocument()
    const facts = await panel(copy.facts)
    expect(within(facts).getByText('https://docs-mcp.example.com/mcp')).toBeInTheDocument()
    expect(within(await panel(copy.toolsTitle(4))).getByText('summarize_page')).toBeInTheDocument()
    expect(within(await panel(copy.connection)).getByText(copy.noAuthNeeded)).toBeInTheDocument()
  })

  it('answers not found for an unknown or non-UUID id', async () => {
    renderApp('/mcp/5eed000e-0000-4000-8000-999999999999')
    expect(await screen.findByText(copy.notFound, {}, T)).toBeInTheDocument()
  })

  it('saves a credential, and says when it stored but failed to verify', async () => {
    renderApp(`/mcp/${idOf('weather-api')}`)
    const conn = await panel(copy.connection)
    expect(await within(conn).findByText(copy.credentialSet, {}, T)).toBeInTheDocument()
    await userEvent.type(within(conn).getByLabelText(copy.credentialLabel), 'invalid-key')
    await userEvent.click(within(conn).getByRole('button', { name: copy.replace }))
    expect(
      await within(conn).findByText(copy.verifyFailed('upstream returned 401 Unauthorized')),
    ).toBeInTheDocument()
    await userEvent.click(within(conn).getByRole('button', { name: copy.remove }))
    expect(await within(conn).findByText(copy.noCredential, {}, T)).toBeInTheDocument()
  })

  it('authorizes an OAuth server in a popup and shows it authorized', async () => {
    const popup = { closed: false }
    const open = vi.spyOn(window, 'open').mockReturnValue(popup as Window)
    renderApp(`/mcp/${idOf('jira-cloud')}`)
    const conn = await panel(copy.connection)
    await userEvent.click(await within(conn).findByRole('button', { name: copy.authorize }, T))
    await waitFor(() =>
      expect(open).toHaveBeenCalledWith(
        'about:blank#mcp-oauth',
        'mcp-oauth',
        'width=600,height=720',
      ),
    )
    popup.closed = true
    expect(
      await within(conn).findByText(new RegExp(`^${copy.authorized}`), {}, T),
    ).toBeInTheDocument()
  })

  it('refuses to open a non-https sign-in page', async () => {
    const open = vi.spyOn(window, 'open')
    renderApp(`/mcp/${idOf('jira-cloud')}`)
    const conn = await panel(copy.connection)
    server.use(
      http.post('/api/mcp/connectors/:id/oauth/authorize', () =>
        HttpResponse.json({
          data: { authorization_url: 'http://evil.example.com' },
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    await userEvent.click(await within(conn).findByRole('button', { name: copy.authorize }, T))
    expect(await screen.findByText(copy.unsafeUrl, {}, T)).toBeInTheDocument()
    expect(open).not.toHaveBeenCalled()
  })

  it("manages a picked agent's access and tool rules on the Agents tab", async () => {
    const bodies = recordRequestBodies()
    const agent = seededAgent()
    renderApp(`/mcp/${idOf('docs-search')}?tab=agents&agent=${agent}`)
    // Owners see who already uses it.
    const users = await panel(copy.consumersTitle)
    expect((await within(users).findAllByRole('link', {}, T)).length).toBeGreaterThan(0)
    const row = await screen.findByRole('radiogroup', { name: copy.stanceFor('summarize_page') }, T)
    expect(within(row).getByRole('radio', { name: STANCE.ask.label })).toHaveAttribute(
      'aria-checked',
      'true',
    )
    await userEvent.click(within(row).getByRole('radio', { name: STANCE.block.label }))
    await waitFor(() =>
      expect(within(row).getByRole('radio', { name: STANCE.block.label })).toHaveAttribute(
        'aria-checked',
        'true',
      ),
    )
    await bodies.flush()
    const put = bodies.requests.find((r) => r.method === 'PUT' && r.url.pathname.endsWith('/tools'))
    // The whole rule set for this connector, with the one change.
    expect((put?.body as { rules: unknown[] }).rules).toEqual(
      expect.arrayContaining([
        { connector_id: idOf('docs-search'), tool_pattern: 'summarize_page', stance: 'block' },
        { connector_id: idOf('docs-search'), tool_pattern: 'get_page', stance: 'allow' },
      ]),
    )
    expect((put?.body as { rules: unknown[] }).rules).toHaveLength(4)
    bodies.stop()
  })

  it('says to connect first when the picked agent cannot use an unconnected server', async () => {
    renderApp(`/mcp/${idOf('jira-cloud')}?tab=agents&agent=${seededAgent()}`)
    expect(await screen.findByText(copy.notConnectedNote, {}, T)).toBeInTheDocument()
  })

  it('lists who has access, grants a user and revokes a direct grant', async () => {
    renderApp(`/mcp/${idOf('docs-search')}?tab=access`)
    const grants = await panel(copy.grantsTitle)
    expect(await within(grants).findByText(copy.grantKind.owner, {}, T)).toBeInTheDocument()
    expect(within(grants).getByText(copy.grantKind.direct)).toBeInTheDocument()
    const before = mcpMockState().connectors.find((c) => c.name === 'docs-search')!.userGrants
      .length
    await userEvent.type(within(grants).getByLabelText(copy.grantTitle), 'o')
    expect(within(grants).getByText(copy.typeMore(2))).toBeInTheDocument()
    const candidate = mcpMockState().connectors.find((c) => c.name === 'docs-search')!
    const users = mockCtx.agents().users
    const target = users.find(
      (u) =>
        !u.service_account &&
        u.id !== candidate.owner_id &&
        !candidate.userGrants.some((g) => g.id === u.id),
    )!
    await userEvent.clear(within(grants).getByLabelText(copy.grantTitle))
    await userEvent.type(within(grants).getByLabelText(copy.grantTitle), target.username)
    await userEvent.click(
      await within(grants).findByRole(
        'button',
        { name: copy.grantNamed(target.display_name || target.username) },
        T,
      ),
    )
    expect(
      await screen.findByText(copy.granted(target.display_name || target.username), {}, T),
    ).toBeInTheDocument()
    expect(candidate.userGrants).toHaveLength(before + 1)

    const revoke = await within(grants).findAllByRole('button', { name: /^Revoke access for/ }, T)
    await userEvent.click(revoke[0]!)
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: copy.revoke }))
    expect(await screen.findByText(copy.revoked, {}, T)).toBeInTheDocument()
    await waitFor(() => expect(candidate.userGrants).toHaveLength(before))
  })

  it('makes a server public: the header says so and everyone shows as Public', async () => {
    renderApp(`/mcp/${idOf('docs-search')}?tab=access`)
    await userEvent.click(await screen.findByLabelText(copy.publicToggle, {}, T))
    await waitFor(() =>
      expect(mcpMockState().connectors.find((c) => c.name === 'docs-search')!.public).toBe(true),
    )
    const grants = await panel(copy.grantsTitle)
    expect(
      (await within(grants).findAllByText(copy.grantKind.public, {}, T)).length,
    ).toBeGreaterThan(0)
    const header = screen.getByRole('heading', { level: 1 }).closest('header') ?? document.body
    expect(within(header as HTMLElement).getAllByText(copy.publicChip).length).toBeGreaterThan(0)
  })

  it("shows a failed build's error and its log lines", async () => {
    renderApp(`/mcp/${idOf('sql-runner')}?tab=logs`)
    const logs = await panel(copy.logsTitle)
    expect(await within(logs).findByText(/Could not find a version/, {}, T)).toBeInTheDocument()
    expect(within(logs).getByText(/^Build error: docker build failed/)).toBeInTheDocument()
  })

  it('saves only the changed settings', async () => {
    const bodies = recordRequestBodies()
    renderApp(`/mcp/${idOf('weather-api')}?tab=settings`)
    const name = await screen.findByLabelText(copy.fields.display_name.label, {}, T)
    await userEvent.clear(name)
    await userEvent.type(name, 'Weather service')
    await userEvent.click(screen.getByRole('button', { name: copy.saveChanges }))
    expect(await screen.findByText(copy.saved, {}, T)).toBeInTheDocument()
    await bodies.flush()
    expect(bodies.requests.find((r) => r.method === 'PATCH')?.body).toEqual({
      display_name: 'Weather service',
    })
    bodies.stop()
  })

  it("hides the owner's tabs on a server shared with a non-admin", async () => {
    configureMocks({ superuser: false })
    renderApp(`/mcp/${idOf('finance-ledger')}?tab=access`)
    expect(await screen.findByRole('tab', { name: copy.tabsNames.agents }, T)).toBeInTheDocument()
    expect(screen.queryByRole('tab', { name: copy.tabsNames.access })).toBeNull()
    expect(screen.queryByRole('tab', { name: copy.tabsNames.settings })).toBeNull()
    // The hidden tab fell back to Overview.
    expect(await panel(copy.facts)).toBeInTheDocument()
  })

  it('deletes from the danger zone and goes back to the catalog', async () => {
    const { router } = renderApp(`/mcp/${idOf('weather-api')}`)
    await userEvent.click(await screen.findByRole('button', { name: copy.delete }, T))
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: copy.delete }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/mcp'), T)
    expect(mcpMockState().connectors.some((c) => c.name === 'weather-api')).toBe(false)
  })
})
