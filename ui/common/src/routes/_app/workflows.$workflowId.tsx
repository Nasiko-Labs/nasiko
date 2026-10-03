import { createFileRoute } from '@tanstack/react-router'
import { WorkflowPage } from '@/features/workflows/WorkflowPage'
import { prefetchWorkflow } from '@/features/workflows/prefetch'
import { workflowSearchSchema } from '@/features/workflows/search'

export const Route = createFileRoute('/_app/workflows/$workflowId')({
  validateSearch: workflowSearchSchema,
  loaderDeps: ({ search }) => ({ run: search.run }),
  loader: ({ context, params, deps, preload }) => {
    if (preload) prefetchWorkflow(context.queryClient, params.workflowId, deps.run)
  },
  // Another workflow is another page: its editor and dialogs start fresh.
  remountDeps: ({ params }) => params.workflowId,
  component: WorkflowRoute,
})

function WorkflowRoute() {
  const { workflowId } = Route.useParams()
  return <WorkflowPage id={workflowId} run={Route.useSearch().run} />
}
