/**
 * The Agent and key step's Starter pack follows the edition's `starterPack` slot (user request 2026-11-12): no slot, the
 * tab is offered (OnboardingDialog.test.tsx covers that path end to end); a slot that says no drops the tab and opens on
 * the Registry tab; a slot still answering holds the tabs back, so the pack never flashes in and out.
 */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, within } from '@testing-library/react'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { describe, expect, it } from 'vitest'
import { EditionProvider } from '@nasiko/app-core/edition/EditionProvider'
import { starterPack, type Edition } from '@nasiko/app-core/edition/edition'
import { meQuery } from '@nasiko/data-access/auth'
import { AgentStep } from './AgentStep'
import { copy } from './copy'

/** An edition whose one layer answers the Starter pack with `answer` (a never-settling promise keeps it pending). */
const editionAnswering = (answer: () => Promise<boolean>, note?: string): Edition => ({
  id: 'test',
  layers: [
    {
      id: 'test',
      slots: {
        starterPack: starterPack({
          query: { queryKey: ['test', 'starter-pack'], queryFn: answer },
          offered: (free) => free !== false,
          note,
        }),
      },
    },
  ],
})

function renderStep(edition: Edition) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } })
  qc.setQueryData(meQuery.queryKey, { sub: 'u1', username: 'ada', is_superuser: false } as never)
  const rootRoute = createRootRoute({
    component: () => (
      <AgentStep started={null} onStarted={() => undefined} onLeave={() => undefined} />
    ),
  })
  const router = createRouter({
    routeTree: rootRoute,
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  render(
    <EditionProvider edition={edition}>
      <QueryClientProvider client={qc}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </EditionProvider>,
  )
}

describe('AgentStep Starter pack', () => {
  it('drops the Starter pack on an upgraded workspace and opens on the Registry tab', async () => {
    renderStep(editionAnswering(() => Promise.resolve(false)))
    expect(await screen.findByRole('tab', { name: 'Registry' })).toHaveAttribute(
      'aria-selected',
      'true',
    )
    expect(screen.queryByRole('tab', { name: copy.agent.seeded })).toBeNull()
    expect(screen.queryByRole('region', { name: copy.agent.seededLabel })).toBeNull()
    expect(screen.getByText(copy.agent.introOwn)).toBeInTheDocument()
  })

  it('offers it on the Free plan, first', async () => {
    renderStep(editionAnswering(() => Promise.resolve(true)))
    expect(await screen.findByRole('tab', { name: copy.agent.seeded })).toHaveAttribute(
      'aria-selected',
      'true',
    )
    expect(screen.getByRole('region', { name: copy.agent.seededLabel })).toBeInTheDocument()
    expect(screen.getByText(copy.agent.intro)).toBeInTheDocument()
    // No layer note, and the Deploy page's key notice stays out of the guide (its next step takes the key).
    expect(screen.queryByRole('note')).toBeNull()
  })

  it("heads the pack with the layer's note", async () => {
    renderStep(editionAnswering(() => Promise.resolve(true), 'Up to 50 model calls a day.'))
    const pack = await screen.findByRole('region', { name: copy.agent.seededLabel })
    expect(within(pack).getByRole('note')).toHaveTextContent('Up to 50 model calls a day.')
  })

  it('holds the tabs back until the plan is known', async () => {
    renderStep(editionAnswering(() => new Promise<boolean>(() => undefined)))
    expect(await screen.findByText(copy.agent.intro)).toBeInTheDocument()
    expect(screen.queryByRole('tab')).toBeNull()
  })
})
