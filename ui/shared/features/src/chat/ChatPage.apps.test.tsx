/**
 * The apps an agent works with, where its chat starts (components/AgentApps.tsx): each app the viewer can reach,
 * connected or with Connect, which connects in place and refreshes the row.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { copy as mcpCopy } from '@nasiko/features/mcp/copy'
import { appsToConnectIn } from './agentApps'
import { copy } from './copy'
import { clearDrafts } from './drafts'
import { clearChatRegistry } from './registry'

afterEach(() => {
  clearChatRegistry()
  clearDrafts()
})

const T = { timeout: 8000 }

async function runningAgent(): Promise<{ id: string }> {
  for (let offset = 0; offset < 1000; offset += 100) {
    const rows = (await (
      await fetch(new URL(`/api/agents?limit=100&offset=${offset}`, location.origin))
    ).json()) as { id: string; status: string; tags: string[] }[]
    const hit = rows.find((a) => a.status === 'running' && !a.tags.includes('coding-agent'))
    if (hit) return hit
    if (rows.length < 100) break
  }
  throw new Error('no running agent in the mock seed')
}

const row = (id: string, name: string, display: string, status: string) => ({
  connector_id: id,
  provider_type: 'composio',
  name,
  display_name: display,
  logo_url: null,
  status,
})

/** The viewer's standing on the agent's apps; Gmail turns connected once the connect call lands. */
function connections() {
  let gmail = 'needs_connection'
  const connected: string[] = []
  server.use(
    http.get('/api/mcp/agents/:id/my-connections', () =>
      HttpResponse.json({
        data: {
          connectors: [
            row('c-gmail', 'gmail', 'Gmail', gmail),
            row('c-slack', 'slack', 'Slack', 'connected'),
            row('c-private', 'private', 'Private', 'not_shared'),
          ],
        },
        status_code: 200,
        message: 'ok',
      }),
    ),
    http.post('/api/mcp/connect', async ({ request }) => {
      connected.push(((await request.json()) as { connector_id: string }).connector_id)
      gmail = 'connected'
      return HttpResponse.json({ data: { status: 'connected' }, status_code: 200, message: 'ok' })
    }),
  )
  return connected
}

describe('new chat: the apps the agent works with', () => {
  it('shows each reachable app, connected or with Connect, and connects in place', async () => {
    const connected = connections()
    const agent = await runningAgent()
    renderApp(`/chat?agent=${agent.id}`)
    const apps = await screen.findByTestId('agent-apps', {}, T)
    expect(within(apps).getByText(copy.worksWith)).toBeInTheDocument()
    expect(within(apps).getByLabelText(copy.appConnected('Slack'))).toBeInTheDocument()
    expect(within(apps).queryByText('Private')).toBeNull()

    await userEvent.click(within(apps).getByRole('button', { name: mcpCopy.connectNamed('Gmail') }))
    expect(connected).toEqual(['c-gmail'])
    expect(
      await within(apps).findByLabelText(copy.appConnected('Gmail'), {}, T),
    ).toBeInTheDocument()
  })

  it('leads with the connected apps, and "+N more" opens every toolkit on MCP servers', async () => {
    const names = ['Airtable', 'Asana', 'Discord', 'Dropbox', 'Gmail', 'Jira', 'Notion', 'Slack']
    server.use(
      http.get('/api/mcp/agents/:id/my-connections', () =>
        HttpResponse.json({
          data: {
            connectors: names.map((n) =>
              row(
                `c-${n}`,
                n.toLowerCase(),
                n,
                n === 'Slack' || n === 'Notion' ? 'connected' : 'needs_connection',
              ),
            ),
          },
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    const agent = await runningAgent()
    const { router } = renderApp(`/chat?agent=${agent.id}`)
    const apps = await screen.findByTestId('agent-apps', {}, T)
    const chips = within(apps)
      .getAllByRole('listitem')
      .map((li) => li.textContent ?? '')
    // The two connected apps lead, ahead of the alphabetically earlier ones still to connect.
    expect(chips[1]).toContain('Notion')
    expect(chips[2]).toContain('Slack')
    const more = within(apps).getByRole('link', { name: copy.moreAppsLabel(names.length - 5) })
    expect(more).toHaveTextContent(copy.moreApps(names.length - 5))
    await userEvent.click(more)
    await waitFor(() => expect(router.state.location.pathname).toBe('/mcp'))
    expect(router.state.location.search).toMatchObject({ view: 'toolkits' })
  })

  it('shows nothing for an agent with no apps', async () => {
    server.use(
      http.get('/api/mcp/agents/:id/my-connections', () =>
        HttpResponse.json({ data: { connectors: [] }, status_code: 200, message: 'ok' }),
      ),
    )
    const agent = await runningAgent()
    renderApp(`/chat?agent=${agent.id}`)
    await screen.findByTestId('new-chat-hero', {}, T)
    expect(screen.queryByTestId('agent-apps')).toBeNull()
  })

  it("a reply's tip names the app to connect", () => {
    const tip =
      '**Try it on your own Gmail:** send "Summarize my unread emails from today in Gmail".'
    const apps = [
      {
        id: 'g',
        name: 'gmail',
        label: 'Gmail',
        logoUrl: null,
        authType: null,
        status: 'needs_connection' as const,
      },
    ]
    expect(appsToConnectIn(tip, apps)).toHaveLength(1)
  })
})
