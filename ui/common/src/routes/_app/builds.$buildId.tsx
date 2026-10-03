import { createFileRoute } from '@tanstack/react-router'
import { BuildPage } from '@/features/deploy/BuildPage'

/** One build (plans/feat-deploy.md §5): the id is the build id, which equals the upload id. */
export const Route = createFileRoute('/_app/builds/$buildId')({ component: BuildRoute })

function BuildRoute() {
  const { buildId } = Route.useParams()
  // Keyed: moving between builds (a toast's See why, Back/Forward) starts from a fresh stream and stage clock.
  return <BuildPage key={buildId} buildId={buildId} />
}
