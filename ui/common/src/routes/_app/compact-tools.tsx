import { createFileRoute } from '@tanstack/react-router'
import { CompactToolsPage } from '@/features/compact-tools/CompactToolsPage'

export const Route = createFileRoute('/_app/compact-tools')({
  component: CompactToolsPage,
})
