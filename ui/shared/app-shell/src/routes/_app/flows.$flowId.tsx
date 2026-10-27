import { createFileRoute } from '@tanstack/react-router'
import { FlowPage } from '@nasiko/features/flows/FlowPage'
import { prefetchFlow } from '@nasiko/features/flows/prefetch'
import { flowSearchSchema, type FlowSearch } from '@nasiko/features/flows/search'
import { useSetSearch } from '@nasiko/ui/lib/search'

/** One flow (plans/feat-flows.md §2): the id is the flow's trace id; `?step=` is the selected call (F25). */
export const Route = createFileRoute('/_app/flows/$flowId')({
  validateSearch: flowSearchSchema,
  loader: ({ context, params, preload }) => {
    if (preload) prefetchFlow(context.queryClient, params.flowId)
  },
  // Another flow is another page: its clock, selection and announcements start fresh.
  remountDeps: ({ params }) => params.flowId,
  component: FlowRoute,
})

function FlowRoute() {
  const { flowId } = Route.useParams()
  // Selection replaces history (F25): Back leaves the flow, it doesn't walk the clicks.
  const setSearch = useSetSearch<FlowSearch>(Route.fullPath, true)
  return (
    <FlowPage
      flowId={flowId}
      step={Route.useSearch().step ?? null}
      onStep={(step) => setSearch({ step: step ?? undefined })}
    />
  )
}
