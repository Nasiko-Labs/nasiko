/**
 * One workflow (plans/feat-workflows.md §5): its review (read-only when live, the editor while a draft) or, with
 * `?run=`, one of its runs. Opening a run pushes history; the run's Back pops what this page pushed and replaces
 * otherwise (a deep link, the runs page). A 404 or 403 is one dead end (the server never says which to a stranger).
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useRouter } from '@tanstack/react-router'
import { Pencil, Play, RotateCw, Workflow as WorkflowIcon } from 'lucide-react'
import { useState } from 'react'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { PrevNext } from '@nasiko/ui/composites/prev-next'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { EmptyState, StateCard } from '@nasiko/ui/composites/state-card'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { isUuid } from '@nasiko/features/agents/normalize'
import { ApiError } from '@nasiko/data-access/client'
import { useSiblings } from '@nasiko/app-core/siblings'
import { dropFromLists, workflowQuery } from './api'
import { copy, reason } from './copy'
import { descriptionOf, isDeployed } from './logic'
import { useStartRun } from './run'
import type { Workflow } from './types'
import { DeleteWorkflowDialog } from './components/DeleteWorkflowDialog'
import { EditFace } from './components/EditFace'
import { RunView } from './components/RunView'

const COLUMN = 'mx-auto flex w-full max-w-3xl flex-col gap-6 [&>*]:min-w-0'

export function WorkflowPage({ id, run }: { id: string; run?: string }) {
  // The server's path parameter is a UUID (anything else is a plain-text 400).
  if (!isUuid(id)) return <DeadEnd title={copy.notFound} text={copy.notFoundText} />
  return <Detail id={id.toLowerCase()} run={run} />
}

/** `id`: a workflow stepped to from a list keeps its arrows when it fails to load, so the user can step on. */
function DeadEnd({ id, title, text }: { id?: string; title: string; text: string }) {
  return (
    <div className={COLUMN}>
      {id ? <TopBar stepper={<WorkflowSiblings id={id} />} /> : null}
      <EmptyState
        icon={WorkflowIcon}
        title={<h1>{title}</h1>}
        action={
          <Link to="/workflows" {...asButton({ variant: 'outline' })}>
            {copy.backToWorkflows}
          </Link>
        }
      >
        {text}
      </EmptyState>
    </div>
  )
}

function Detail({ id, run }: { id: string; run?: string }) {
  const wf = useQuery(workflowQuery(id))
  if (wf.data) return <Loaded key={wf.data.id} wf={wf.data} run={run} />
  if (wf.isError)
    return wf.error instanceof ApiError && (wf.error.status === 404 || wf.error.isForbidden) ? (
      <DeadEnd id={id} title={copy.notFound} text={copy.notFoundText} />
    ) : (
      <div className={COLUMN}>
        <h1 className="sr-only">{copy.loadOneFailed}</h1>
        <TopBar stepper={<WorkflowSiblings id={id} />} />
        <StateCard
          tone="error"
          title={copy.loadOneFailed}
          fix={reason(wf.error)}
          action={
            <Button size="sm" variant="outline" onClick={() => void wf.refetch()}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
      </div>
    )
  return <PageLoader label={copy.loadingWorkflow} />
}

function Loaded({ wf, run }: { wf: Workflow; run?: string }) {
  const navigate = useNavigate()
  const router = useRouter()
  const [editing, setEditing] = useState(false)
  const live = isDeployed(wf)
  // Save & run lands on the Runs tab with the run open, as Run does (`run.ts`). The edits are saved by then, but the
  // editor's leave guard may not have re-rendered yet.
  const openRun = (execId: string) =>
    void navigate({ to: '/workflows/runs', search: { run: execId }, ignoreBlocker: true })
  if (run)
    return (
      <RunView
        wf={wf}
        exec={run}
        onBack={() =>
          router.state.location.state.workflowRunPushed
            ? router.history.back()
            : void navigate({
                to: '/workflows/$workflowId',
                params: { workflowId: wf.id },
                search: {},
                // Keeps the list this workflow was opened from (lib/siblings.ts).
                state: (s) => ({ siblings: s.siblings }),
                replace: true,
              })
        }
      />
    )
  return (
    <div className={COLUMN}>
      <title>{`${wf.name} · Nasiko`}</title>
      <TopBar
        trail={
          live
            ? [wf.name]
            : [
                <Link key="drafts" to="/workflows/drafts">
                  {copy.crumbDrafts}
                </Link>,
                wf.name,
              ]
        }
        stepper={<WorkflowSiblings id={wf.id} />}
      />
      {live && !editing ? (
        <ReadOnlyFace wf={wf} onEdit={() => setEditing(true)} />
      ) : (
        <EditFace wf={wf} onCancel={() => setEditing(false)} onRun={openRun}>
          <OutputGuidelines wf={wf} />
        </EditFace>
      )}
      <DangerZone wf={wf} />
    </div>
  )
}

/** Previous/Next through the list this workflow was opened from (lib/siblings.ts); nothing on a deep link. */
function WorkflowSiblings({ id, className }: { id: string; className?: string }) {
  const navigate = useNavigate()
  const around = useSiblings(id)
  if (!around) return null
  // Replace: Back still returns to the list. A run (`?run=`) never carries over.
  const go = (workflowId: string | undefined) => {
    if (workflowId)
      void navigate({
        to: '/workflows/$workflowId',
        params: { workflowId },
        search: {},
        state: { siblings: around.siblings },
        replace: true,
      })
  }
  return (
    <PrevNext
      noun="workflow"
      index={around.index}
      count={around.count}
      onPrev={() => go(around.prev)}
      onNext={() => go(around.next)}
      className={className}
    />
  )
}

function OutputGuidelines({ wf }: { wf: Workflow }) {
  const text = wf.maf_json?.output_generation
  if (!text) return null
  return (
    <section aria-labelledby="output-guidelines" className="flex flex-col gap-2">
      <h2 id="output-guidelines" className="text-base font-medium">
        {copy.outputGuidelines}
      </h2>
      <p className="text-sm whitespace-pre-wrap text-muted-foreground">{text}</p>
    </section>
  )
}

/** A live workflow as it stands: Edit to change it, Run to run it. */
function ReadOnlyFace({ wf, onEdit }: { wf: Workflow; onEdit: () => void }) {
  const steps = wf.maf_json?.steps ?? []
  const startRun = useStartRun()
  return (
    <>
      <div className="flex flex-col gap-2">
        <h1 tabIndex={-1} className="text-xl font-semibold break-words outline-none">
          {wf.name}
        </h1>
        {descriptionOf(wf) ? (
          <p className="text-sm text-muted-foreground">{descriptionOf(wf)}</p>
        ) : null}
      </div>
      <section data-tour="wf-steps" aria-labelledby="steps-title" className="flex flex-col gap-3">
        <h2 id="steps-title" className="text-base font-medium">
          {copy.stepsTitle}
        </h2>
        {steps.length ? (
          <ol className="flex flex-col gap-3">
            {steps.map((s, i) => (
              <li key={s.step_id} className="flex flex-col gap-1 rounded-xl border p-4">
                <h3 className="text-sm font-medium">{copy.step(i + 1)}</h3>
                <p className="text-sm break-words">{s.task_description}</p>
                <p className="text-xs text-muted-foreground">
                  {s.agent_name || copy.assignedWhenSaved}
                </p>
              </li>
            ))}
          </ol>
        ) : (
          <EmptyState title={copy.noSteps}>{copy.noStepsLive}</EmptyState>
        )}
      </section>
      <OutputGuidelines wf={wf} />
      <div data-tour="wf-run" className="flex flex-wrap justify-end gap-2">
        <Button variant="ghost" onClick={onEdit}>
          <Pencil aria-hidden /> {copy.edit}
        </Button>
        <Button disabled={!steps.length} onClick={() => startRun(wf.id)}>
          <Play aria-hidden /> {copy.run}
        </Button>
      </div>
    </>
  )
}

function DangerZone({ wf }: { wf: Workflow }) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [asking, setAsking] = useState<Workflow | null>(null)
  const live = isDeployed(wf)
  return (
    <section
      data-tour="wf-danger"
      aria-labelledby="danger-zone"
      className="flex flex-col gap-3 rounded-xl border border-destructive/30 p-4"
    >
      <h2 id="danger-zone" className="text-base font-medium">
        {copy.dangerZone}
      </h2>
      <p className="text-sm text-muted-foreground">
        {live ? copy.dangerLive : copy.dangerDraft} {copy.dangerAfter}
      </p>
      <div>
        <Button
          variant="outline"
          className="border-destructive/40 text-destructive hover:text-destructive"
          onClick={() => setAsking(wf)}
        >
          {copy.delete}
        </Button>
      </div>
      <DeleteWorkflowDialog
        workflow={asking}
        onClose={() => setAsking(null)}
        onDeleted={() => {
          dropFromLists(queryClient, wf.id)
          void navigate({ to: live ? '/workflows' : '/workflows/drafts' })
        }}
      />
    </section>
  )
}
