import { createFileRoute } from '@tanstack/react-router'
import { sessionsSearchSchema, type SessionsSearch } from '@nasiko/features/sessions/search'
import { prefetchSessions } from '@nasiko/features/sessions/api'
import { SessionsPage } from '@nasiko/features/sessions/SessionsPage'
import { useSetSearch } from '@nasiko/ui/lib/search'

export const Route = createFileRoute('/_app/sessions/')({
  validateSearch: sessionsSearchSchema,
  loaderDeps: ({ search: { preset, from, to, day } }) => ({ preset, from, to, day }),
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page (plan §8 Phase 3).
  loader: ({ context, deps, preload }) => {
    if (preload) prefetchSessions(context.queryClient, deps)
  },
  component: SessionsRoute,
})

function SessionsRoute() {
  const setSearch = useSetSearch<SessionsSearch>(Route.fullPath)
  return <SessionsPage search={Route.useSearch()} setSearch={setSearch} />
}
