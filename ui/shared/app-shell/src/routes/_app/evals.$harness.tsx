import { createFileRoute } from '@tanstack/react-router'
import { HarnessEvalsPage } from '@nasiko/features/evals/HarnessEvalsPage'
import { evalsSearchSchema, type EvalsSearch } from '@nasiko/features/evals/search'
import { useSetSearch } from '@nasiko/ui/lib/search'

/** One harness's sessions → dataset → comparison (docs/superpowers/specs/2026-11-23-evals-design.md). */
export const Route = createFileRoute('/_app/evals/$harness')({
  validateSearch: evalsSearchSchema,
  // Another harness is another page: its ticks, dataset and results start fresh.
  remountDeps: ({ params }) => params.harness,
  component: HarnessEvalsRoute,
})

function HarnessEvalsRoute() {
  const { harness } = Route.useParams()
  // The dataset and run ids are pushed: Back returns to the step before.
  const setSearch = useSetSearch<EvalsSearch>(Route.fullPath)
  return <HarnessEvalsPage harness={harness} search={Route.useSearch()} setSearch={setSearch} />
}
