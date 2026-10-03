/**
 * The app's one router factory: `mount()` and the page tests (common/src/test/renderApp.tsx) build the same router,
 * so the defaults below are what the tests exercise (plan §8 Phase 3). The route tree is the edition's
 * (`@edition` is the edition's src/: each edition's tsc program and build resolve it to its own).
 */
import type { QueryClient } from '@tanstack/react-query'
import { createRouter, type RouterHistory } from '@tanstack/react-router'
import { routeTree } from '@edition/routeTree.gen'
import { RouteError, RoutePending } from '@/app/shell/RouteStates'

export function createAppRouter({
  queryClient,
  history,
}: {
  queryClient: QueryClient
  history?: RouterHistory
}) {
  return createRouter({
    routeTree,
    history,
    context: { queryClient },
    // Hover/focus on a link loads the route chunk and runs its loader (prefetch) before the click.
    defaultPreload: 'intent',
    // The loader's prefetch goes to the Query cache; the router must not hold its own copy.
    defaultPreloadStaleTime: 0,
    // A pending UI only for a slow transition, so a fast one never flashes a spinner.
    defaultPendingMs: 150,
    defaultPendingComponent: RoutePending,
    defaultErrorComponent: RouteError,
    scrollRestoration: true,
  })
}

type AppRouter = ReturnType<typeof createAppRouter>

declare module '@tanstack/react-router' {
  interface Register {
    router: AppRouter
  }
}
