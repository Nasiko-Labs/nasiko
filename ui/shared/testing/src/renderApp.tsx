/**
 * Render the whole app (router + app QueryClient) at a URL, against the MSW handlers.
 * Shared by the /qa regression tests.
 */
import { QueryClientProvider } from '@tanstack/react-query'
import {
  createMemoryHistory,
  RouterProvider,
  type AnyRoute,
  type AnyRouter,
} from '@tanstack/react-router'
import { render } from '@testing-library/react'
import { LayoutGroup, LazyMotion, domMax } from 'motion/react'
import { EditionProvider } from '@nasiko/app-core/edition/EditionProvider'
import { createAppRouter } from '@nasiko/app-shell/router'
import { edition } from '@edition/edition'
import { TooltipProvider } from '@nasiko/ui/primitives/tooltip'
import { createQueryClient } from '@nasiko/app-core/queryClient'
import { trackQueryClient } from './queryClients'

export function renderApp(url: string) {
  // A holder, since the query client is built before the router it redirects with.
  const routerRef: { current?: AnyRouter } = {}
  const queryClient = trackQueryClient(createQueryClient(() => routerRef.current, { retry: false }))
  const router = createAppRouter({
    queryClient,
    history: createMemoryHistory({ initialEntries: [url] }),
  })
  routerRef.current = router as unknown as AnyRouter
  render(
    <EditionProvider edition={edition}>
      <QueryClientProvider client={queryClient}>
        <TooltipProvider>
          <LazyMotion features={domMax}>
            <LayoutGroup>
              <RouterProvider router={router} />
            </LayoutGroup>
          </LazyMotion>
        </TooltipProvider>
      </QueryClientProvider>
    </EditionProvider>,
  )
  return { router, queryClient }
}

/**
 * How long a file's warm-up may take: it pays the first import of every page it names (the React Compiler's Babel
 * pass included), which is the cost a cold first render would otherwise pay inside one test's findBy wait.
 */
export const WARM_ROUTES_TIMEOUT_MS = process.env.CI === 'true' ? 120_000 : 60_000

/**
 * Load the route modules behind these URLs before a file's tests run, so the first `renderApp` of the file renders
 * as fast as the rest instead of spending its findBy wait on module transforms (a parallel run on a busy machine took
 * over 12 s there). Call it from `beforeAll` with `WARM_ROUTES_TIMEOUT_MS`; it renders nothing and reads no data.
 */
export async function warmRoutes(urls: readonly string[]): Promise<void> {
  const router = createAppRouter({
    queryClient: createQueryClient(() => undefined, { retry: false }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  const routes = new Set<AnyRoute>()
  for (const url of urls) {
    for (const route of router.getMatchedRoutes(new URL(url, 'http://localhost').pathname)[0])
      routes.add(route)
  }
  await Promise.all([...routes].map((route) => router.loadRouteChunk(route) ?? Promise.resolve()))
}
