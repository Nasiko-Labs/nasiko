/**
 * Where a bucket's day leads (design "Bucket selection", DX "Sessions destinations", eng "Drill-down identity"):
 * "Sessions on this day" (the UTC day, also for an hourly bucket: no hour filter is promised) while the day is inside
 * Sessions' 7 days, else "TokenOps for <date>" with the note. Sessions match agents by raw name, so when another
 * visible agent shares this one's name the link says so instead of pretending to filter to this agent.
 */
import { Link } from '@tanstack/react-router'
import { useAgentsDirectory } from '@nasiko/features/agents/api'
import { fmtShortDay } from '@nasiko/ui/lib/format'
import { copy } from '../copy'
import { dayOf, withinSessions, type HubRow } from '../timeline'
import { SESSIONS_DAYS } from '../tuning'

const LINK = 'text-sm text-primary-text underline-offset-4 hover:underline'

export function DayLinks({
  row,
  agentId,
  agentName,
  now,
}: {
  row: HubRow
  agentId: string
  agentName: string
  now: Date
}) {
  const dir = useAgentsDirectory()
  const day = dayOf(row)
  const shared = (dir.byNameAll.get(agentName) ?? []).length
  if (!withinSessions(row, now, SESSIONS_DAYS))
    return (
      <p className="m-0 space-y-1">
        <Link to="/tokenops" search={{ day, agent: agentId }} className={LINK}>
          {copy.bucket.tokenopsDay(fmtShortDay(new Date(`${day}T00:00:00Z`)))}
        </Link>
        <span className="block text-xs text-muted-foreground">
          {copy.bucket.olderNote(SESSIONS_DAYS)}
        </span>
      </p>
    )
  return (
    <p className="m-0">
      <Link to="/sessions" search={{ day, agent: agentName }} className={LINK}>
        {shared > 1
          ? copy.bucket.sessionsShared(agentName, shared)
          : row.size === 'week'
            ? copy.bucket.sessionsOn(fmtShortDay(new Date(`${day}T00:00:00Z`)))
            : copy.bucket.sessionsDay}
      </Link>
    </p>
  )
}
