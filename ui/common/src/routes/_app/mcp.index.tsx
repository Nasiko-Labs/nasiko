import { createFileRoute } from '@tanstack/react-router'
import { CatalogPage } from '@/features/mcp/CatalogPage'
import { prefetchCatalog } from '@/features/mcp/prefetch'
import { catalogSearchSchema, type CatalogSearch } from '@/features/mcp/search'
import { useSetSearch } from '@/lib/search'

export const Route = createFileRoute('/_app/mcp/')({
  validateSearch: catalogSearchSchema,
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page.
  loader: ({ context, preload }) => {
    if (preload) prefetchCatalog(context.queryClient)
  },
  component: CatalogRoute,
})

function CatalogRoute() {
  // Search, scope and tab replace history: typing must not stack Back entries.
  const setSearch = useSetSearch<CatalogSearch>(Route.fullPath, true)
  return <CatalogPage search={Route.useSearch()} setSearch={setSearch} />
}
