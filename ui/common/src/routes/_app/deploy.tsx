import { createFileRoute } from '@tanstack/react-router'
import { useCallback } from 'react'
import { deploySearchSchema, type DeploySearch } from '@/features/deploy/search'
import { DeployPage } from '@/features/deploy/DeployPage'

export const Route = createFileRoute('/_app/deploy')({
  validateSearch: (raw: Record<string, unknown>) => deploySearchSchema.parse(raw),
  component: DeployRoute,
})

function DeployRoute() {
  const navigate = Route.useNavigate()
  // The method tab and the picked repository replace history (they are views, not steps).
  const setSearch = useCallback(
    (patch: Partial<DeploySearch>) =>
      void navigate({
        search: (prev) => ({ ...prev, ...patch }),
        replace: true,
        resetScroll: false,
      }),
    [navigate],
  )
  return <DeployPage search={Route.useSearch()} setSearch={setSearch} />
}
