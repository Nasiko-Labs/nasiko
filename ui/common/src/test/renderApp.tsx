/**
 * Render the whole app (router + app QueryClient) at a URL, against the MSW handlers.
 * Shared by the /qa regression tests.
 */
import { QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory, RouterProvider, type AnyRouter } from '@tanstack/react-router'
import { render } from '@testing-library/react'
import { LayoutGroup, LazyMotion, domMax } from 'motion/react'
import { EditionProvider } from '@/app/EditionProvider'
import { createAppRouter } from '@/app/router'
import { edition } from '@edition/edition'
import { TooltipProvider } from '@/components/ui/tooltip'
import { createQueryClient } from '@/lib/queryClient'

export function renderApp(url: string) {
  // A holder, since the query client is built before the router it redirects with.
  const routerRef: { current?: AnyRouter } = {}
  const queryClient = createQueryClient(() => routerRef.current, { retry: false })
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
