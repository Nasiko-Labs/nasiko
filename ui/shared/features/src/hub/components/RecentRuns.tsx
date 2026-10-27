/**
 * Recent runs (plans/feat-agent-centric.md PR1c; CEO R2-5, R2-7; eng "Hub sessions", "Flows list"): two short lists,
 * never merged. Sessions come from the hub's bounded sample (last 7 days, the latest N across the fleet filtered to this
 * agent), so an empty list says what it checked and links to Sessions, never a bare "No sessions". Flows are the ones
 * this agent started (the list's `root_agent_id`); flows that only call it need an agent filter on `/api/flows` (FL-12).
 * Side by side from `lg`, stacked below. Each row opens the shipped zoom: a session's trace, a flow's page. A harness
 * owner sees their own recorded sessions with it instead of the sample, each opening its ledger (PR1b).
 */
import { Link } from '@tanstack/react-router'
import { Panel, PanelSkeleton } from '@nasiko/ui/composites/panel'
import { Badge } from '@nasiko/ui/primitives/badge'
import { useAgentsDirectory } from '@nasiko/features/agents/api'
import { relTime } from '@nasiko/features/agents/format'
import { fmtUtcDayTime } from '@nasiko/ui/lib/format'
import type { useHarnessChats, useHubFlows, useHubSessions } from '../api'
import { copy } from '../copy'
import { RetryLine } from './RetryLine'
import { RUNS_SHOWN } from '../tuning'

const LINK =
  'inline-flex items-center text-sm text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11'

export function RecentRuns({
  agentName,
  harnessChats,
  sessions,
  statuses,
  flows,
}: {
  agentName: string
  /** A harness owner's own recorded sessions (PR1b): listed instead of the sample, each opening its ledger. */
  harnessChats?: ReturnType<typeof useHarnessChats>
  sessions: ReturnType<typeof useHubSessions>
  statuses: ReadonlyMap<string, string>
  flows: ReturnType<typeof useHubFlows>
}) {
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      {harnessChats ? (
        <HarnessSessions agentName={agentName} chats={harnessChats} />
      ) : (
        <SessionsList agentName={agentName} sessions={sessions} statuses={statuses} />
      )}
      <FlowsList flows={flows} />
    </div>
  )
}

function SessionsList({
  agentName,
  sessions,
  statuses,
}: {
  agentName: string
  sessions: ReturnType<typeof useHubSessions>
  statuses: ReadonlyMap<string, string>
}) {
  const dir = useAgentsDirectory()
  // Sessions name agents by raw name: another visible agent with it makes the match ambiguous (eng), never merged.
  const shared = (dir.byNameAll.get(agentName)?.length ?? 0) > 1
  const all = (
    <Link to="/sessions" search={{ agent: agentName, preset: '7d' }} className={LINK}>
      {copy.runs.sessionsAll(agentName)}
    </Link>
  )
  return (
    <Panel title={copy.runs.sessions} subtitle={copy.runs.sessionsWindow}>
      {sessions.isPending ? (
        <PanelSkeleton height={96} />
      ) : sessions.isError ? (
        <RetryLine label={copy.runs.sessionsError} onRetry={() => void sessions.refetch()} />
      ) : (
        <div className="space-y-2">
          {shared ? (
            <p className="m-0 text-xs text-muted-foreground">{copy.runs.sessionsShared}</p>
          ) : null}
          {sessions.data.rows.length ? (
            <ul className="m-0 list-none divide-y divide-border p-0 text-sm">
              {sessions.data.rows.slice(0, RUNS_SHOWN).map((s) => (
                <li
                  key={s.session_id}
                  className="flex flex-wrap items-center justify-between gap-2 py-1.5"
                >
                  <Link
                    to="/sessions/$sessionId"
                    params={{ sessionId: s.session_id }}
                    search={{}}
                    className="inline-flex items-center font-mono text-xs underline-offset-4 hover:underline pointer-coarse:min-h-11"
                  >
                    {s.session_id.slice(0, 8)}
                  </Link>
                  <span className="flex items-center gap-2 text-xs text-muted-foreground">
                    {statuses.get(s.session_id) === 'failed' ? (
                      <Badge variant="destructive">{copy.runs.failed}</Badge>
                    ) : null}
                    {relTime(s.start_time)}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="m-0 text-sm text-muted-foreground">
              {copy.runs.sessionsNone(sessions.data.scanned)}
            </p>
          )}
          <p className="m-0">{all}</p>
        </div>
      )}
    </Panel>
  )
}

function HarnessSessions({
  agentName,
  chats,
}: {
  agentName: string
  chats: ReturnType<typeof useHarnessChats>
}) {
  return (
    <Panel title={copy.runs.sessions} subtitle={copy.runs.harnessWindow}>
      {chats.isPending ? (
        <PanelSkeleton height={96} />
      ) : chats.isError ? (
        <RetryLine label={copy.runs.sessionsError} onRetry={() => void chats.refetch()} />
      ) : (
        <div className="space-y-2">
          {chats.data.length ? (
            <ul className="m-0 list-none divide-y divide-border p-0 text-sm">
              {chats.data.slice(0, RUNS_SHOWN).map((s) => (
                <li
                  key={s.session_id}
                  className="flex flex-wrap items-center justify-between gap-2 py-1.5"
                >
                  {/* Every recorded harness session has the server's one title: the row is its time and size. */}
                  <Link
                    to="/sessions/$sessionId"
                    params={{ sessionId: s.session_id }}
                    search={{ view: 'ledger' }}
                    className="inline-flex min-w-0 items-center truncate underline-offset-4 hover:underline pointer-coarse:min-h-11"
                  >
                    {copy.runs.harnessRow(fmtUtcDayTime(s.created_at), s.message_count)}
                  </Link>
                  <span className="text-xs text-muted-foreground">
                    {relTime(s.updated_at ?? s.created_at)}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="m-0 text-sm text-muted-foreground">{copy.runs.harnessNone}</p>
          )}
          <p className="m-0">
            <Link to="/sessions" search={{ agent: agentName, preset: '7d' }} className={LINK}>
              {copy.runs.sessionsAll(agentName)}
            </Link>
          </p>
        </div>
      )}
    </Panel>
  )
}

function FlowsList({ flows }: { flows: ReturnType<typeof useHubFlows> }) {
  return (
    <Panel title={copy.runs.flows}>
      {flows.isPending ? (
        <PanelSkeleton height={96} />
      ) : flows.isError ? (
        <RetryLine label={copy.runs.flowsError} onRetry={() => void flows.refetch()} />
      ) : (
        <div className="space-y-2">
          {flows.mine.length ? (
            <ul className="m-0 list-none divide-y divide-border p-0 text-sm">
              {flows.mine.slice(0, RUNS_SHOWN).map((f) => (
                <li
                  key={f.flow_id}
                  className="flex flex-wrap items-center justify-between gap-2 py-1.5"
                >
                  <Link
                    to="/flows/$flowId"
                    params={{ flowId: f.flow_id }}
                    className="min-w-0 truncate underline-offset-4 hover:underline pointer-coarse:py-3"
                  >
                    {f.title || f.flow_id.slice(0, 8)}
                  </Link>
                  <span className="flex items-center gap-2 text-xs text-muted-foreground">
                    <Badge variant={f.status === 'failed' ? 'destructive' : 'outline'}>
                      {f.status}
                    </Badge>
                    {relTime(f.created_at)}
                  </span>
                </li>
              ))}
            </ul>
          ) : flows.partial ? null : (
            <p className="m-0 text-sm text-muted-foreground">
              {flows.complete ? copy.runs.flowsNone : copy.runs.flowsNoneCapped(flows.read)}
            </p>
          )}
          {flows.partial ? (
            <RetryLine label={copy.runs.flowsPartial} onRetry={() => void flows.refetch()} />
          ) : null}
          <p className="m-0">
            <Link to="/flows" className={LINK}>
              {copy.runs.flowsAll}
            </Link>
          </p>
        </div>
      )}
    </Panel>
  )
}
