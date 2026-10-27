import { createFileRoute } from '@tanstack/react-router'
import { FlowsPage } from '@nasiko/features/flows/FlowsPage'
import { prefetchFlows } from '@nasiko/features/flows/prefetch'
import { flowsSearchSchema, type FlowsSearch } from '@nasiko/features/flows/search'
import { useSetSearch } from '@nasiko/ui/lib/search'

/** Your flows (plans/feat-flows.md §3). */
export const Route = createFileRoute('/_app/flows/')({
  validateSearch: flowsSearchSchema,
  loaderDeps: ({ search: { preset, from, to, q, status } }) => ({ preset, from, to, q, status }),
  loader: ({ context, deps, preload }) => {
    if (preload) prefetchFlows(context.queryClient, deps)
  },
  component: FlowsRoute,
})

function FlowsRoute() {
  const setSearch = useSetSearch<FlowsSearch>(Route.fullPath)
  return <FlowsPage search={Route.useSearch()} setSearch={setSearch} />
}
