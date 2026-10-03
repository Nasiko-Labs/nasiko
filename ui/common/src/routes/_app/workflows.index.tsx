import { createFileRoute } from '@tanstack/react-router'
import { WorkflowsPage } from '@/features/workflows/WorkflowsPage'
import { prefetchDeployed } from '@/features/workflows/prefetch'
import { deployedSearchSchema, type DeployedSearch } from '@/features/workflows/search'
import { useSetSearch } from '@/lib/search'

/** One page serves both lists; it only sends sorts from its own mode's list. */
type SetListSearch = (patch: Partial<{ q: string; sort: string }>) => void

export const Route = createFileRoute('/_app/workflows/')({
  validateSearch: deployedSearchSchema,
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page.
  loaderDeps: ({ search }) => ({ sort: search.sort }),
  loader: ({ context, deps, preload }) => {
    if (preload) prefetchDeployed(context.queryClient, deps.sort)
  },
  component: DeployedRoute,
})

function DeployedRoute() {
  // Search and sort replace history: typing must not stack Back entries.
  const setSearch = useSetSearch<DeployedSearch>(Route.fullPath, true) as SetListSearch
  return <WorkflowsPage mode="deployed" search={Route.useSearch()} setSearch={setSearch} />
}
