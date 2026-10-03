/**
 * An agent's Builds tab (plans/feat-deploy.md §6; design review 1): that agent's last 20 builds
 * (`GET /api/builds/agent/{id}`), each opening the same Build page. Rows follow the Builds list (two lines on phones).
 */
import { Link } from '@tanstack/react-router'
import { Hammer } from 'lucide-react'
import { PageLoader } from '@/components/shared/page-loader'
import { PanelError } from '@/components/shared/panel'
import { EmptyState, StateCard } from '@/components/shared/state-card'
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
import { useAgentBuilds, useFailureDetails } from './api'
import { copy } from './copy'
import { failureReason } from './errors'
import { buildSource, fmtElapsed, isActive } from './steps'
import { BuildBadge } from './components/BuildBadge'

export function AgentBuildsTab({ agentId, agentName }: { agentId: string; agentName?: string }) {
  const q = useAgentBuilds(agentId)
  const rows = q.data?.available ? q.data.value : []
  const reasons = useFailureDetails(rows.filter((b) => b.status === 'failed').map((b) => b.id))
  const now = q.dataUpdatedAt
  if (q.isPending) return <PageLoader label={copy.builds.loading} inline className="min-h-64" />
  if (q.isError)
    return (
      <PanelError error={q.error} onRetry={() => void q.refetch()} what={copy.agentBuilds.what} />
    )
  if (q.data && !q.data.available) return <StateCard title={copy.builds.noRights} />
  const deploy = (
    <Button asChild size="sm" variant="outline" className="pointer-coarse:min-h-11">
      <Link to="/deploy" search={{ name: agentName }}>
        {copy.agentBuilds.deploy}
      </Link>
    </Button>
  )
  if (!rows.length)
    return (
      <EmptyState icon={Hammer} title={copy.agentBuilds.empty} action={deploy}>
        {copy.agentBuilds.emptyHint}
      </EmptyState>
    )
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-xs text-muted-foreground">{copy.agentBuilds.last20}</p>
        {deploy}
      </div>
      <Table className="text-sm" aria-label={copy.agentBuilds.label}>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead className="max-md:sr-only">{copy.build.version}</TableHead>
            <TableHead className="max-md:sr-only">{copy.builds.colStatus}</TableHead>
            <TableHead className="max-md:sr-only">{copy.builds.colSource}</TableHead>
            <TableHead className="max-md:sr-only">{copy.builds.colStarted}</TableHead>
            <TableHead className="text-right max-md:sr-only">{copy.builds.colDuration}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((b) => {
            const reason = b.status === 'failed' ? reasons.get(b.id) : undefined
            const end = isActive(b.status) ? now : Date.parse(b.updated_at)
            return (
              <TableRow
                key={b.id}
                data-testid="agent-build-row"
                data-status={b.status}
                className="max-md:grid max-md:grid-cols-[1fr_auto] max-md:gap-x-2 max-md:py-2"
              >
                <TableCell className="min-w-0 max-md:p-0">
                  <Link
                    to="/builds/$buildId"
                    params={{ buildId: b.id }}
                    className="font-mono font-medium underline-offset-4 hover:underline pointer-coarse:inline-flex pointer-coarse:min-h-11 pointer-coarse:items-center"
                  >
                    {b.version_tag}
                  </Link>
                  {b.status === 'failed' ? (
                    <p className="truncate text-xs text-muted-foreground">
                      {reason ? failureReason(reason) : copy.status.failed}
                    </p>
                  ) : null}
                </TableCell>
                <TableCell className="max-md:p-0 max-md:text-right">
                  <BuildBadge badge={b.status} />
                </TableCell>
                <TableCell className="text-muted-foreground max-md:col-span-2 max-md:p-0 max-md:text-xs">
                  {buildSource(b)}
                </TableCell>
                <TableCell className="text-muted-foreground tabular-nums max-md:hidden">
                  {relTime(b.created_at, Math.max(now, Date.parse(b.created_at)))}
                </TableCell>
                <TableCell className="text-right text-muted-foreground tabular-nums max-md:hidden">
                  {fmtElapsed(end - Date.parse(b.created_at))}
                </TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    </div>
  )
}
