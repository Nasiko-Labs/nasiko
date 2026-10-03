/**
 * One session row. The whole row is a real link to the session trace (middle-click opens
 * a tab); expand is a separate chevron button. Status sits in a fixed-width column so rows
 * don't reflow as checks resolve. The columns follow the list's width, not the viewport's (the
 * sidebar takes 240 px): below 672 px the row becomes two lines.
 */
import { Link } from '@tanstack/react-router'
import { m } from 'motion/react'
import { ChevronDown, ScrollText } from 'lucide-react'
import { memo, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { AgentLink } from '@/features/agents/components/AgentLink'
import { OpenChatLink } from '@/features/chat/components/OpenChatLink'
import { copy, STATUS_LABEL } from '@/features/observability/copy'
import { sessionCost, type Status } from '@/features/observability/sessions'
import type { SessionSummary } from '@/features/observability/types'
import { fmtDuration, fmtMoney, fmtTokens, fmtUtcDayTime, fmtUtcTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import { useSessionDetail } from './api'
import type { SessionsSearch } from './search'
import { LogDrawer } from './LogDrawer'

const STATUS_TONE: Record<Status, string> = {
  failed: 'text-destructive',
  ok: 'text-success',
  unchecked: 'text-muted-foreground',
  unknown: 'text-muted-foreground',
  checking: 'text-muted-foreground',
}

function timeLabel(iso: string | null | undefined, withDay: boolean): string {
  if (!iso) return '—'
  return withDay ? fmtUtcDayTime(iso) : fmtUtcTime(iso)
}

/** Memoised: the list re-renders on every status and live update; unchanged rows skip it. */
export const SessionRow = memo(function SessionRow({
  s,
  agentLabel,
  rawName,
  status,
  slow,
  costly,
  withDay,
  expanded,
  onToggle,
  linkSearch,
  fresh,
}: {
  s: SessionSummary
  agentLabel: string
  rawName: string
  status: Status
  slow: boolean
  costly: boolean
  withDay: boolean
  expanded: boolean
  onToggle: (sessionId: string) => void
  linkSearch: Partial<SessionsSearch>
  fresh: boolean
}) {
  const input = s.first_input ?? s.session_id
  const badges = [...(slow ? ['slow' as const] : []), ...(costly ? ['costly' as const] : [])]
  const label = [
    input,
    agentLabel,
    fmtMoney(sessionCost(s)),
    STATUS_LABEL[status],
    timeLabel(s.start_time, withDay),
    fmtDuration(s.duration_ms),
    ...badges,
  ].join(' · ')
  return (
    <m.li
      initial={fresh ? { opacity: 0, y: -8 } : false}
      animate={{ opacity: 1, y: 0 }}
      className="border-b border-border last:border-b-0"
    >
      <div className="group relative flex items-stretch transition-colors hover:bg-muted/50 has-aria-expanded:bg-muted/50">
        <Link
          to="/sessions/$sessionId"
          params={{ sessionId: s.session_id }}
          search={linkSearch}
          className="min-w-0 flex-1 px-2 py-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring @[672px]:grid @[672px]:grid-cols-[5.5rem_10rem_minmax(0,1fr)_5rem_4.5rem_8.5rem] @[672px]:items-center @[672px]:gap-x-3"
          aria-label={label}
        >
          {/* Narrow list: two lines — agent · cost · status, then the input and time. */}
          <span className="flex flex-col gap-0.5 @[672px]:hidden">
            <span className="flex items-baseline gap-2">
              <span className="min-w-0 flex-1 truncate font-medium" title={rawName || undefined}>
                {agentLabel}
              </span>
              <span className="font-medium tabular-nums">{fmtMoney(sessionCost(s))}</span>
              <span className={cn('w-22 text-right text-xs', STATUS_TONE[status])}>
                {STATUS_LABEL[status]}
              </span>
            </span>
            <span className="flex items-baseline gap-2 text-muted-foreground">
              <span className="min-w-0 flex-1 truncate">“{input}”</span>
              <span className="text-xs tabular-nums">{timeLabel(s.start_time, withDay)}</span>
            </span>
          </span>
          <span className="hidden text-muted-foreground tabular-nums @[672px]:inline">
            {timeLabel(s.start_time, withDay)}
          </span>
          <span className="hidden truncate @[672px]:inline" title={rawName || undefined}>
            {agentLabel}
          </span>
          <span className="hidden truncate @[672px]:inline">
            “{input}”
            {badges.length ? (
              <span className="ml-2 inline-flex gap-1 align-middle">
                {badges.map((b) => (
                  <Badge
                    key={b}
                    variant={b === 'slow' ? 'warning' : 'outline'}
                    className={cn(
                      'rounded px-1 text-2xs',
                      // A chart fill is never text (1.3:1 for mint on its tint): the tint is the cue, the word is foreground.
                      b === 'costly' && 'border-chart-4-edge/40 bg-chart-4/15 text-foreground',
                    )}
                  >
                    {b}
                  </Badge>
                ))}
              </span>
            ) : null}
          </span>
          <span className="hidden text-right font-medium tabular-nums @[672px]:inline">
            {fmtMoney(sessionCost(s))}
          </span>
          <span className="hidden text-right text-muted-foreground tabular-nums @[672px]:inline">
            {fmtDuration(s.duration_ms)}
          </span>
          <span className={cn('hidden w-34 @[672px]:inline', STATUS_TONE[status])}>
            {STATUS_LABEL[status]}
          </span>
        </Link>
        <Button
          variant="ghost"
          size="icon"
          onClick={() => onToggle(s.session_id)}
          aria-expanded={expanded}
          aria-label={`Show details for ${input}`}
          className="h-auto w-11 shrink-0 rounded-none text-muted-foreground hover:bg-transparent hover:text-foreground focus-visible:ring-offset-0 dark:hover:bg-transparent"
        >
          <ChevronDown
            className={cn('size-4 transition-transform', expanded && 'rotate-180')}
            aria-hidden
          />
        </Button>
      </div>
      {expanded ? <RowDetail s={s} agentLabel={agentLabel} /> : null}
    </m.li>
  )
})

/** Lazy: the list has only total tokens; the split comes from SessionDetail. */
function RowDetail({ s, agentLabel }: { s: SessionSummary; agentLabel: string }) {
  const d = useSessionDetail(s.session_id)
  const [logs, setLogs] = useState(false)
  return (
    <div className="grid gap-2 bg-muted/40 px-3 py-3 text-sm @[672px]:grid-cols-3">
      <div className="@[672px]:col-span-2">
        <div className="text-xs text-muted-foreground">Last output</div>
        <div className="line-clamp-3">{s.last_output ?? '—'}</div>
      </div>
      <div className="flex flex-col gap-1">
        <div className="text-xs text-muted-foreground">Tokens · traces</div>
        {d.isPending ? (
          <span className="text-muted-foreground">Loading…</span>
        ) : d.isError ? (
          <span className="text-muted-foreground">{copy.notFound}</span>
        ) : (
          <span className="tabular-nums">
            {fmtTokens(d.data.cost_summary.prompt.tokens)} in ·{' '}
            {fmtTokens(d.data.cost_summary.completion.tokens)} out · {d.data.num_traces} trace
            {d.data.num_traces === 1 ? '' : 's'}
          </span>
        )}
        {s.agent_id ? (
          <>
            <Button
              size="sm"
              variant="outline"
              className="mt-1 w-fit"
              onClick={() => setLogs(true)}
            >
              <ScrollText className="size-3.5" aria-hidden /> View {agentLabel} logs
            </Button>
            <LogDrawer agent={s.agent_id} label={agentLabel} open={logs} onOpenChange={setLogs} />
            <AgentLink
              name={s.agent_id}
              fallback={null}
              className="w-fit text-xs text-primary-text"
            >
              Open {agentLabel}
            </AgentLink>
          </>
        ) : null}
        {/* Only while this detail is open (v1c §5.10). */}
        <OpenChatLink sessionId={s.session_id} className="mt-1 w-fit" />
      </div>
    </div>
  )
}
