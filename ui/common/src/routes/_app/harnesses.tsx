import { createFileRoute } from '@tanstack/react-router'
import { useSlots } from '@/app/edition-context'
import { HarnessesPage } from '@/features/harnesses/HarnessesPage'
import { harnessesSearchSchema, type HarnessesSearch } from '@/features/harnesses/search'
import { useSetSearch } from '@/lib/search'

export const Route = createFileRoute('/_app/harnesses')({
  validateSearch: harnessesSearchSchema,
  component: HarnessesRoute,
})

/** A layer that serves the org levels (EE) renders its own page here; the core shows the viewer's own usage. */
function HarnessesRoute() {
  const OrgPage = useSlots().harnessOrgScope?.Page
  const setSearch = useSetSearch<HarnessesSearch>(Route.fullPath)
  const search = Route.useSearch()
  return OrgPage ? <OrgPage /> : <HarnessesPage search={search} setSearch={setSearch} />
}
