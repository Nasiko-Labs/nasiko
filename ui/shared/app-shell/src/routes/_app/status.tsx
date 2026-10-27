import { createFileRoute } from '@tanstack/react-router'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { Card } from '@nasiko/ui/primitives/card'
import { useHealth } from '@nasiko/data-access/health'

export const Route = createFileRoute('/_app/status')({ component: StatusPage })

function StatusPage() {
  const health = useHealth()
  const state = health.isPending ? 'checking' : health.isError ? 'unreachable' : 'connected'
  return (
    <div className="flex max-w-2xl flex-col gap-4">
      <PageHeader title="Backend status" />
      <Card data-tour="status-backend" className="gap-0 p-4 text-sm">
        <div>
          OSS nasiko-server via the Vite proxy: <strong data-testid="backend-state">{state}</strong>
        </div>
        {health.isError ? (
          <p className="mt-2 text-muted-foreground">
            Start it from <code>nasiko-cloud-rs</code> with <code>just run-stack</code>, or point{' '}
            <code>NASIKO_API_URL</code> at it.
          </p>
        ) : null}
      </Card>
    </div>
  )
}
