/**
 * Render the whole app (router + app QueryClient) at a URL, against the MSW handlers.
 * Shared by the /qa regression tests.
 */
import { QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory, RouterProvider, type AnyRouter } from '@tanstack/react-router'
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
