import { createFileRoute } from '@tanstack/react-router'
import { useGuide, useGuideState } from '@/features/onboarding/api'
import { GuideHost } from '@/features/onboarding/GuideHost'
import { OverviewPage } from '@/features/overview/OverviewPage'
import { overviewSearchSchema, type OverviewSearch } from '@/features/overview/search'
import { useSetSearch } from '@/lib/search'

export const Route = createFileRoute('/_app/')({
  validateSearch: overviewSearchSchema,
  component: OverviewRoute,
})

function OverviewRoute() {
  const setSearch = useSetSearch<OverviewSearch>(Route.fullPath, true)
  const search = Route.useSearch()
  const { due } = useGuide()
  const state = useGuideState()
  // A first-time user gets the guide instead of the Overview (spec §2); skipping or finishing shows it.
  const firstRun = due || (state.open && state.firstRun)
  return (
    <>
      {firstRun ? null : <OverviewPage search={search} setSearch={setSearch} />}
      <GuideHost />
    </>
  )
}
