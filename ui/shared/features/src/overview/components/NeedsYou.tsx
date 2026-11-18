/**
 * Needs your attention (plans/feat-overview.md §6; "Needs you" until 2026-11-05): the inbox, most urgent first.
 * Severity is said once, by a plain icon beside the text (design review 9A); five rows show and the rest scroll inside
 * the card (2A); the header always says when it last checked (8A); a failed source adds a line and never turns into
 * "Nothing needs your attention" (5A).
 *
 * 2026-11-05: an all-clear names what it checked (one line per source that answered), so an empty inbox reads as
 * "watched and fine", not as a blank card; and the setup items (LLM router, workflows, MCP servers) sit under the
 * alerts in their own group, always: one not set up yet is a step to take, one that is says how many exist and links
 * to them. They are suggestions, never alerts: no severity, no "failed" line.
 */
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { OpenFlowLink } from '@nasiko/features/flows/components/OpenFlowLink'
import { Link } from '@tanstack/react-router'
import {
  Check,
  CircleAlert,
  CircleCheck,
  Hand,
  Plug,
  TriangleAlert,
  Waypoints,
  Workflow,
} from 'lucide-react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { withButtonStyle } from '@nasiko/ui/primitives/button'
import { ScrollArea } from '@nasiko/ui/primitives/scroll-area'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { relTime } from '@nasiko/features/agents/format'
import { rememberRailView } from '@nasiko/features/chat/rememberTarget'
import { cn } from '@nasiko/ui/lib/utils'
import type { NeedsYou as NeedsYouData } from '../api'
import { copy } from '../copy'
import {
  budgetView,
  type NeedRow,
  type SetupId,
  type SetupItem,
  type Severity,
  type SourceId,
} from '../needs'
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

/**
 * One row of the inbox, alerts and setup items alike: hairline rows at every width, as Fleet health beside it lists
 * agents (user 2026-11-05: tiles read boxy for these; they were three across from 900 px of card before).
 */
const ROW_ITEM = 'border-t border-border first:border-t-0'

export function NeedsYou({
  data,
  now,
  userId,
  pausedFlows,
  setup = [],
  watch = null,
  className,
}: {
  data: NeedsYouData
  now: number
  userId: string
  /** Chat session → its paused flow (plans/feat-flows.md O2, amended 2026-10-06): a request row links it. */
  pausedFlows?: ReadonlyMap<string, string>
  /** The setup items under the alerts (`useSetupSteps`). */
  setup?: readonly SetupItem[]
  /** Fleet health's Watch agents (`watchSummary`): one row when the inbox is otherwise empty. */
  watch?: { count: number; detail: string } | null
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
  const setupLeft = setup.filter((i) => !i.count).length
  // Watch agents are promoted only into an inbox with nothing else in it (user 2026-11-05).
  const watchRow: NeedRow | null =
    needs.empty && watch ? { key: 'watch', kind: 'watch', severity: 'watch', ...watch } : null
  const count = { action: 0, watch: watchRow ? 1 : 0, waiting: 0 }
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
        needs.rows.length || watchRow || setupLeft ? (
          <span className="inline-flex gap-3 font-medium">
            {(['action', 'watch', 'waiting'] as const).map((s) =>
              count[s] ? (
                <span key={s} className={ICON[s].tone}>
                  {/* The promoted row counts its agents, not itself. */}
                  {copy.needs.count[s](s === 'watch' && watchRow ? (watch?.count ?? 0) : count[s])}
                </span>
              ) : null,
            )}
            {setupLeft ? (
              <span className="font-normal">{copy.needs.setupCount(setupLeft)}</span>
            ) : null}
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
            <>
              <AllClear checked={needs.checked} urgentOnly={!!watchRow || setupLeft > 0} />
              {watchRow ? (
                <div className="border-t border-border pr-2.5">
                  <NeedRowView row={watchRow} now={now} userId={userId} />
                </div>
              ) : null}
            </>
          ) : needs.rows.length ? (
            // Five rows show; the list scrolls inside the card after that (design review 2A).
            // Taller on touch, where row actions are 44 px (/ship review): five rows still fit without clipping.
            <ScrollArea className="[&>[data-slot=scroll-area-viewport]]:max-h-70 pointer-coarse:[&>[data-slot=scroll-area-viewport]]:max-h-84 [&>[data-slot=scroll-area-viewport]>div]:block!">
              <ul className="pr-2.5" aria-label={copy.needs.title}>
                {needs.rows.map((r) => (
                  <li key={r.key} className={ROW_ITEM}>
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
          {setup.length ? <SetupSteps items={setup} /> : null}
        </div>
      )}
    </Card>
  )
}

/** Nothing needs the viewer: say so, then what was checked, one line per source that answered. */
function AllClear({
  checked,
  urgentOnly,
}: {
  checked: readonly SourceId[]
  /** Agents to watch or setup steps are listed below: "nothing urgent", so the sentence doesn't contradict them. */
  urgentOnly: boolean
}) {
  return (
    <div data-testid="needs-empty" className="flex items-start gap-3 py-2.5">
      <CircleCheck
        aria-hidden
        className="box-content size-4 shrink-0 rounded-md bg-current/12 p-1.5 text-success"
      />
      <div className="flex min-w-0 flex-col gap-1.5">
        <p className="text-sm font-semibold">
          {urgentOnly ? copy.needs.nothingUrgent : copy.needs.nothing}
        </p>
        <ul className="flex flex-wrap gap-x-5 gap-y-1 text-xs text-muted-foreground">
          {checked.map((id) => (
            <li key={id} className="inline-flex items-center gap-1.5">
              <Check aria-hidden className="size-3.5 text-success" />
              {copy.needs.clear[id]}
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}

/** Where each item's action goes: a step starts the thing, a set-up item opens where it's managed. */
const SETUP: Record<
  SetupId,
  {
    Icon: typeof Plug
    to: '/router' | '/workflows/new' | '/mcp'
    manage: '/router' | '/workflows' | '/mcp'
  }
> = {
  router: { Icon: Waypoints, to: '/router', manage: '/router' },
  workflows: { Icon: Workflow, to: '/workflows/new', manage: '/workflows' },
  mcp: { Icon: Plug, to: '/mcp', manage: '/mcp' },
}

/**
 * The setup items under the alerts, always in the same order so nothing jumps when one gets set up. A step: a neutral
 * icon (no severity), what it gives, and the one action. Set up: the icon in the success tone with a check, how many
 * exist, and where to manage them.
 */
function SetupSteps({ items }: { items: readonly SetupItem[] }) {
  return (
    <section
      aria-labelledby="needs-setup"
      data-testid="needs-setup"
      className="mt-1 flex flex-col gap-1 border-t border-border pt-3"
    >
      <h3 id="needs-setup" className="text-xs font-medium text-muted-foreground">
        {copy.needs.setupTitle}
      </h3>
      {/* The alerts' scrollbar gutter, so both groups' buttons end on one edge. */}
      <ul className="pr-2.5">
        {items.map(({ id, count, more }) => {
          const { Icon, to, manage } = SETUP[id]
          const c = copy.needs.setup[id]
          const done = count > 0
          return (
            <li key={id} data-kind="setup" data-done={done} className={ROW_ITEM}>
              <div className="flex items-center gap-3 py-2.5">
                <div className="flex min-w-0 flex-1 items-center gap-3">
                  <span aria-hidden className="relative shrink-0">
                    <Icon
                      className={cn(
                        'box-content size-4 rounded-md p-1.5',
                        done ? 'bg-success/12 text-success' : 'bg-muted text-muted-foreground',
                      )}
                    />
                    {done ? (
                      <Check className="absolute -right-1 -bottom-1 size-3 rounded-full bg-success p-px text-background" />
                    ) : null}
                  </span>
                  <div className="min-w-0 flex-1">
                    <p id={`setup-${id}`} className="truncate text-sm font-semibold">
                      {done ? c.doneTitle : c.title}
                    </p>
                    <p className="line-clamp-2 text-xs text-muted-foreground">
                      {done
                        ? c.done(more ? `${count}+` : String(count), count === 1 && !more)
                        : c.detail}
                    </p>
                  </div>
                </div>
                <Action>
                  <Link
                    to={done ? manage : to}
                    search={{} as never}
                    aria-describedby={`setup-${id}`}
                  >
                    {done ? c.manage : c.action}
                  </Link>
                </Action>
              </div>
            </li>
          )
        })}
      </ul>
    </section>
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
    case 'watch':
      title = copy.needs.watch(row.count)
      detail = row.detail
      action = (
        <Action>
          <Link
            to="/agents"
            search={{ health: 'watch' } as never}
            aria-label={copy.needs.reviewWatchLabel}
          >
            {copy.needs.reviewWatch}
          </Link>
        </Action>
      )
      break
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
