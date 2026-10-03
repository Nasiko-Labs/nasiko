import { createFileRoute } from '@tanstack/react-router'
import { prefetchTokenops } from '@/features/tokenops/prefetch'
import { tokenopsSearchSchema, type TokenopsSearch } from '@/features/tokenops/search'
import { TokenopsPage } from '@/features/tokenops/TokenopsPage'
import { useSetSearch } from '@/lib/search'

export const Route = createFileRoute('/_app/tokenops')({
  validateSearch: tokenopsSearchSchema,
  loaderDeps: ({ search: { preset, from, to, view, agent, provider, model } }) => ({
    preset,
    from,
    to,
    view,
    agent,
    provider,
    model,
  }),
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page (plan §8 Phase 3).
  loader: ({ context, deps, preload }) => {
    if (preload) prefetchTokenops(context.queryClient, deps)
  },
  component: TokenopsRoute,
})

function TokenopsRoute() {
  const setSearch = useSetSearch<TokenopsSearch>(Route.fullPath)
  return <TokenopsPage search={Route.useSearch()} setSearch={setSearch} />
}
