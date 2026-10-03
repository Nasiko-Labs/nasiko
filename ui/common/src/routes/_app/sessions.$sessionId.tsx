import { createFileRoute } from '@tanstack/react-router'
import { traceSearchSchema, type TraceSearch } from '@/features/sessions/search'
import { prefetchSessionDetail } from '@/features/sessions/api'
import { SessionTracePage } from '@/features/trace/SessionTracePage'
import { useSetSearch } from '@/lib/search'

/** Session trace: a sibling of the Sessions list (not nested), so it replaces the list. */
export const Route = createFileRoute('/_app/sessions/$sessionId')({
  validateSearch: traceSearchSchema,
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page (plan §8 Phase 3).
  loader: ({ context, params, preload }) => {
    if (preload) prefetchSessionDetail(context.queryClient, params.sessionId)
  },
  // Another session is another page: its frozen clock, selection and panels start fresh.
  remountDeps: ({ params }) => params.sessionId,
  component: TraceRoute,
})

function TraceRoute() {
  const { sessionId } = Route.useParams()
  const setSearch = useSetSearch<TraceSearch>(Route.fullPath, true)
  return <SessionTracePage sessionId={sessionId} search={Route.useSearch()} setSearch={setSearch} />
}
