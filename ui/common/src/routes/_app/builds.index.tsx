import { createFileRoute } from '@tanstack/react-router'
import { useCallback } from 'react'
import { buildsSearchSchema, type BuildsSearch } from '@/features/deploy/search'
import { BuildsPage } from '@/features/deploy/BuildsPage'

export const Route = createFileRoute('/_app/builds/')({
  validateSearch: (raw: Record<string, unknown>) => buildsSearchSchema.parse(raw),
  component: BuildsRoute,
})

function BuildsRoute() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  // Filters and search replace history; paging pushes it (the TokenOps rule).
  const setSearch = useCallback(
    (patch: Partial<BuildsSearch>, opts?: { replace?: boolean }) =>
      void navigate({
        search: (prev) => ({ ...prev, ...patch }),
        replace: opts?.replace ?? false,
        resetScroll: false,
      }),
    [navigate],
  )
  return <BuildsPage search={search} setSearch={setSearch} />
}
