/**
 * Needs you for this agent (design "Needs you on the hub"): a row list inside one Panel, no nested cards, the crash card
 * not repeated. The verdict reads "No issues found in checked data" with a coverage line; a failed source shows its own
 * line with Retry. Rules in `needs.ts`.
 */
import { Link } from '@tanstack/react-router'
import { Panel, PanelSkeleton } from '@nasiko/ui/composites/panel'
import { Badge } from '@nasiko/ui/primitives/badge'
import { copy } from '../copy'
import type { HubNeedsResult } from '../api'
import type { NeedRow } from '../needs'
import { RetryLine } from './RetryLine'

const LINK =
  'inline-flex items-center text-sm text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11'

export function NeedsPanel({ result, agentName }: { result: HubNeedsResult; agentName: string }) {
  const { needs, retry } = result
  const parts: string[] = []
  if (needs.coverage?.requests) parts.push(copy.needs.partRequests)
  if (needs.coverage?.status) parts.push(copy.needs.partStatus)
  if (needs.coverage?.budget) parts.push(copy.needs.partBudget)
  const s = needs.coverage?.sessions
  if (s) parts.push(copy.needs.partSessions(s.checked, s.found, s.scanned))
  return (
    <Panel title={copy.needs.title}>
      {needs.loading && !needs.rows.length ? (
        <PanelSkeleton height={48} />
      ) : (
        <div className="space-y-2 text-sm">
          {needs.rows.length ? (
            <ul className="m-0 list-none divide-y divide-border p-0">
              {needs.rows.map((r) => (
                <Row key={r.kind} row={r} agentName={agentName} />
              ))}
            </ul>
          ) : needs.clear ? (
            <p className="m-0 font-medium">{copy.needs.clear}</p>
          ) : null}
          {needs.failed.map((f) => (
            <RetryLine
              key={f}
              label={
                f === 'status' && needs.statusIncomplete
                  ? copy.needs.failed.statusIncomplete
                  : copy.needs.failed[f]
              }
              onRetry={retry[f]}
            />
          ))}
          {needs.unproven ? (
            <p className="m-0 text-xs text-muted-foreground">
              {copy.needs.unproven(needs.unproven)}
            </p>
          ) : null}
          {parts.length ? (
            <p className="m-0 text-xs text-muted-foreground">{copy.needs.checked(parts)}</p>
          ) : null}
        </div>
      )}
    </Panel>
  )
}

function Row({ row, agentName }: { row: NeedRow; agentName: string }) {
  if (row.kind === 'status')
    return (
      <li className="flex flex-wrap items-center gap-2 py-2">
        <Badge variant={row.level === 'action' ? 'destructive' : 'outline'}>
          {row.level === 'action' ? copy.needs.statusAction : copy.needs.statusWatch}
        </Badge>
        <span>{row.reason}</span>
        <span className="text-xs text-muted-foreground">{copy.needs.ratedOn}</span>
      </li>
    )
  if (row.kind === 'budget')
    return (
      <li className="flex flex-wrap items-center gap-2 py-2">
        <Badge variant={row.level === 'action' ? 'destructive' : 'outline'}>
          {copy.needs.budgetBadge}
        </Badge>
        <span>{row.sentence}</span>
        {/* The Budget section on this page (B5). */}
        <a href="#hub-budget" className={LINK}>
          {copy.needs.reviewBudget}
        </a>
      </li>
    )
  if (row.kind === 'requests')
    return (
      <li className="flex flex-wrap items-center gap-2 py-2">
        <span>{copy.needs.requests(row.count)}</span>
        {row.firstChat ? (
          <Link
            to="/chat/$sessionId"
            params={{ sessionId: row.firstChat }}
            hash={row.firstRequest ? `request-${row.firstRequest}` : undefined}
            className={LINK}
          >
            {copy.needs.answer}
          </Link>
        ) : null}
      </li>
    )
  return (
    <li className="flex flex-wrap items-center gap-2 py-2">
      <span>{copy.needs.sessions(row.failed, row.checked)}</span>
      {row.ambiguous ? (
        <span className="text-xs text-muted-foreground">{copy.needs.shared}</span>
      ) : null}
      <Link
        to="/sessions"
        search={{ agent: agentName, status: 'failed', preset: '7d' }}
        className={LINK}
      >
        {copy.needs.openFailed}
      </Link>
    </li>
  )
}
