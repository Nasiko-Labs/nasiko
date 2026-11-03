/**
 * Needs you (plans/feat-overview.md §6): the inbox, most urgent first. Severity is said once, by a plain icon beside
 * the text (design review 9A); five rows show and the rest scroll inside the card (2A); the header always says when
 * it last checked (8A); a failed source adds a line and never turns into "Nothing needs you" (5A).
 */
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { OpenFlowLink } from '@nasiko/features/flows/components/OpenFlowLink'
import { Link } from '@tanstack/react-router'
import { CircleAlert, Hand, TriangleAlert } from 'lucide-react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { EmptyState } from '@nasiko/ui/composites/state-card'
import { withButtonStyle } from '@nasiko/ui/primitives/button'
import { ScrollArea } from '@nasiko/ui/primitives/scroll-area'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { relTime } from '@nasiko/features/agents/format'
import { rememberRailView } from '@nasiko/features/chat/rememberTarget'
import { cn } from '@nasiko/ui/lib/utils'
import type { NeedsYou as NeedsYouData } from '../api'
import { copy } from '../copy'
import { budgetView, type NeedRow, type Severity } from '../needs'
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
  return withButtonStyle(children, {
    variant: 'outline',
    size: 'sm',
    className: cn('shrink-0 text-xs', TOUCH),
  })
}

export function NeedsYou({
  data,
  now,
  userId,
  pausedFlows,
  className,
}: {
  data: NeedsYouData
  now: number
  userId: string
  /** Chat session → its paused flow (plans/feat-flows.md O2, amended 2026-10-06): a request row links it. */
  pausedFlows?: ReadonlyMap<string, string>
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
              {/* Full width on the Overview: from 900 px of card the rows are tiles three across, so a short list never
                  leaves the card's lower half empty. Narrower they are hairline rows. */}
              <ul
                className="grid grid-cols-1 gap-x-3 pr-2.5 @[900px]/card:grid-cols-3 @[900px]/card:gap-y-3"
                aria-label={copy.needs.title}
              >
                {needs.rows.map((r) => (
                  <li
                    key={r.key}
                    className="border-t border-border first:border-t-0 @[900px]/card:rounded-lg @[900px]/card:border @[900px]/card:bg-muted/20 @[900px]/card:px-3 @[900px]/card:first:border-t"
                  >
                    <NeedRowView
                      row={r}
                      now={now}
                      userId={userId}
                      flowId={r.kind === 'request' ? pausedFlows?.get(r.chat.sessionId) : undefined}
                    />
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

/** One Needs you row: its icon, what it is, and its one action (the insight panel lists requests with it too). */
export function NeedRowView({
  row,
  now,
  userId,
  flowId,
}: {
  row: NeedRow
  now: number
  userId: string
  /** The request's paused flow, when one matches its chat. */
  flowId?: string
}) {
  const { Icon, tone, detail: detailTone, label } = ICON[row.severity]
  // Budgets are Enterprise only: an OSS build has no `/budgets` route to link a budget row to.
  const BudgetsLink = useSlots().budgets.NeedsLink
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
          <Link
            to="/agents/$agentId"
            params={{ agentId: row.id }}
            search={{}}
            hash="hub-budget"
            aria-label={copy.needs.agentBudgetLabel(row.name)}
          >
            {copy.needs.agentBudget}
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
    case 'budget': {
      const v = budgetView(row)
      title = v.title
      detail = v.detail
      action = row.agentId ? (
        <Action>
          <Link
            to="/agents/$agentId"
            params={{ agentId: row.agentId }}
            search={{}}
            hash="hub-budget"
            aria-label={copy.needs.agentBudgetLabel(row.label)}
          >
            {copy.needs.agentBudget}
          </Link>
        </Action>
      ) : BudgetsLink ? (
        // Budgets are Enterprise only, and so is `/budgets`: the core cannot type a link to a route it has no
        // routes for, and a budget row only ever reaches an OSS build if a layer put it there.
        <Action>
          <BudgetsLink label={copy.needs.budgets} ariaLabel={copy.needs.budgetsLabel(row.label)} />
        </Action>
      ) : null
      break
    }
    case 'sessions':
      title = copy.needs.sessions(row.failed)
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
      {flowId ? (
        <OpenFlowLink flowId={flowId} variant="ghost" className="shrink-0 text-xs" />
      ) : null}
      {action}
    </div>
  )
}
