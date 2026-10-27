import { createFileRoute } from '@tanstack/react-router'
import { MyAgentsPage } from '@nasiko/features/agents/MyAgentsPage'
import { prefetchMine } from '@nasiko/features/agents/prefetch'
import { mineSearchSchema, type MineSearch } from '@nasiko/features/agents/search'
import { useSetSearch } from '@nasiko/ui/lib/search'

export const Route = createFileRoute('/_app/agents/mine')({
  validateSearch: mineSearchSchema,
  loaderDeps: ({ search: { owner } }) => ({ owner }),
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page (plan §8 Phase 3).
  loader: ({ context, deps, preload }) => {
    if (preload) prefetchMine(context.queryClient, deps)
  },
  component: MineRoute,
})

function MineRoute() {
  const setSearch = useSetSearch<MineSearch>(Route.fullPath, true)
  return <MyAgentsPage search={Route.useSearch()} setSearch={setSearch} />
}
