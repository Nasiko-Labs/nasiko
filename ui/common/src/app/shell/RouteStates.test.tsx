import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/lib/api/client'
import { RouteError } from './RouteStates'

function renderThrowing(fail: () => unknown) {
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <p>{String(fail())}</p> }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
    defaultErrorComponent: RouteError,
  })
  render(
    <QueryClientProvider client={new QueryClient()}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  )
}

describe('RouteError (the router default)', () => {
  it('turns a render crash into a card with Try again, which re-renders the route', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    let broken = true
    renderThrowing(() => {
      if (broken) throw new Error('boom')
      return 'fine'
    })
    expect(await screen.findByText("This page couldn't load")).toBeInTheDocument()
    broken = false
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(await screen.findByText('fine')).toBeInTheDocument()
    vi.restoreAllMocks()
  })

  it('renders a 403 as No access, with nothing to retry', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    renderThrowing(() => {
      throw new ApiError(403, 'requires admin role', '/api/users', 'm')
    })
    expect(await screen.findByText("You don't have access to this page")).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull()
    vi.restoreAllMocks()
  })
})
