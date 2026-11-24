/**
 * `/evals` (docs/superpowers/specs/2026-11-23-evals-design.md): the viewer's connected coding harnesses, each opening
 * its sessions; with none connected, the way to connect one.
 */
import { Link } from '@tanstack/react-router'
import { ChevronRight, FlaskConical } from 'lucide-react'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { isEndpointAbsent } from '@nasiko/data-access/detect'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { EmptyState, StateCard } from '@nasiko/ui/composites/state-card'
import { fmtShortDay } from '@nasiko/ui/lib/format'
import { asButton, Button } from '@nasiko/ui/primitives/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@nasiko/ui/primitives/card'
import { useEvalHarnesses } from './api'
import { copy } from './copy'
import { harnessName } from './logic'

/** What a failed evals read means: a server without the routes, or a failure worth retrying. */
export function EvalsError({ error, retry }: { error: unknown; retry: () => void }) {
  return isEndpointAbsent(error) ? (
    <StateCard title={copy.absent}>{copy.absentBody}</StateCard>
  ) : (
    <StateCard
      tone="error"
      title={copy.failed}
      action={
        <Button size="sm" variant="outline" onClick={retry}>
          {copy.retry}
        </Button>
      }
    />
  )
}

export function EvalsPage() {
  const q = useEvalHarnesses()
  return (
    <div className="mx-auto flex w-full max-w-page flex-col gap-4">
      <TopBar />
      <PageHeader title={copy.title} description={copy.description} />
      {q.isPending ? (
        <PageLoader label={copy.loading} />
      ) : q.isError ? (
        <EvalsError error={q.error} retry={() => void q.refetch()} />
      ) : !q.data.length ? (
        <EmptyState
          icon={FlaskConical}
          title={copy.empty.title}
          action={
            <Link to="/harnesses" {...asButton({ size: 'sm' })}>
              {copy.empty.action}
            </Link>
          }
        >
          {copy.empty.body}
        </EmptyState>
      ) : (
        <>
          <p className="text-sm text-muted-foreground">{copy.pickHarness}</p>
          <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {q.data.map((h) => (
              <li key={h.id}>
                <Link
                  to="/evals/$harness"
                  params={{ harness: h.id }}
                  className="group block rounded-xl focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background focus-visible:outline-none"
                >
                  <Card className="transition-colors group-hover:bg-muted/50">
                    <CardHeader>
                      <CardTitle className="flex items-center justify-between gap-2">
                        {harnessName(h.id)}
                        <ChevronRight className="size-4 text-muted-foreground" aria-hidden />
                      </CardTitle>
                      <CardDescription>{copy.sessionCount(h.sessions)}</CardDescription>
                    </CardHeader>
                    {h.last_active_at ? (
                      <CardContent className="text-xs text-muted-foreground">
                        {copy.lastActive(fmtShortDay(h.last_active_at))}
                      </CardContent>
                    ) : null}
                  </Card>
                </Link>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  )
}
