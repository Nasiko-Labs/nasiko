import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, mcpMockState } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { server } from '@/test/setup'
import { renderApp } from '@/test/renderApp'
import { copy, STANCE } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 8000 }
const seededAgent = () => [...mcpMockState().access.keys()][0]!.split(':')[0]!
const docsId = () => mcpMockState().connectors.find((c) => c.name === 'docs-search')!.id

describe("Agent detail's MCP tab (plans/feat-mcp.md §5.2)", () => {
  it('lists the servers the agent may use with their rule summaries', async () => {
    renderApp(`/agents/${seededAgent()}?tab=mcp`)
    expect(await screen.findByText(copy.summary(3, 4, 1), {}, T)).toBeInTheDocument()
    // GitHub has delete_repo blocked.
    expect(await screen.findByText(copy.summary(3, 4, 0), {}, T)).toBeInTheDocument()
    // Jira isn't connected by the viewer, so this agent can't be offered it.
    expect(screen.queryByText('Jira Cloud')).toBeNull()
  })

  it('greys out uploads that are building or failed: status, disabled switch, a link to the server', async () => {
    renderApp(`/agents/${seededAgent()}?tab=mcp`)
    const failed = await screen.findByRole('switch', { name: copy.enable('SQL runner') }, T)
    expect(failed).toBeDisabled()
    expect(screen.getByRole('switch', { name: copy.enable('CRM sync') })).toBeDisabled()
    expect(screen.getByText(copy.notReadyFailed, { exact: false })).toBeInTheDocument()
    expect(screen.getByText(copy.notReadyBuilding, { exact: false })).toBeInTheDocument()
    const sql = mcpMockState().connectors.find((c) => c.name === 'sql-runner')!.id
    expect(screen.getByRole('link', { name: copy.viewLogs })).toHaveAttribute(
      'href',
      `/mcp/${sql}?tab=logs`,
    )
    // No expander: there are no tools to set yet.
    expect(screen.queryByRole('button', { name: copy.expand('SQL runner') })).toBeNull()
  })

  it('switches a server off for the agent at once', async () => {
    renderApp(`/agents/${seededAgent()}?tab=mcp`)
    const toggle = await screen.findByRole('switch', { name: copy.disable('Docs search') }, T)
    await userEvent.click(toggle)
    expect(
      await screen.findByRole('switch', { name: copy.enable('Docs search') }, T),
    ).toBeInTheDocument()
    await waitFor(() =>
      expect(mcpMockState().access.get(`${seededAgent()}:${docsId()}`)?.enabled).toBe(false),
    )
  })

  it('rolls a stance back when the save fails', async () => {
    server.use(
      http.put('/api/mcp/agents/:id/tools', () =>
        HttpResponse.json(
          { data: null, status_code: 403, message: 'forbidden: nope' },
          { status: 403 },
        ),
      ),
    )
    renderApp(`/agents/${seededAgent()}?tab=mcp`)
    await userEvent.click(
      await screen.findByRole('button', { name: copy.expand('Docs search') }, T),
    )
    const group = await screen.findByRole('radiogroup', { name: copy.stanceFor('get_page') }, T)
    await userEvent.click(within(group).getByRole('radio', { name: STANCE.block.label }))
    expect(await screen.findByText(copy.ruleFailed('forbidden: nope'), {}, T)).toBeInTheDocument()
    await waitFor(() =>
      expect(within(group).getByRole('radio', { name: STANCE.allow.label })).toHaveAttribute(
        'aria-checked',
        'true',
      ),
    )
  })

  it('points to the catalog when no server is available', async () => {
    const other = mcpMockState()
    other.connections.clear()
    for (const c of other.connectors) c.auth_type = c.auth_type === 'none' ? 'bearer' : c.auth_type
    renderApp(`/agents/${seededAgent()}?tab=mcp`)
    expect(await screen.findByText(copy.noAgentServers, {}, T)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: copy.openCatalog })).toHaveAttribute('href', '/mcp')
  })
})
