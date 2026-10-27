import { createFileRoute } from '@tanstack/react-router'
import { NewWorkflowPage } from '@nasiko/features/workflows/NewWorkflowPage'

export const Route = createFileRoute('/_app/workflows/new')({
  component: NewWorkflowPage,
})
