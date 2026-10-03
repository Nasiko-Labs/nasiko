/**
 * A new workflow (plans/feat-workflows.md §3): a name, a description the planner can draft steps from, and the steps.
 * Deploy makes it live; "Save as draft and test" keeps it for the workflow page's Save & run. Leaving with unsaved
 * changes asks first. On `main` (no drafts, W-1) the draft actions are hidden.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useBlocker, useNavigate } from '@tanstack/react-router'
import { ChevronLeft, Loader2, TriangleAlert } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { PageHeader } from '@/components/shared/page-header'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useAgentsDirectory } from '@/features/agents/api'
import { ApiError } from '@/lib/api/client'
import {
  createWorkflow,
  generateWorkflow,
  promoteWorkflow,
  runWorkflow,
  saveDraft,
  updateWorkflow,
  workflowKeys,
  workflowListQuery,
} from './api'
import { copy, reason } from './copy'
import {
  agentOptions,
  blankStep,
  derivedName,
  fromPlan,
  isDraftsAbsent,
  remapStepError,
  snapshot,
  toPayload,
  type EditorStep,
} from './logic'
import type { Workflow } from './types'
import { PlanComposer } from './components/PlanComposer'
import { StepEditor } from './components/StepEditor'

const EMPTY = snapshot('', '', [blankStep()])

export function NewWorkflowPage() {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const agents = useAgentsDirectory()
  // The drafts list doubles as the probe: the Drafts page reads the same key.
  const drafts = useQuery(workflowListQuery({ mode: 'drafts', sort: 'all' }))
  const canDraft = !(drafts.isError && isDraftsAbsent(drafts.error))
  const [name, setName] = useState('')
  const [desc, setDesc] = useState('')
  const [steps, setSteps] = useState<EditorStep[]>(() => [blankStep()])
  const [clean, setClean] = useState(EMPTY)
  const [draftId, setDraftId] = useState<string | null>(null)
  const [generatedFrom, setGeneratedFrom] = useState<string | null>(null)
  const [planned, setPlanned] = useState(EMPTY)
  const [deployed, setDeployed] = useState<Workflow | null>(null)

  const n = name.trim()
  const d = desc.trim()
  const now = snapshot(name, desc, steps)
  const dirty = !deployed && now !== clean
  const { steps: payload, index } = toPayload(steps)

  const gen = useMutation({ mutationFn: generateWorkflow })
  const generate = (description: string) =>
    gen.mutate(description, {
      onSuccess: (plan) => {
        if (!n && plan.name) setName(plan.name)
        const next = fromPlan(plan)
        setSteps(next)
        setPlanned(snapshot('', '', next))
        setGeneratedFrom(description)
      },
    })

  const failed = (err: unknown) => toast.error(copy.saveFailed(remapStepError(reason(err), index)))
  const lists = () => void queryClient.invalidateQueries({ queryKey: workflowKeys.lists })

  /** The draft row, then its name and steps; a draft gone under us (404) is re-created once. */
  const writeDraft = async () => {
    const instruction = d || n
    const attempt = async (id: string | null) => {
      if (!id || !payload.length) {
        id = (await saveDraft({ instruction, ...(id ? { draft_id: id } : {}) })).id
        setDraftId(id)
        if (!payload.length) {
          // Keep the typed name (the server named the draft after the instruction).
          if (n !== derivedName(instruction)) await updateWorkflow(id, { name: n })
          return id
        }
      }
      // The description only if one was typed (legacy stored the name as it).
      await updateWorkflow(id, { name: n, description: d || null, steps: payload })
      return id
    }
    try {
      return await attempt(draftId)
    } catch (err) {
      if (!(err instanceof ApiError && err.status === 404 && draftId)) throw err
      setDraftId(null)
      return attempt(null)
    }
  }
  const draft = useMutation({
    mutationFn: writeDraft,
    onSuccess: () => {
      setClean(now)
      lists()
    },
    onError: failed,
  })
  const deploy = useMutation({
    // With a draft already saved, that row is the workflow: never a second one.
    mutationFn: async () => {
      if (!draftId) return createWorkflow({ name: n, description: d || undefined, steps: payload })
      await updateWorkflow(draftId, { name: n, description: d || null, steps: payload })
      return promoteWorkflow(draftId)
    },
    onSuccess: (wf) => {
      setDeployed(wf)
      setClean(now)
      lists()
    },
    onError: failed,
  })
  const run = useMutation({
    mutationFn: (id: string) => runWorkflow(id),
    // The Runs tab with the run open, as Run does (`run.ts`).
    onSuccess: (r) =>
      void navigate({
        to: '/workflows/runs',
        search: { run: r.execution_id },
        ignoreBlocker: true,
      }),
    onError: (err, id) => {
      void navigate({
        to: '/workflows/$workflowId',
        params: { workflowId: id },
        search: {},
        ignoreBlocker: true,
      })
      toast.error(copy.runDidntStart(reason(err)))
    },
  })

  const blocker = useBlocker({
    // A 401 sends the page to /login: an expired session can't save anyway (as LeaveGuard).
    shouldBlockFn: ({ next }) => dirty && next.pathname !== '/login',
    enableBeforeUnload: () => dirty,
    withResolver: true,
  })

  const busy = gen.isPending || draft.isPending || deploy.isPending
  const named = !!n && !gen.isPending
  const locked = !!deployed

  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-6 [&>*]:min-w-0">
      <div className="flex items-start gap-2">
        <Button asChild variant="ghost" size="icon-sm" className="mt-0.5">
          <Link to="/workflows" aria-label={copy.back}>
            <ChevronLeft aria-hidden />
          </Link>
        </Button>
        <PageHeader title={copy.newTitle} description={copy.newText} />
      </div>
      <div className="flex flex-col gap-2">
        <Label htmlFor="wf-name">
          {n ? copy.nameLabel : copy.nameRequired}
          {n ? null : (
            <span aria-hidden className="text-destructive">
              *
            </span>
          )}
        </Label>
        <Input
          id="wf-name"
          placeholder={copy.namePlaceholder}
          aria-required
          value={name}
          disabled={locked}
          onChange={(e) => setName(e.target.value)}
        />
      </div>
      <PlanComposer
        value={desc}
        onChange={setDesc}
        onGenerate={generate}
        generatedFrom={generatedFrom}
        confirmReplace={
          steps.some((s) => s.taskDescription.trim()) && snapshot('', '', steps) !== planned
        }
        busy={gen.isPending}
        error={gen.error}
        placeholder={copy.descPlaceholderNew}
        disabled={locked}
      />
      {gen.isPending ? null : (
        <StepEditor steps={steps} onChange={setSteps} agents={agentOptions(agents.data ?? [])} />
      )}
      <div className="flex flex-wrap justify-end gap-2">
        {canDraft ? (
          <Button
            variant="outline"
            disabled={!named || locked || busy}
            onClick={() =>
              draft.mutate(undefined, {
                onSuccess: (id) =>
                  void navigate({
                    to: '/workflows/$workflowId',
                    params: { workflowId: id },
                    search: {},
                    ignoreBlocker: true,
                  }),
              })
            }
          >
            {draft.isPending ? <Loader2 className="animate-spin" aria-hidden /> : null}
            {copy.saveDraft}
          </Button>
        ) : null}
        <Button
          disabled={!named || !payload.length || locked || busy}
          onClick={() => deploy.mutate()}
        >
          {deploy.isPending ? <Loader2 className="animate-spin" aria-hidden /> : null}
          {copy.deploy}
        </Button>
      </div>

      <AlertDialog
        open={blocker.status === 'blocked'}
        onOpenChange={(o) => !o && blocker.status === 'blocked' && blocker.reset()}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle className="flex items-center gap-2">
              <TriangleAlert aria-hidden className="size-5 text-warning" />
              {copy.leaveTitle}
            </AlertDialogTitle>
            <AlertDialogDescription>{copy.leaveText}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{copy.keepEditing}</AlertDialogCancel>
            <Button
              variant="outline"
              onClick={() => blocker.status === 'blocked' && blocker.proceed()}
            >
              {copy.discard}
            </Button>
            {canDraft ? (
              <Button
                disabled={!named || draft.isPending}
                onClick={() =>
                  draft.mutate(undefined, {
                    onSuccess: () => blocker.status === 'blocked' && blocker.proceed(),
                  })
                }
              >
                {draft.isPending ? <Loader2 className="animate-spin" aria-hidden /> : null}
                {copy.saveDraftShort}
              </Button>
            ) : null}
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* Not dismissible: a second Deploy would be a second live workflow. */}
      <AlertDialog open={!!deployed}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{copy.deployedTitle}</AlertDialogTitle>
            <AlertDialogDescription>{copy.deployedText}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <Button
              variant="outline"
              onClick={() => void navigate({ to: '/workflows', ignoreBlocker: true })}
            >
              {copy.findInLibrary}
            </Button>
            <Button disabled={run.isPending} onClick={() => deployed && run.mutate(deployed.id)}>
              {run.isPending ? <Loader2 className="animate-spin" aria-hidden /> : null}
              {copy.runWorkflow}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
