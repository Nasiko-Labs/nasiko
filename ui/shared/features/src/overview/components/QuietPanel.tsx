/**
 * The summary's quiet tiles (2026-11-05), merged into one panel: each says what would fill it and links there, never a
 * bare zero. The panel takes the room the tiles left: one tile's cell, the row under the tiles that kept a figure, or
 * the whole 2×2 when none did, where its cells stack, so each is about a tile's height beside the lead card. Cells sit
 * on hairlines in one card, as the first run's "Works before your first agent" panel does.
 */
import { Link } from '@tanstack/react-router'
import {
  Activity,
  ArrowRight,
  Coins,
  DollarSign,
  SquareTerminal,
  Workflow,
  type LucideIcon,
} from 'lucide-react'
import { asButton } from '@nasiko/ui/primitives/button'
import { Card } from '@nasiko/ui/primitives/card'
import { fmtMoney } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import type { QuietItem } from '../quiet'
import { LAST_WEEK } from '../tuning'
import { TOUCH } from './Card'
import { PreviewSlot, WatchPreview } from './HarnessPreview'

const q = copy.quiet

interface View {
  Icon: LucideIcon
  label: string
  title: string
  detail: string
  action: string
  to: '/chat' | '/tokenops' | '/sessions' | '/harnesses'
  search?: Record<string, unknown>
}

function view(item: QuietItem, days: number): View {
  switch (item.id) {
    case 'activity':
      return {
        Icon: Activity,
        label: q.activityLabel,
        title: q.activity(days),
        detail: item.before ? q.activityBefore(fmtMoney(item.before), days) : q.activityDetail,
        action: q.startChat,
        to: '/chat',
      }
    case 'spend':
      return {
        Icon: DollarSign,
        label: copy.kpi.spend,
        title: item.unpriced ? q.spendUnpriced : q.spendNone(days),
        detail: item.unpriced ? q.spendUnpricedDetail : q.spendNoneDetail,
        action: q.seeTokenops,
        to: '/tokenops',
      }
    case 'tokens':
      return {
        Icon: Coins,
        label: copy.kpi.tokens,
        title: q.tokens,
        detail: q.tokensDetail,
        action: q.seeSessions,
        to: '/sessions',
        search: LAST_WEEK,
      }
    case 'runs':
      return {
        Icon: Workflow,
        label: copy.kpi.runs,
        title: q.runs(days),
        detail: q.runsDetail,
        action: q.startChat,
        to: '/chat',
      }
    case 'harnesses':
      return {
        Icon: SquareTerminal,
        label: copy.kpi.harnesses,
        title: item.connected ? q.harnessesIdle(days) : q.harnessesNone,
        detail: item.connected ? q.harnessesIdleDetail(item.connected) : q.harnessesNoneDetail,
        action: item.connected ? q.openHarnesses : q.connect,
        to: '/harnesses',
      }
  }
}

function QuietCell({
  item,
  days,
  className,
}: {
  item: QuietItem
  days: number
  className?: string
}) {
  const v = view(item, days)
  // None connected: a preview of what connecting brings, played on Home in place, on sample data (2026-11-06).
  const preview = item.id === 'harnesses' && !item.connected
  const body = (
    <>
      <div className="flex flex-col gap-1">
        <h3 className="text-base font-medium text-pretty">{v.title}</h3>
        <p className="max-w-prose text-sm text-pretty text-muted-foreground">{v.detail}</p>
      </div>
      <div className="mt-auto flex flex-wrap gap-2">
        {preview ? <WatchPreview /> : null}
        <Link
          to={v.to}
          search={(v.search ?? {}) as never}
          {...asButton({ variant: 'outline', size: 'sm', className: TOUCH })}
        >
          {v.action} <ArrowRight aria-hidden />
        </Link>
      </div>
    </>
  )
  return (
    <div
      data-testid={`quiet-${item.id}`}
      className={cn('relative flex flex-col items-start gap-3 bg-card p-4', className)}
    >
      <p className="flex items-center gap-2 text-sm text-muted-foreground">
        <span
          aria-hidden
          className="flex size-7 shrink-0 items-center justify-center rounded-md bg-muted"
        >
          <v.Icon className="size-4" />
        </span>
        {v.label}
      </p>
      {preview ? <PreviewSlot>{body}</PreviewSlot> : body}
    </div>
  )
}

export function QuietPanel({
  items,
  days,
  stacked,
  className,
}: {
  items: readonly QuietItem[]
  days: number
  /** The panel fills the 2×2 (no tile has a figure): one cell per row, as tall as the lead card beside it. */
  stacked: boolean
  className?: string
}) {
  return (
    // The columns are on the inner grid: a container query matches the card's children, never the card itself.
    <Card
      data-testid="overview-quiet"
      className={cn('@container/quiet overflow-hidden p-0', className)}
    >
      <div
        className={cn(
          'grid flex-1 gap-px bg-border',
          !stacked && items.length === 2 && '@[520px]/quiet:grid-cols-2',
          !stacked && items.length >= 3 && '@[520px]/quiet:grid-cols-2 @[760px]/quiet:grid-cols-3',
        )}
      >
        {items.map((item, i) => (
          <QuietCell
            key={item.id}
            item={item}
            days={days}
            // Three cells two across: the third takes the row, so no hairline hole shows.
            className={
              i === 2 && !stacked
                ? '@[520px]/quiet:col-span-2 @[760px]/quiet:col-span-1'
                : undefined
            }
          />
        ))}
      </div>
    </Card>
  )
}
