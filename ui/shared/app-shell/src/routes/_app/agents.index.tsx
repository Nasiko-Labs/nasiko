import { createFileRoute } from '@tanstack/react-router'
import { CatalogPage } from '@nasiko/features/agents/CatalogPage'
import { prefetchCatalog } from '@nasiko/features/agents/prefetch'
import { catalogSearchSchema, type CatalogSearch } from '@nasiko/features/agents/search'
import { useSetSearch } from '@nasiko/ui/lib/search'

export const Route = createFileRoute('/_app/agents/')({
  validateSearch: catalogSearchSchema,
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page (plan §8 Phase 3).
  loader: ({ context, preload }) => {
    if (preload) prefetchCatalog(context.queryClient)
  },
  component: CatalogRoute,
})

function CatalogRoute() {
  // Search and filters replace history: typing must not stack Back entries.
  const setSearch = useSetSearch<CatalogSearch>(Route.fullPath, true)
  return <CatalogPage search={Route.useSearch()} setSearch={setSearch} />
}
