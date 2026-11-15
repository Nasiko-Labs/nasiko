/**
 * An agent's Builds tab (plans/feat-deploy.md §6; design review 1): that agent's last 20 builds
 * (`GET /api/builds/agent/{id}`), each opening the same Build page. Rows follow the Builds list (two lines on phones).
 */
import { Link } from '@tanstack/react-router'
import { FileText, Hammer, RotateCw } from 'lucide-react'
import { Fragment, useState } from 'react'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { PanelError } from '@nasiko/ui/composites/panel'
import { EmptyState, StateCard } from '@nasiko/ui/composites/state-card'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { Spinner } from '@nasiko/ui/primitives/spinner'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { relTime } from '@nasiko/features/agents/format'
import { NO_BUILD_LOGS, useAgentBuilds, useBuildLogs, useFailureDetails } from './api'
import { copy } from './copy'
import { failureReason } from './errors'
import { buildSource, fmtElapsed, isActive } from './steps'
import { BuildBadge } from './components/BuildBadge'
import { BuildLogView } from './components/BuildLogView'

export function AgentBuildsTab({ agentId, agentName }: { agentId: string; agentName?: string }) {
  const q = useAgentBuilds(agentId)
  const rows = q.data?.available ? q.data.value : []
  const reasons = useFailureDetails(rows.filter((b) => b.status === 'failed').map((b) => b.id))
  // One build's logs open at a time, fetched on demand (steps.ts `isActive` gates the toggle to
  // finished builds — a running build has no final log yet).
  const [openLogs, setOpenLogs] = useState<string | null>(null)
  const now = q.dataUpdatedAt
  if (q.isPending) return <PageLoader label={copy.builds.loading} inline className="min-h-64" />
  if (q.isError)
    return (
      <PanelError error={q.error} onRetry={() => void q.refetch()} what={copy.agentBuilds.what} />
    )
  if (q.data && !q.data.available) return <StateCard title={copy.builds.noRights} />
  const deploy = (
    <Link
      to="/deploy"
      search={agentName ? { name: agentName, newVersion: true } : {}}
      {...asButton({ variant: 'outline', size: 'sm', className: 'pointer-coarse:min-h-11' })}
    >
      {copy.agentBuilds.deploy}
    </Link>
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
            <TableHead className="sr-only">{copy.build.logsHeading}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((b) => {
            const reason = b.status === 'failed' ? reasons.get(b.id) : undefined
            const end = isActive(b.status) ? now : Date.parse(b.updated_at)
            const open = openLogs === b.id
            return (
              <Fragment key={b.id}>
                <TableRow
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
                  <TableCell className="text-right max-md:col-span-2 max-md:p-0">
                    {isActive(b.status) ? null : (
                      <Button
                        size="xs"
                        variant="ghost"
                        onClick={() => setOpenLogs(open ? null : b.id)}
                        aria-expanded={open}
                        className="text-muted-foreground"
                      >
                        <FileText aria-hidden /> {open ? copy.build.hideLogs : copy.build.viewLogs}
                      </Button>
                    )}
                  </TableCell>
                </TableRow>
                {open ? (
                  <TableRow className="hover:bg-transparent max-md:block">
                    <TableCell colSpan={6} className="max-md:block">
                      <BuildLogs buildId={b.id} failed={b.status === 'failed'} />
                    </TableCell>
                  </TableRow>
                ) : null}
              </Fragment>
            )
          })}
        </TableBody>
      </Table>
    </div>
  )
}

/**
 * One build's captured output, fetched on demand from `GET /api/builds/{id}/logs` when its row is
 * expanded. The server records build output for every source build (import/registry/upload), so this
 * works for agents installed from the CLI, the registry and the Marketplace alike; a build with none
 * recorded says so rather than showing an error.
 */
function BuildLogs({ buildId, failed }: { buildId: string; failed: boolean }) {
  const logs = useBuildLogs(buildId, true)
  return (
    <div className="text-xs" aria-label={copy.build.logsHeading}>
      {logs.isPending ? (
        <p className="flex items-center gap-1 text-muted-foreground">
          <Spinner aria-hidden /> {copy.build.logsLoading}
        </p>
      ) : logs.isError ? (
        <p className="flex flex-wrap items-center gap-2 text-destructive">
          {copy.build.logsError}
          <Button size="xs" variant="outline" onClick={() => void logs.refetch()}>
            <RotateCw aria-hidden /> {copy.build.logsRetry}
          </Button>
        </p>
      ) : logs.data === NO_BUILD_LOGS ? (
        <p className="text-muted-foreground">{copy.build.logsEmpty}</p>
      ) : (
        <BuildLogView logs={logs.data} highlightErrors={failed} />
      )}
    </div>
  )
}
