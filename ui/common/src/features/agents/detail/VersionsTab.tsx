/**
 * Versions (plan §7.3): newest first, active marked. Roll back lives in each row's menu and
 * only where the server allows it (`can_rollback && !is_active`), for managers. Progress is a
 * watch in the query cache, so it survives tab switches and shows in the header status too.
 */
import { MoreHorizontal } from 'lucide-react'
import { useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { PageLoader } from '@/components/shared/page-loader'
import { ErrorState } from '@/features/observability/StateCard'
import { useRollback, useVersions, useWatch } from '../api'
import { CliCommand, LearnMore, Section } from '../components/bits'
import { relTime } from '../format'
import { RollbackDialog } from '../components/dialogs'
import { copy } from '../copy'
import type { AgentView } from '../normalize'
import type { AgentVersion } from '../types'

export function VersionsTab({ agent }: { agent: AgentView }) {
  const versions = useVersions(agent.id, true)
  const rollback = useRollback(agent.id)
  const w = useWatch(agent.id)
  const [target, setTarget] = useState<AgentVersion | null>(null)
  const rb = w.watch?.kind === 'rollback' ? w.watch : null

  const confirm = (reason: string) => {
    if (!target) return
    rollback.mutate(
      { target_version: target.version, reason: reason || undefined },
      {
        onSuccess: (res) => {
          setTarget(null)
          w.begin('rollback', res?.build_id)
        },
      },
    )
  }

  return (
    <Section title={copy.versions} action={<LearnMore href="versions" />}>
      {rb ? (
        <div
          role="status"
          className="space-y-2 rounded-md border border-info/40 bg-info/5 p-3 text-sm"
        >
          {!rb.outcome ? (
            <p>{copy.rollBackQueued(rb.buildId ?? '—')}</p>
          ) : rb.outcome === 'timeout' ? (
            <>
              <p>{copy.stillRollingBack(rb.buildId ?? '—')}</p>
              <CliCommand command={`nasiko logs ${agent.id}`} note={false} />
              <Button size="sm" variant="outline" onClick={() => w.begin('rollback', rb.buildId)}>
                {copy.keepWatching}
              </Button>
            </>
          ) : rb.outcome === 'crashed' ? (
            <p className="text-warning">{copy.crashedAgain}</p>
          ) : (
            <p>{copy.rolledBack}</p>
          )}
        </div>
      ) : null}
      {versions.isPending ? (
        <PageLoader label={copy.loadingVersions} inline className="min-h-64" />
      ) : versions.isError ? (
        <ErrorState error={versions.error} onRetry={() => void versions.refetch()} />
      ) : versions.data.length === 0 ? (
        <p className="text-sm text-muted-foreground">{copy.noVersions}</p>
      ) : (
        <ul className="divide-y divide-border text-sm">
          {versions.data.map((v) => {
            const canRoll = agent.canManage && v.can_rollback && !v.is_active
            return (
              <li key={v.id} className="flex flex-wrap items-start justify-between gap-2 py-2.5">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="font-mono">{v.version}</span>
                    {v.is_active ? (
                      <Badge variant="outline" className="border-success/40 text-success">
                        {copy.active}
                      </Badge>
                    ) : null}
                    <span className="text-xs text-muted-foreground">{relTime(v.created_at)}</span>
                  </div>
                  {v.changelog ? <p className="text-muted-foreground">{v.changelog}</p> : null}
                </div>
                {canRoll ? (
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="size-9 p-0"
                        aria-label={`Actions for version ${v.version}`}
                      >
                        <MoreHorizontal className="size-4" aria-hidden />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem
                        disabled={w.watching}
                        onSelect={() => {
                          rollback.reset()
                          setTarget(v)
                        }}
                      >
                        {copy.rollBackTo}
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                ) : null}
              </li>
            )
          })}
        </ul>
      )}
      <RollbackDialog
        open={!!target}
        onOpenChange={(o) => {
          if (!o) setTarget(null)
        }}
        version={target?.version ?? ''}
        pending={rollback.isPending}
        error={rollback.isError ? rollback.error : undefined}
        onConfirm={confirm}
      />
    </Section>
  )
}
