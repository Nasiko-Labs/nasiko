import { createFileRoute } from '@tanstack/react-router'
import { RunsPage } from '@/features/workflows/RunsPage'
import { prefetchRuns } from '@/features/workflows/prefetch'
import { runsSearchSchema, type RunsSearch } from '@/features/workflows/search'
import { useSetSearch } from '@/lib/search'

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
