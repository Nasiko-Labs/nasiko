import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { act, render, waitFor } from '@testing-library/react'
import { expect, it } from 'vitest'
import { useSetSearch } from './search'

it('useSetSearch keeps the sibling list across a search change and drops other state', async () => {
  let set!: ReturnType<typeof useSetSearch<{ tab: string }>>
  function Probe() {
    set = useSetSearch<{ tab: string }>('/')
    return null
  }
  const router = createRouter({
    routeTree: createRootRoute({ component: Probe }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  render(<RouterProvider router={router} />)
  await waitFor(() => expect(set).toBeDefined())
  await act(() =>
    router.navigate({
      to: '/',
      // agentDeleted: an agents HistoryState key (features/agents/search.ts)
      state: { siblings: ['a', 'b'], agentDeleted: { name: 'x', errors: [], stopped: 0 } },
    }),
  )
  expect(router.state.location.state.siblings).toEqual(['a', 'b'])
  await act(async () => set({ tab: 'x' }))
  expect(router.state.location.state.siblings).toEqual(['a', 'b'])
  expect(router.state.location.state).not.toHaveProperty('agentDeleted')
})
