/**
 * Fleet health (plans/feat-overview.md §5.3): counts over every visible agent (design review 16B), each linking to the
 * Agents catalog filtered by that rating (15A, computed by the same `useFleetHealth`, eng review R1), a square per agent
 * in rating order (the badges are its labels, so it is decorative), then the rated agents, Watch first with their reason,
 * each with its rating and 7-day p95. Needs-action agents go to Needs you instead of repeating here.
 */
import { Link } from '@tanstack/react-router'
import { CircleCheck, TriangleAlert } from 'lucide-react'
import { useRef } from 'react'
import { EmptyState } from '@/components/shared/state-card'
import { Badge } from '@/components/ui/badge'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { fmtLatency } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { FleetHealth as FleetHealthData } from '../api'
import { copy } from '../copy'
import type { Rating } from '../health'
import { HEALTH_LIST_ROWS } from '../tuning'
import { Card, CardError, CardSkeleton, SourceFailed, TOUCH } from './Card'

const ORDER: readonly Rating[] = ['healthy', 'watch', 'action', 'unknown']
/** Status tokens, never the accent (design review 12A). */
const SEGMENT: Record<Rating, string> = {
  healthy: 'bg-success',
  watch: 'bg-warning',
  action: 'bg-destructive',
  unknown: 'bg-muted-foreground/30',
}
/** Each count is a status `Badge` that links to the filtered catalog; zero counts stay quiet. */
const BADGE: Record<Rating, 'success' | 'warning' | 'destructive' | 'muted'> = {
  healthy: 'success',
  watch: 'warning',
  action: 'destructive',
  unknown: 'muted',
}

export function FleetHealth({ fleet, className }: { fleet: FleetHealthData; className?: string }) {
  const titleRef = useRef<HTMLHeadingElement>(null)
  const s = fleet.summary
  const total = s ? ORDER.reduce((n, r) => n + s.counts[r], 0) : 0
  const rated = s ? total - s.counts.unknown : 0
  const healthy = s
    ? [...s.byId.values()]
        .filter((a) => a.rating === 'healthy')
        .sort((a, b) => a.name.localeCompare(b.name))
    : []
  const listed = s ? [...s.watch, ...healthy].slice(0, HEALTH_LIST_ROWS) : []
  const squares = s
    ? [...s.byId.values()].sort(
        (a, b) => ORDER.indexOf(a.rating) - ORDER.indexOf(b.rating) || a.name.localeCompare(b.name),
      )
    : []
  return (
    <Card
      id="overview-health"
      title={copy.health.title}
      to="/agents"
      linkLabel={copy.health.link}
      titleRef={titleRef}
      className={className}
    >
      {fleet.isPending && !s ? (
        <CardSkeleton />
      ) : !s ? (
        <CardError what={copy.health.what} onRetry={fleet.retry} titleRef={titleRef} />
      ) : (
        <div className="flex flex-col gap-3">
          <p className="flex flex-wrap gap-1.5">
            {ORDER.map((r) => {
              const names =
                r === 'action'
                  ? s.action.map((a) => a.name)
                  : r === 'watch'
                    ? s.watch.map((a) => a.name)
                    : []
              return (
                <Badge
                  key={r}
                  asChild
                  variant={s.counts[r] ? BADGE[r] : 'outline'}
                  className={cn(
                    'font-normal tabular-nums focus-visible:ring-ring dark:focus-visible:ring-ring',
                    TOUCH,
                    !s.counts[r] && 'text-muted-foreground',
                  )}
                >
                  <Link
                    to="/agents"
                    search={{ health: r } as never}
                    aria-label={copy.health.countLink(copy.ratingCount[r](s.counts[r]), names)}
                  >
                    {copy.ratingCount[r](s.counts[r])}
                  </Link>
                </Badge>
              )
            })}
          </p>
          {/* auto-fill keeps a square's size fixed by the card, so a small fleet stays small; a large one wraps. */}
          <div
            aria-hidden
            data-testid="health-squares"
            className="grid grid-cols-[repeat(auto-fill,minmax(0.75rem,1fr))] gap-1"
          >
            {squares.map((a) => (
              <Tooltip key={a.id}>
                <TooltipTrigger asChild>
                  <span className={cn('h-5 rounded-xs', SEGMENT[a.rating])} />
                </TooltipTrigger>
                <TooltipContent>{`${a.name} · ${copy.rating[a.rating]}`}</TooltipContent>
              </Tooltip>
            ))}
          </div>
          {fleet.costFailed ? (
            <SourceFailed what={copy.health.costWhat} onRetry={fleet.retryCost} />
          ) : null}
          {total > 0 && rated === 0 ? (
            // Nothing rated: no agent is deployed (else the cost-data line above says why). QA ISSUE-004.
            fleet.deployedCount === 0 ? (
              <div data-testid="health-none-deployed">
                <EmptyState title={copy.health.noneDeployed} className="py-6 md:py-6" />
              </div>
            ) : null
          ) : (
            <div>
              {!s.watch.length ? (
                <p className="text-xs text-muted-foreground">{copy.health.noWatch}</p>
              ) : null}
              <ul className="divide-y divide-border" aria-label={copy.health.title}>
                {listed.map((a) => {
                  const p95 = fleet.p95.get(a.id)
                  return (
                    <li
                      key={a.id}
                      className={cn('flex min-h-11 items-center gap-3 py-1.5 text-sm', TOUCH)}
                    >
                      <span className="min-w-0 flex-1 truncate">
                        <Link
                          to="/agents/$agentId"
                          params={{ agentId: a.id }}
                          search={{}}
                          className="font-medium underline-offset-4 hover:underline"
                        >
                          {a.name}
                        </Link>
                        {a.reasons[0] ? (
                          <span className="text-xs text-muted-foreground">
                            {' '}
                            · {a.reasons[0].text}
                          </span>
                        ) : null}
                      </span>
                      {/* A plain icon and the word, never a tinted pill: tinted badges are only the counts above (design 9A). */}
                      <span
                        className={cn(
                          'inline-flex shrink-0 items-center gap-1 text-xs',
                          a.rating === 'watch' ? 'text-warning' : 'text-success',
                        )}
                      >
                        {a.rating === 'watch' ? (
                          <TriangleAlert className="size-3.5" aria-hidden />
                        ) : (
                          <CircleCheck className="size-3.5" aria-hidden />
                        )}
                        {copy.rating[a.rating]}
                      </span>
                      <span
                        className="w-14 shrink-0 text-right font-mono text-xs text-muted-foreground tabular-nums"
                        title={copy.health.colP95}
                      >
                        {p95 === undefined ? '—' : fmtLatency(p95)}
                      </span>
                    </li>
                  )
                })}
              </ul>
            </div>
          )}
        </div>
      )}
    </Card>
  )
}
