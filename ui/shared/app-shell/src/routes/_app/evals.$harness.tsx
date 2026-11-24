import { createFileRoute } from '@tanstack/react-router'
import { HarnessEvalsPage } from '@nasiko/features/evals/HarnessEvalsPage'

/** One harness's sessions → dataset → comparison (docs/superpowers/specs/2026-11-23-evals-design.md). */
export const Route = createFileRoute('/_app/evals/$harness')({
  // Another harness is another page: its ticks, dataset and results start fresh.
  remountDeps: ({ params }) => params.harness,
  component: HarnessEvalsRoute,
})

function HarnessEvalsRoute() {
  const { harness } = Route.useParams()
  return <HarnessEvalsPage harness={harness} />
}
