import { createFileRoute } from '@tanstack/react-router'
import { AgentDetailPage } from '@/features/agents/AgentDetailPage'
import { prefetchAgent } from '@/features/agents/prefetch'
import { detailSearchSchema } from '@/features/agents/search'

export const Route = createFileRoute('/_app/agents/$agentId')({
  validateSearch: detailSearchSchema,
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page (plan §8 Phase 3).
  loader: ({ context, params, preload }) => {
    if (preload) prefetchAgent(context.queryClient, params.agentId)
  },
  // Another agent is another page: its tabs, dialogs and watches start fresh.
  remountDeps: ({ params }) => params.agentId,
  component: DetailRoute,
})

function DetailRoute() {
  const { agentId } = Route.useParams()
  const { tab } = Route.useSearch()
  return <AgentDetailPage agentRef={agentId} tab={tab} />
}
