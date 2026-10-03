/** One run of the workflow (plans/feat-workflows.md §6), live while it moves (a paused run included). */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronLeft, RotateCw } from 'lucide-react'
import { PageLoader } from '@/components/shared/page-loader'
import { EmptyState, StateCard } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import { relTime } from '@/features/agents/format'
import { Markdown } from '@/features/chat/components/Markdown'
import { ApiError } from '@/lib/api/client'
import { fmtDuration, fmtTokens } from '@/lib/format'
import { executionQuery } from '../api'
import { copy, reason } from '../copy'
import { execStatus, showsRunError } from '../logic'
import type { Workflow } from '../types'
import { Chip, ToneBadge } from './bits'
import { RunSteps } from './RunSteps'

export function RunView({ wf, exec, onBack }: { wf: Workflow; exec: string; onBack: () => void }) {
  const queryClient = useQueryClient()
  const run = useQuery(executionQuery(exec))
  const e = run.data
  // A deep link can pair a run with another workflow: then its titles are not ours to give.
  const ours = !e?.maf_id || e.maf_id === wf.id
  const labels = ours
    ? Object.fromEntries((wf.maf_json?.steps ?? []).map((s) => [s.step_id, s.task_description]))
    : undefined
  const title = ours ? wf.name || copy.execution : copy.execution
  const steps = e?.step_results ?? []

  let body
  if (run.isError && !e)
    body =
      run.error instanceof ApiError && (run.error.status === 404 || run.error.isForbidden) ? (
        <EmptyState title={copy.runNotFound}>{copy.runNotFoundText}</EmptyState>
      ) : (
        <StateCard
          tone="error"
          title={copy.loadRunFailed}
          fix={reason(run.error)}
          action={
            <Button size="sm" variant="outline" onClick={() => void run.refetch()}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
      )
  else if (!e) body = <PageLoader label={copy.loadingRun} inline className="min-h-64" />
  else {
    const status = execStatus(e.status)
    const chips = [
      steps.length ? copy.steps(steps.length) : '',
      e.started_at ? copy.started(relTime(e.started_at)) : '',
      e.duration_ms != null ? fmtDuration(e.duration_ms) : '',
      e.tokens_used ? copy.tokens(fmtTokens(e.tokens_used)) : '',
      (e.attempt_count ?? 0) > 1 ? copy.attempt(e.attempt_count ?? 0, e.max_attempts ?? 0) : '',
    ].filter(Boolean)
    body = (
      <>
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="text-base font-medium">{copy.executionN(e.execution_number)}</h2>
            <ToneBadge tone={status.tone}>{status.label}</ToneBadge>
          </div>
          <ul aria-label={copy.runDetails} className="flex flex-wrap gap-1.5">
            {chips.map((t) => (
              <li key={t}>
                <Chip>{t}</Chip>
              </li>
            ))}
          </ul>
        </div>
        <RunSteps
          steps={steps}
          labels={labels}
          totalTokens={e.tokens_used ?? 0}
          hitl={e.hitl}
          onHitlChange={() =>
            void queryClient.invalidateQueries({ queryKey: executionQuery(exec).queryKey })
          }
        />
        {e.output ? (
          <section aria-labelledby="run-output" className="flex flex-col gap-2">
            <h2 id="run-output" className="text-base font-medium">
              {copy.output}
            </h2>
            <Markdown text={e.output} className="rounded-xl border p-4 text-sm break-words" />
          </section>
        ) : null}
        {showsRunError(e) ? (
          <p className="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm break-words whitespace-pre-wrap text-destructive">
            {e.error}
          </p>
        ) : null}
      </>
    )
  }

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 [&>*]:min-w-0">
      <title>{`${title} · OpenRuntime`}</title>
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="icon-sm" aria-label={copy.backToWorkflow} onClick={onBack}>
          <ChevronLeft aria-hidden />
        </Button>
        <h1 tabIndex={-1} className="min-w-0 truncate text-xl font-semibold outline-none">
          {title}
        </h1>
      </div>
      {body}
    </div>
  )
}
