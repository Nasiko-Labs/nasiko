import { screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, deployMockState } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

describe("An agent's Builds tab (plans/feat-deploy.md §6, design review 1)", () => {
  it("lists that agent's builds, newest first, each opening its Build page", async () => {
    const agentId = deployMockState().state.builds[0]!.record.agent_id
    const own = deployMockState().state.builds.filter((b) => b.record.agent_id === agentId)
    renderApp(`/agents/${agentId}?tab=builds`)
    const table = await screen.findByRole(
      'table',
      { name: copy.agentBuilds.label },
      { timeout: 5000 },
    )
    const rows = within(table).getAllByTestId('agent-build-row')
    expect(rows.length).toBe(Math.min(own.length, 20))
    for (const r of rows)
      expect(within(r).getAllByRole('link')[0]!.getAttribute('href')).toMatch(/^\/builds\/5eed/)
    expect(
      screen.getByRole('link', { name: copy.agentBuilds.deploy }).getAttribute('href'),
    ).toMatch(/^\/deploy\?name=/)
  })

  it('says why an agent has no builds', async () => {
    const withBuilds = new Set(deployMockState().state.builds.map((b) => b.record.agent_id))
    const bare = seed.agents.find((a) => !withBuilds.has(a.id))!
    renderApp(`/agents/${bare.id}?tab=builds`)
    expect(
      await screen.findByText(copy.agentBuilds.empty, {}, { timeout: 5000 }),
    ).toBeInTheDocument()
  })

  it('says so when the server withholds builds', async () => {
    configureMocks({ variant: 'deploy-no-rights' })
    renderApp(`/agents/${seed.agents[0]!.id}?tab=builds`)
    expect(await screen.findByText(copy.builds.noRights, {}, { timeout: 5000 })).toBeInTheDocument()
  })
})
