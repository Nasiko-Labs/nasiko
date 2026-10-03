import { createFileRoute } from '@tanstack/react-router'
import { prefetchRouter } from '@/features/router/api'
import { RouterPage } from '@/features/router/RouterPage'
import { routerSearchSchema, type RouterSearch } from '@/features/router/search'
import { useSetSearch } from '@/lib/search'

export const Route = createFileRoute('/_app/router')({
  validateSearch: routerSearchSchema,
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page (plan §8 Phase 3).
  loader: ({ context, preload }) => {
    if (preload) prefetchRouter(context.queryClient)
  },
  component: RouterRoute,
})

function RouterRoute() {
  // The source filter replaces history, like the catalog's filters.
  const setSearch = useSetSearch<RouterSearch>(Route.fullPath, true)
  return <RouterPage search={Route.useSearch()} setSearch={setSearch} />
}
