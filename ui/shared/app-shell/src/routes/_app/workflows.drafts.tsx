import { createFileRoute } from '@tanstack/react-router'
import { WorkflowsPage } from '@nasiko/features/workflows/WorkflowsPage'
import { prefetchDrafts } from '@nasiko/features/workflows/prefetch'
import { draftsSearchSchema, type DraftsSearch } from '@nasiko/features/workflows/search'
import { useSetSearch } from '@nasiko/ui/lib/search'

/** One page serves both lists; it only sends sorts from its own mode's list. */
type SetListSearch = (patch: Partial<{ q: string; sort: string }>) => void

export const Route = createFileRoute('/_app/workflows/drafts')({
  validateSearch: draftsSearchSchema,
  loaderDeps: ({ search }) => ({ sort: search.sort }),
  loader: ({ context, deps, preload }) => {
    if (preload) prefetchDrafts(context.queryClient, deps.sort)
  },
  component: DraftsRoute,
})

function DraftsRoute() {
  const setSearch = useSetSearch<DraftsSearch>(Route.fullPath, true) as SetListSearch
  return <WorkflowsPage mode="drafts" search={Route.useSearch()} setSearch={setSearch} />
}
