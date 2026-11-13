import { createFileRoute } from '@tanstack/react-router'
import { BuildsPage } from '@nasiko/features/mcp/BuildsPage'
import { mcpBuildsSearchSchema, type McpBuildsSearch } from '@nasiko/features/mcp/search'
import { useSetSearch } from '@nasiko/ui/lib/search'

export const Route = createFileRoute('/_app/mcp/builds')({
  validateSearch: mcpBuildsSearchSchema,
  component: McpBuildsRoute,
})

function McpBuildsRoute() {
  const setSearch = useSetSearch<McpBuildsSearch>(Route.fullPath, true)
  return <BuildsPage search={Route.useSearch()} setSearch={setSearch} />
}
