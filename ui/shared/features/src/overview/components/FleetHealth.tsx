/**
 * Fleet health (plans/feat-overview.md §5.3): counts over every visible agent (design review 16B), each linking to the
 * Agents catalog filtered by that rating (15A, computed by the same `useFleetHealth`, eng review R1), a square per agent
 * in rating order up to `HEALTH_SQUARES_MAX` agents and one bar sized by the counts past it (NAS-1519: a 1,000-agent
 * fleet drew a 980 px block); the badges are its labels, so either is decorative. Then the rated agents, Watch first
 * with their reason, each with its rating and 7-day p95. Needs-action agents go to Needs you instead of repeating here.
 *
 * Redesign 2026-10-19: it sits beside Needs you and opens with the run states the summary's old Agents running tile
 * showed ("14 of 19 running · 1 deploying · 2 crashed or failed"), so the summary's tiles can carry money and tokens.
 */
import { Link } from '@tanstack/react-router'
import { Bot, CircleCheck, TriangleAlert } from 'lucide-react'
import { useRef, useState } from 'react'
import { asButton } from '@nasiko/ui/primitives/button'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { displayStatus, isHarness, type DisplayStatus } from '@nasiko/features/agents/status'
import { fmtLatency } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import type { FleetHealth as FleetHealthData } from '../api'
import { copy } from '../copy'
import type { Rating } from '../health'
import { HEALTH_LIST_ROWS, HEALTH_SQUARES_MAX } from '../tuning'
import { Card, CardError, CardQuiet, CardSkeleton, SourceFailed, TOUCH } from './Card'

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

const PARTS = ['deploying', 'attention', 'stopped', 'not-deployed'] as const
/** A list row's fixed height (`h-11`), for the list's minimum. */
const ROW_REM = 2.75

/** "14 of 19 running · 1 deploying · 2 crashed or failed": the directory's run states, harnesses left out. */
function runStates(fleet: FleetHealthData): string {
  const states: DisplayStatus[] = [...fleet.byId.values()]
    .filter((a) => !isHarness(a))
    .map((a) => displayStatus(a.status, false))
  if (!states.length) return copy.kpi.noAgents
  const count = (s: DisplayStatus) => states.filter((x) => x === s).length
  return [
    copy.kpi.running(count('running'), states.length),
    ...PARTS.filter((s) => count(s) > 0).map((s) => copy.kpi.agentParts[s](count(s))),
  ].join(' · ')
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
  const candidates = s ? [...s.watch, ...healthy] : []
  // The list takes the height its row gives the card (Needs you beside it is often taller): as many rows as fit,
  // never fewer than HEALTH_LIST_ROWS and never a cut-off one. The rows sit out of flow, so they never grow the card.
  const [fit, setFit] = useState(HEALTH_LIST_ROWS)
  const listed = candidates.slice(0, fit)
  const fillRef = (box: HTMLDivElement | null) => {
    if (!box) return
    const observer = new ResizeObserver(() => {
      const row = box.querySelector('li')?.offsetHeight
      if (row) setFit(Math.max(HEALTH_LIST_ROWS, Math.floor(box.clientHeight / row)))
    })
    observer.observe(box)
    return () => observer.disconnect()
  }
  const asBar = total > HEALTH_SQUARES_MAX
  const squares =
    s && !asBar
      ? [...s.byId.values()].sort(
          (a, b) =>
            ORDER.indexOf(a.rating) - ORDER.indexOf(b.rating) || a.name.localeCompare(b.name),
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
        <div className="flex flex-1 flex-col gap-3">
          <p className="text-sm text-muted-foreground tabular-nums" data-testid="health-run-states">
            {runStates(fleet)}
          </p>
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
                  variant={s.counts[r] ? BADGE[r] : 'outline'}
                  className={cn(
                    'font-normal tabular-nums focus-visible:ring-ring dark:focus-visible:ring-ring',
                    TOUCH,
                    !s.counts[r] && 'text-muted-foreground',
                  )}
                  render={
                    <Link
                      to="/agents"
                      search={{ health: r } as never}
                      aria-label={copy.health.countLink(copy.ratingCount[r](s.counts[r]), names)}
                    />
                  }
                >
                  {copy.ratingCount[r](s.counts[r])}
                </Badge>
              )
            })}
          </p>
          {asBar ? (
            // Each rating's share of the fleet, in the badges' order (4 px at least, so a small share still shows).
            <div
              aria-hidden
              data-testid="health-bar"
              className="flex h-3 gap-0.5 overflow-hidden rounded-sm"
            >
              {ORDER.filter((r) => s.counts[r]).map((r) => (
                <span
                  key={r}
                  className={cn('min-w-1 basis-0', SEGMENT[r])}
                  style={{ flexGrow: s.counts[r] }}
                />
              ))}
            </div>
          ) : (
            // auto-fill keeps a square's size fixed by the card, so a small fleet stays small.
            <div
              aria-hidden
              data-testid="health-squares"
              className="grid grid-cols-[repeat(auto-fill,minmax(0.75rem,1fr))] gap-1"
            >
              {squares.map((a) => (
                <Tooltip key={a.id}>
                  <TooltipTrigger
                    render={<span className={cn('h-5 rounded-xs', SEGMENT[a.rating])} />}
                  />
                  <TooltipContent>{`${a.name} · ${copy.rating[a.rating]}`}</TooltipContent>
                </Tooltip>
              ))}
            </div>
          )}
          {fleet.costFailed ? (
            <SourceFailed what={copy.health.costWhat} onRetry={fleet.retryCost} />
          ) : null}
          {total > 0 && rated === 0 ? (
            // Nothing rated: no agent is deployed (else the cost-data line above says why). QA ISSUE-004.
            fleet.deployedCount === 0 ? (
              <CardQuiet
                Icon={Bot}
                title={copy.cardQuiet.health}
                detail={copy.cardQuiet.healthDetail}
                action={
                  <Link
                    to="/agents"
                    search={{} as never}
                    {...asButton({ variant: 'outline', size: 'sm', className: TOUCH })}
                  >
                    {copy.cardQuiet.healthAction}
                  </Link>
                }
                testId="health-none-deployed"
              />
            ) : null
          ) : (
            <div className="flex flex-1 flex-col">
              {!s.watch.length ? (
                <p className="text-xs text-muted-foreground">{copy.health.noWatch}</p>
              ) : null}
              <div
                ref={fillRef}
                data-testid="health-list"
                className="relative flex-1"
                style={{
                  minHeight: `${Math.min(HEALTH_LIST_ROWS, candidates.length) * ROW_REM}rem`,
                }}
              >
                <ul
                  className="absolute inset-x-0 top-0 divide-y divide-border"
                  aria-label={copy.health.title}
                >
                  {listed.map((a) => {
                    const p95 = fleet.p95.get(a.id)
                    return (
                      <li
                        key={a.id}
                        className={cn('flex h-11 items-center gap-3 py-1.5 text-sm', TOUCH)}
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
            </div>
          )}
        </div>
      )}
    </Card>
  )
}
