import { createFileRoute } from '@tanstack/react-router'
import { DetailPage } from '@/features/mcp/DetailPage'
import { prefetchConnector } from '@/features/mcp/prefetch'
import { detailSearchSchema } from '@/features/mcp/search'

export const Route = createFileRoute('/_app/mcp/$connectorId')({
  validateSearch: detailSearchSchema,
  loader: ({ context, params, preload }) => {
    if (preload) prefetchConnector(context.queryClient, params.connectorId)
  },
  // Another server is another page: its tabs and dialogs start fresh.
  remountDeps: ({ params }) => params.connectorId,
  component: DetailRoute,
})

function DetailRoute() {
  const { connectorId } = Route.useParams()
  return <DetailPage id={connectorId} search={Route.useSearch()} />
}
