/** Deploy's entry points (plans/feat-deploy.md §7): every CLI-only state gains Deploy an agent, and keeps its CLI steps. */
import { cleanup, screen, within } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { configureMocks } from '@/mocks/handlers'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { copy as agentsCopy } from '@/features/agents/copy'
import { copy } from './copy'

setupPinnedSeed()

const deployLinks = async () =>
  screen.findAllByRole('link', { name: copy.entry.label }, { timeout: 5000 })

describe('Deploy an agent entry points', () => {
  it(
    'Overview first run and the empty Agents pages offer it beside the CLI steps',
    { timeout: 15_000 },
    async () => {
      server.use(http.get('*/api/agents', () => HttpResponse.json([])))
      // A server without the onboarding guide: its Setup guide card replaces this deploy card otherwise.
      configureMocks({ variant: 'onboarding-absent' })
      for (const [path, testId, empty] of [
        ['/', 'overview-first-run', null],
        ['/agents', null, agentsCopy.noAgentsCatalog],
        ['/agents/mine', null, agentsCopy.noAgentsMine],
      ] as const) {
        renderApp(path)
        if (testId) await screen.findByTestId(testId, {}, { timeout: 5000 })
        if (empty) await screen.findByText(empty, {}, { timeout: 5000 })
        const links = await deployLinks()
        expect(links.every((l) => l.getAttribute('href') === '/deploy')).toBe(true)
        const scope = testId ? within(screen.getByTestId(testId)) : screen
        expect(scope.getByText(copy.entry.orCli)).toBeInTheDocument()
        expect(scope.getAllByRole('button', { name: /Copy/ }).length).toBeGreaterThan(0)
        cleanup()
      }
      configureMocks({ variant: null })
    },
  )

  it('Quick actions and the Agents headers link to /deploy', async () => {
    renderApp('/')
    expect(
      within(await screen.findByTestId('overview-actions', {}, { timeout: 5000 })).getByRole(
        'link',
        { name: copy.entry.label },
      ),
    ).toHaveAttribute('href', '/deploy')
  })

  it('a never-deployed agent pre-fills its name', async () => {
    // Seed agent 11 is registered, never deployed (src/mocks/agents.ts).
    const agent = seed.agents[11]!
    renderApp(`/agents/${agent.id}`)
    const links = await deployLinks()
    expect(links.map((l) => l.getAttribute('href'))).toContain(`/deploy?name=${agent.name}`)
  })
})
