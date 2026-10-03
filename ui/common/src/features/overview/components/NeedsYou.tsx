/**
 * Needs you (plans/feat-overview.md §6): the inbox, most urgent first. Severity is said once, by a plain icon beside
 * the text (design review 9A); five rows show and the rest scroll inside the card (2A); the header always says when
 * it last checked (8A); a failed source adds a line and never turns into "Nothing needs you" (5A).
 */
import { Link } from '@tanstack/react-router'
import { CircleAlert, Hand, TriangleAlert } from 'lucide-react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { EmptyState } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Skeleton } from '@/components/ui/skeleton'
import { relTime } from '@/features/agents/format'
import { rememberRailView } from '@/features/chat/rememberTarget'
import { cn } from '@/lib/utils'
import type { NeedsYou as NeedsYouData } from '../api'
import { copy } from '../copy'
import type { NeedRow, Severity } from '../needs'
import { CHECKED_TICK_MS, LAST_WEEK, NEEDS_VISIBLE_ROWS } from '../tuning'
import { Card, CardError, CardSkeleton, SourceFailed, TOUCH } from './Card'

/** `detail` colours the row's reason: action and watch say why in their own tone; a waiting request stays quiet. */
const ICON: Record<
  Severity,
  { Icon: typeof CircleAlert; tone: string; detail: string; label: string }
> = {
  action: {
    Icon: CircleAlert,
    tone: 'text-destructive',
    detail: 'text-destructive',
    label: copy.rating.action,
  },
  watch: {
    Icon: TriangleAlert,
    tone: 'text-warning',
    detail: 'text-warning',
    label: copy.rating.watch,
  },
  waiting: {
    Icon: Hand,
    tone: 'text-info',
    detail: 'text-muted-foreground',
    label: copy.needs.waiting,
  },
}
/** A row's one action: a small outline button that is a link. */
function Action({ children }: { children: ReactNode }) {
  return (
    <Button asChild variant="outline" size="sm" className={cn('shrink-0 text-xs', TOUCH)}>
      {children}
    </Button>
  )
}

export function NeedsYou({
  data,
  now,
  userId,
  className,
}: {
  data: NeedsYouData
  now: number
  userId: string
  className?: string
}) {
  const titleRef = useRef<HTMLHeadingElement>(null)
  const { needs } = data
  // Every source the server has failed: one card error (on OSS, budgets are absent, so three of three count).
  const allFailed =
    !needs.rows.length &&
    !needs.loading &&
    needs.failed.length > 0 &&
    needs.failed.length === needs.available
  // "Checked …" ages on an open tab: the page's `now` is frozen per visit (/ship adversarial).
  const [clock, setClock] = useState(() => Date.now())
  useEffect(() => {
    const t = setInterval(() => setClock(Date.now()), CHECKED_TICK_MS)
    return () => clearInterval(t)
  }, [])
  const ageNow = Math.max(now, clock)
  const count = { action: 0, watch: 0, waiting: 0 }
  for (const r of needs.rows) count[r.severity]++
  return (
    <Card
      id="overview-needs"
      title={copy.needs.title}
      titleClassName="text-lg tracking-tight"
      // Anything that needs action lifts the card: a destructive edge along the top (live bolder pass).
      className={cn(
        'gap-3.5',
        className,
        count.action > 0 && 'border-destructive/35 shadow-[inset_0_3px_0_var(--color-destructive)]',
      )}
      meta={
        needs.rows.length ? (
          <span className="inline-flex gap-3 font-medium">
            {(['action', 'watch', 'waiting'] as const).map((s) =>
              count[s] ? (
                <span key={s} className={ICON[s].tone}>
                  {copy.needs.count[s](count[s])}
                </span>
              ) : null,
            )}
          </span>
        ) : undefined
      }
      aside={
        data.lastChecked
          ? copy.needs.checked(
              relTime(new Date(data.lastChecked).toISOString(), Math.max(ageNow, data.lastChecked)),
            )
          : undefined
      }
      titleRef={titleRef}
    >
      {allFailed ? (
        <CardError
          what={copy.needs.what}
          onRetry={() => Object.values(data.retry).forEach((r) => r())}
          titleRef={titleRef}
        />
      ) : needs.loading && !needs.rows.length ? (
        <CardSkeleton />
      ) : (
        <div className="flex min-h-0 flex-1 flex-col gap-2">
          {needs.empty ? (
            <div data-testid="needs-empty">
              <EmptyState title={copy.needs.nothing} className="py-6 md:py-6" />
            </div>
          ) : needs.rows.length ? (
            // Five rows show; the list scrolls inside the card after that (design review 2A).
            // Taller on touch, where row actions are 44 px (/ship review): five rows still fit without clipping.
            <ScrollArea className="[&>[data-slot=scroll-area-viewport]]:max-h-70 pointer-coarse:[&>[data-slot=scroll-area-viewport]]:max-h-84 [&>[data-slot=scroll-area-viewport]>div]:block!">
              <ul className="divide-y divide-border pr-2.5" aria-label={copy.needs.title}>
                {needs.rows.map((r) => (
                  <li key={r.key}>
                    <Row row={r} now={now} userId={userId} />
                  </li>
                ))}
              </ul>
            </ScrollArea>
          ) : null}
          {/* Past five rows the list scrolls; say so, a clipped row alone is easy to miss (QA ISSUE-002). */}
          {needs.rows.length > NEEDS_VISIBLE_ROWS ? (
            <p className="text-xs text-muted-foreground" data-testid="needs-more">
              {copy.needs.more(needs.rows.length - NEEDS_VISIBLE_ROWS)}
            </p>
          ) : null}
          {needs.loading && needs.rows.length ? (
            <Skeleton className="h-8 w-full motion-reduce:animate-none" aria-label={copy.loading} />
          ) : null}
          {needs.failed.map((id) => (
            <SourceFailed key={id} what={copy.needs.source[id]} onRetry={data.retry[id]} />
          ))}
        </div>
      )}
    </Card>
  )
}

function Row({ row, now, userId }: { row: NeedRow; now: number; userId: string }) {
  const { Icon, tone, detail: detailTone, label } = ICON[row.severity]
  let title: string
  let detail: string
  let action: ReactNode
  switch (row.kind) {
    case 'request':
      title = copy.needs.request(row.chat.kind, row.chat.count)
      detail = `${row.chat.chatTitle} · ${relTime(row.chat.at, Math.max(now, Date.parse(row.chat.at) || now))}`
      action = (
        <Action>
          <Link
            to="/chat/$sessionId"
            params={{ sessionId: row.chat.sessionId }}
            state={{ waitingRequest: row.chat.firstId } as never}
            aria-label={copy.needs.reviewLabel(row.chat.chatTitle)}
          >
            {copy.needs.review}
          </Link>
        </Action>
      )
      break
    case 'outside':
      title = copy.needs.outside(row.count)
      detail = copy.needs.outsideDetail
      // Chat's Waiting view is the one that explains requests outside Chat: pick it before opening Chat (/ship review).
      action = (
        <Action>
          <Link
            to="/chat"
            onClick={() => rememberRailView(userId, 'waiting')}
            aria-label={copy.needs.openWaiting}
          >
            {copy.needs.openChat}
          </Link>
        </Action>
      )
      break
    case 'agent':
      title = row.name
      detail = row.reason
      action = row.budget ? (
        <Action>
          <Link to="/router" hash="router-budgets" aria-label={copy.needs.budgetsLabel(row.name)}>
            {copy.needs.budgets}
          </Link>
        </Action>
      ) : (
        <Action>
          <Link
            to="/agents/$agentId"
            params={{ agentId: row.id }}
            search={{}}
            aria-label={copy.needs.openAgentLabel(row.name)}
          >
            {copy.needs.openAgent}
          </Link>
        </Action>
      )
      break
    case 'budget':
      title = copy.needs.budget(row.label, row.state, row.crossed)
      detail = copy.needs.budgetDetail
      action = (
        <Action>
          <Link to="/router" hash="router-budgets" aria-label={copy.needs.budgetsLabel(row.label)}>
            {copy.needs.budgets}
          </Link>
        </Action>
      )
      break
    case 'sessions':
      title = copy.needs.sessions(row.failed, row.checked)
      detail = row.agents.join(', ')
      action = (
        <Action>
          <Link
            to="/sessions"
            search={{ ...LAST_WEEK, lane: 'failing' } as never}
            aria-label={copy.needs.seeFailing}
          >
            {copy.needs.seeSessions}
          </Link>
        </Action>
      )
      break
  }
  return (
    <div className="flex items-center gap-3 py-2.5" data-kind={row.kind}>
      {/* The icon sits on a tile of its own tone. */}
      <Icon
        className={cn('box-content size-4 shrink-0 rounded-md bg-current/12 p-1.5', tone)}
        aria-label={label}
        role="img"
      />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-semibold">{title}</p>
        {detail ? <p className={cn('truncate text-xs', detailTone)}>{detail}</p> : null}
      </div>
      {action}
    </div>
  )
}
