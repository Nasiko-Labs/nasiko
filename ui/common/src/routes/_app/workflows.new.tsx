import { createFileRoute } from '@tanstack/react-router'
import { NewWorkflowPage } from '@/features/workflows/NewWorkflowPage'

export const Route = createFileRoute('/_app/workflows/new')({
  component: NewWorkflowPage,
})
