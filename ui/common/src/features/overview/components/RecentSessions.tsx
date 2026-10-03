/**
 * Recent sessions (plans/feat-overview.md §9): the five newest of the last 7 days, failures first, with the status marks
 * from the same checks Needs you uses (the newest 25, eng review R7). Tempo errors say the trace store is needed.
 */
import { Link } from '@tanstack/react-router'
import { CircleAlert, CircleCheck, CircleDashed } from 'lucide-react'
import { useRef } from 'react'
import { EmptyState } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { relTime } from '@/features/agents/format'
import { sessionCost, type Status } from '@/features/observability/sessions'
import { fmtMoney } from '@/lib/format'
import { ApiError } from '@/lib/api/client'
import type { NeedsYou } from '../api'
import { copy } from '../copy'
import { LAST_WEEK, RECENT_SESSION_ROWS } from '../tuning'
import { Card, CardError, CardSkeleton, TOUCH } from './Card'

const MARK: Partial<Record<Status, { Icon: typeof CircleCheck; tone: string; label: string }>> = {
  failed: { Icon: CircleAlert, tone: 'text-destructive', label: copy.sessions.failed },
  ok: { Icon: CircleCheck, tone: 'text-success', label: copy.sessions.ok },
}
const TRACE_STORE = new Set([500, 502, 503])

export function RecentSessions({
  sessions,
  displayName,
  now,
  className,
}: {
  sessions: NeedsYou['sessions']
  displayName: (raw: string) => string
  now: number
  className?: string
}) {
  const titleRef = useRef<HTMLHeadingElement>(null)
  const { list, newest, status } = sessions
  const failedFirst = [...newest].sort(
    (a, b) =>
      Number(status.get(b.session_id) === 'failed') - Number(status.get(a.session_id) === 'failed'),
  )
  const rows = failedFirst.slice(0, RECENT_SESSION_ROWS)
  // A bare 502 is the proxy with the server down (StateCard's rule), not the trace store (/ship review).
  const serverDown =
    list.error instanceof ApiError && list.error.status === 502 && !list.error.serverMessage
  const traceStore =
    list.error instanceof ApiError && TRACE_STORE.has(list.error.status) && !serverDown
  return (
    <Card
      id="overview-sessions"
      title={copy.sessions.title}
      to="/sessions"
      linkLabel={copy.sessions.link}
      linkSearch={LAST_WEEK}
      titleRef={titleRef}
      className={className}
    >
      {list.isPending ? (
        <CardSkeleton rows={5} />
      ) : traceStore ? (
        <p className="text-sm">
          {copy.sessions.traceStore} <OpenSessions />
        </p>
      ) : list.isError ? (
        <CardError
          what={copy.sessions.what}
          onRetry={() => void list.refetch()}
          titleRef={titleRef}
        />
      ) : !rows.length ? (
        <EmptyState title={copy.sessions.none} action={<OpenSessions />} className="py-6 md:py-6" />
      ) : (
        <Table className="table-fixed text-xs">
          <TableHeader>
            <TableRow className="hover:bg-transparent">
              <TableHead className="h-8 w-6 px-0">
                <span className="sr-only">{copy.sessions.colStatus}</span>
              </TableHead>
              <TableHead className="h-8 px-1 text-xs font-medium text-muted-foreground">
                {copy.sessions.colSession}
              </TableHead>
              {/* Shown when the card itself has room (/ship review), not when the page does. */}
              <TableHead className="hidden h-8 w-[28%] px-1 text-xs font-medium text-muted-foreground @[360px]/card:table-cell">
                {copy.sessions.colAgent}
              </TableHead>
              <TableHead className="h-8 w-20 px-1 text-right text-xs font-medium text-muted-foreground">
                {copy.sessions.colCost}
              </TableHead>
              <TableHead className="h-8 w-24 px-1 text-right text-xs font-medium text-muted-foreground">
                {copy.sessions.colWhen}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((s) => {
              const st = status.get(s.session_id) ?? 'unchecked'
              const mark = MARK[st] ?? {
                Icon: CircleDashed,
                tone: 'text-muted-foreground',
                label: copy.sessions.unchecked,
              }
              const cost = sessionCost(s)
              return (
                <TableRow key={s.session_id} data-testid="recent-session">
                  <TableCell className="h-8 px-0 pointer-coarse:h-11">
                    <mark.Icon
                      className={`size-4 ${mark.tone}`}
                      role="img"
                      aria-label={mark.label}
                    />
                  </TableCell>
                  <TableCell className="truncate px-1 font-medium">
                    <Link
                      to="/sessions/$sessionId"
                      params={{ sessionId: s.session_id }}
                      search={LAST_WEEK as never}
                      className="underline-offset-4 hover:underline"
                    >
                      {s.first_input || s.session_id}
                    </Link>
                  </TableCell>
                  <TableCell className="hidden truncate px-1 text-muted-foreground @[360px]/card:table-cell">
                    {s.agent_id ? displayName(s.agent_id) : copy.needs.unknownAgent}
                  </TableCell>
                  <TableCell className="truncate px-1 text-right text-muted-foreground tabular-nums">
                    {cost === null ? '—' : fmtMoney(cost)}
                  </TableCell>
                  <TableCell className="px-1 text-right text-muted-foreground tabular-nums">
                    {relTime(s.start_time, Math.max(now, Date.parse(s.start_time ?? '') || now))}
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      )}
    </Card>
  )
}

function OpenSessions() {
  return (
    <Button asChild variant="link" size="sm" className={`h-auto px-0 ${TOUCH}`}>
      <Link to="/sessions" search={LAST_WEEK as never}>
        {copy.sessions.open}
      </Link>
    </Button>
  )
}
