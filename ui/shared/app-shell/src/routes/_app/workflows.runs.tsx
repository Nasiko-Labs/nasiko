import { createFileRoute } from '@tanstack/react-router'
import { RunsPage } from '@nasiko/features/workflows/RunsPage'
import { prefetchRuns } from '@nasiko/features/workflows/prefetch'
import { runsSearchSchema, type RunsSearch } from '@nasiko/features/workflows/search'
import { useSetSearch } from '@nasiko/ui/lib/search'

export const Route = createFileRoute('/_app/workflows/runs')({
  validateSearch: runsSearchSchema,
  loader: ({ context, preload }) => {
    if (preload) prefetchRuns(context.queryClient)
  },
  component: RunsRoute,
})

function RunsRoute() {
  const setSearch = useSetSearch<RunsSearch>(Route.fullPath, true)
  return <RunsPage search={Route.useSearch()} setSearch={setSearch} />
}
