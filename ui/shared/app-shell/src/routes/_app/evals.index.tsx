import { createFileRoute } from '@tanstack/react-router'
import { EvalsPage } from '@nasiko/features/evals/EvalsPage'
import { evalsSearchSchema, type EvalsSearch } from '@nasiko/features/evals/search'
import { useSetSearch } from '@nasiko/ui/lib/search'

/** Claude Code's evals: sessions → dataset → comparison (docs/superpowers/specs/2026-11-23-evals-design.md). */
export const Route = createFileRoute('/_app/evals/')({
  validateSearch: evalsSearchSchema,
  component: EvalsRoute,
})

function EvalsRoute() {
  // The dataset and run ids are pushed: Back returns to the step before.
  const setSearch = useSetSearch<EvalsSearch>(Route.fullPath)
  return <EvalsPage search={Route.useSearch()} setSearch={setSearch} />
}
