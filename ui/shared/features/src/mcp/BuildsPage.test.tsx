import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, mcpMockState } from '@nasiko/mocks/handlers'
import { ADMIN_ID } from '@nasiko/mocks/seed-harness'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 8000 }

/** Pushes an upload whose connector is already gone (first-time build failure, mock equivalent of
 * `delete_mcp_connector_or_mark_failed` + the surviving `mcp_upload_status` row). */
function pushGoneUpload(name: string, error: string) {
  mcpMockState().uploadHistory.push({
    uploadId: `b-${name}`,
    connectorId: `c-${name}`,
    connectorName: name,
    ownerId: ADMIN_ID,
    createdAt: new Date(now() - 60_000).toISOString(),
    source: 'zip',
    build: {
      id: `b-${name}`,
      version: 'v1',
      startedAt: now() - 60_000,
      doneAt: now() - 50_000,
      fails: true,
      prior: null,
      error,
      tools: [],
    },
  })
}

describe('MCP Builds (a dedicated section, mirroring the Agents Builds page)', () => {
  it('a first-time upload whose only build failed stays trackable after its connector is gone', async () => {
    // Set up before render so the first fetch already reflects it, same as other mock-state tests.
    pushGoneUpload('broken-server', 'zip contains no root Dockerfile')
    renderApp('/mcp/builds')
    const row = (await screen.findByText('broken-server', {}, T)).closest(
      '[data-testid="mcp-build-row"]',
    ) as HTMLElement
    expect(within(row).getByText('zip contains no root Dockerfile')).toBeInTheDocument()
    expect(within(row).getByText(copy.buildStatus.failed)).toBeInTheDocument()
    // No link: the connector itself never existed by the time this test set up.
    expect(within(row).queryByRole('link')).toBeNull()
  })

  it('a successful upload later deleted on purpose reads "Deleted connector", never the failed-upload hint, badge still Succeeded', async () => {
    // Unlike the two tests above, `fails: false` and `doneAt` already in the past: this build
    // genuinely succeeded. `connectorId` names a connector that's simply never added to
    // `st.connectors` — the mock's own DELETE handler produces exactly this (it only removes
    // the connector; `my-uploads` resolves `connector_id` with a live lookup each time, so a
    // missing connector reads identically whether it was never added or deleted afterwards).
    mcpMockState().uploadHistory.push({
      uploadId: 'b-deleted-after-success',
      connectorId: 'c-deleted-after-success',
      connectorName: 'convert-mcp-after-fix',
      ownerId: ADMIN_ID,
      createdAt: new Date(now() - 120_000).toISOString(),
      source: 'zip',
      build: {
        id: 'b-deleted-after-success',
        version: 'v1',
        startedAt: now() - 120_000,
        doneAt: now() - 110_000,
        fails: false,
        prior: null,
        error: '',
        tools: [],
      },
    })
    renderApp('/mcp/builds')
    // The original name never shows — "Deleted connector" replaces it entirely, exactly like the
    // Agents Builds page shows "Deleted agent" rather than risk reading as still-present.
    const row = (await screen.findByText(copy.builds.deletedConnectorName, {}, T)).closest(
      '[data-testid="mcp-build-row"]',
    ) as HTMLElement
    expect(within(row).queryByText('convert-mcp-after-fix')).toBeNull()
    // The build's own outcome never changes meaning: it really did succeed.
    expect(within(row).getByText(copy.buildStatus.success)).toBeInTheDocument()
    // No secondary hint line — "Deleted connector" alone already says it.
    expect(within(row).queryByText(copy.builds.deletedConnector)).toBeNull()
    expect(within(row).queryByRole('link')).toBeNull()
  })

  it('links to the connector page for an upload whose connector survives (a kept re-upload failure)', async () => {
    // A custom MCP-server connector specifically (not a Composio toolkit, which has no detail page).
    const live = mcpMockState().connectors.find((c) => c.provider === 'mcp_server')!
    mcpMockState().uploadHistory.push({
      uploadId: 'b-kept',
      connectorId: live.id,
      connectorName: live.name,
      ownerId: ADMIN_ID,
      createdAt: new Date(now() - 5_000).toISOString(),
      source: 'zip',
      build: {
        id: 'b-kept',
        version: 'v1',
        startedAt: now() - 5_000,
        doneAt: now() + 10_000,
        fails: false,
        prior: null,
        error: '',
        tools: [],
      },
    })
    renderApp('/mcp/builds')
    expect(await screen.findByRole('link', { name: live.name }, T)).toHaveAttribute(
      'href',
      expect.stringContaining(`/mcp/${live.id}`),
    )
  })

  it('filters by status and searches by name', async () => {
    pushGoneUpload('alpha-broken', 'no root Dockerfile')
    renderApp('/mcp/builds')
    await screen.findByText('alpha-broken', {}, T)
    await userEvent.click(screen.getByRole('radio', { name: copy.buildFilter.failed }))
    expect(await screen.findByText('alpha-broken')).toBeInTheDocument()
    await userEvent.type(
      screen.getByRole('searchbox', { name: copy.builds.searchLabel }),
      'zzz-no-match',
    )
    expect(await screen.findByText(copy.builds.noMatch)).toBeInTheDocument()
  })

  it('the catalog page and the builds page link to each other', async () => {
    renderApp('/mcp')
    const nav = await screen.findByRole('navigation', { name: copy.sections.label }, T)
    await userEvent.click(within(nav).getByRole('link', { name: copy.sections.builds }))
    await screen.findByText(copy.builds.description, {}, T)
    expect(
      within(screen.getByRole('navigation', { name: copy.sections.label })).getByRole('link', {
        name: copy.title,
      }),
    ).toBeInTheDocument()
  })
})
