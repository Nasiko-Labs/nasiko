/**
 * Logs (plans/feat-mcp.md §5, uploaded builds, owner or superuser): the build's status and error, and the container's
 * last 500 lines. `data` is an array of lines (M-3). Both refresh every 5 s while the build runs.
 */
import { RotateCw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { PageLoader } from '@/components/shared/page-loader'
import { CopyButton } from '@/components/shared/copy-button'
import { Section } from '@/features/agents/components/bits'
import { useBuildLogs, useBuildStatus } from '../api'
import { copy, reason, STATUS_LABEL } from '../copy'
import { serverStatus } from '../logic'
import type { ConnectorDetail } from '../types'

export function LogsTab({ connector: c }: { connector: ConnectorDetail }) {
  const building = serverStatus(c) === 'building'
  const build = useBuildStatus(c.connector_id, true, building)
  const logs = useBuildLogs(c.connector_id, true, building)
  const text = (logs.data ?? []).join('\n')
  const b = build.data
  return (
    <Section
      title={copy.logsTitle}
      action={
        <div className="flex items-center gap-1">
          {text ? <CopyButton text={text} label={copy.copyLogs} /> : null}
          <Button
            size="sm"
            variant="outline"
            disabled={logs.isFetching}
            onClick={() => {
              void build.refetch()
              void logs.refetch()
            }}
          >
            <RotateCw className="size-3.5" aria-hidden /> {copy.refresh}
          </Button>
        </div>
      }
    >
      {b ? (
        <p className="flex flex-wrap gap-x-3 text-sm text-muted-foreground">
          <span>
            {copy.buildStatusLine(
              STATUS_LABEL[serverStatus({ ...c, build_status: b.build_status })],
            )}
          </span>
          {b.image_tag ? <span className="font-mono">{copy.imageTag(b.image_tag)}</span> : null}
        </p>
      ) : null}
      {b?.error_msg ? (
        <p role="alert" className="text-sm text-destructive">
          {copy.buildError(b.error_msg)}
        </p>
      ) : null}
      {logs.isPending ? (
        <PageLoader label={copy.loadingLogs} inline className="min-h-64" />
      ) : logs.isError ? (
        <p role="alert" className="text-sm text-destructive">
          {copy.logsFailed}: {reason(logs.error)}
        </p>
      ) : (
        <pre className="max-h-[32rem] overflow-auto rounded-md border border-border bg-muted p-3 font-mono text-xs whitespace-pre-wrap">
          {text || copy.noLogs}
        </pre>
      )}
    </Section>
  )
}
