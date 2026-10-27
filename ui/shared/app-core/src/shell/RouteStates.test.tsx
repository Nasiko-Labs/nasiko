import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@nasiko/data-access/client'
import { resetStaleBuildForTests } from '@nasiko/app-core/staleBuild'
import { server } from '@nasiko/testing/setup'
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

  describe('a chunk that would not load (lib/staleBuild.ts)', () => {
    afterEach(() => {
      resetStaleBuildForTests()
      document.head.innerHTML = ''
    })

    it('checks the build first: the same build means an ordinary failure, not "updated", and no reload', async () => {
      vi.spyOn(console, 'error').mockImplementation(() => {})
      document.head.innerHTML = '<script type="module" src="/assets/index-same.js"></script>'
      server.use(
        http.get(
          `${location.origin}/`,
          () =>
            new HttpResponse('<script type="module" src="/assets/index-same.js"></script>', {
              headers: { 'content-type': 'text/html' },
            }),
        ),
      )
      renderThrowing(() => {
        throw new TypeError('Failed to fetch dynamically imported module: /assets/page-old.js')
      })
      expect(await screen.findByText('Try again in a moment.')).toBeInTheDocument()
      expect(screen.queryByText(/OpenRuntime was updated/)).toBeNull()
      expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument()
      vi.restoreAllMocks()
    })
  })
})
