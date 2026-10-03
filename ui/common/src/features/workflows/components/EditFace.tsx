/**
 * The editor face (plans/feat-workflows.md §5): always for a draft (Save & run, Deploy), behind Edit for a live
 * workflow (Cancel, Save & run: saving edits the live row). The run endpoint runs the stored row, so running the
 * edits means saving them first.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Loader2, Play } from 'lucide-react'
import { useRef, useState, type ReactNode } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useAgentsDirectory } from '@/features/agents/api'
import {
  generateWorkflow,
  promoteWorkflow,
  runWorkflow,
  updateWorkflow,
  workflowKeys,
} from '../api'
import { copy, reason } from '../copy'
import {
  agentOptions,
  descriptionOf,
  fromMaf,
  fromPlan,
  isDeployed,
  remapStepError,
  snapshot,
  toPayload,
  type EditorStep,
} from '../logic'
import type { Workflow } from '../types'
import { PlanComposer } from './PlanComposer'
import { StepEditor } from './StepEditor'

export function EditFace({
  wf,
  onCancel,
  onRun,
  children,
}: {
  wf: Workflow
  onCancel: () => void
  onRun: (execId: string) => void
  children?: ReactNode
}) {
  const queryClient = useQueryClient()
  const agents = useAgentsDirectory()
  const live = isDeployed(wf)
  const initialDesc = descriptionOf(wf)
  const [name, setName] = useState(wf.name)
  const [desc, setDesc] = useState(initialDesc)
  const [steps, setSteps] = useState<EditorStep[]>(() => fromMaf(wf.maf_json?.steps))
  const [generatedFrom, setGeneratedFrom] = useState<string | null>(() =>
    wf.maf_json?.steps?.length ? initialDesc.trim() : null,
  )
  const [planned, setPlanned] = useState(() => snapshot('', '', steps))
  const failedAt = useRef<'Save' | 'Run' | 'Deploy'>('Save')

  const gen = useMutation({ mutationFn: generateWorkflow })
  const generate = (description: string) =>
    gen.mutate(description, {
      onSuccess: (plan) => {
        const next = fromPlan(plan)
        setSteps(next)
        setPlanned(snapshot('', '', next))
        setGeneratedFrom(description)
      },
    })

  const { steps: payload, index } = toPayload(steps)
  const key = workflowKeys.detail(wf.id)
  const act = useMutation({
    mutationFn: async (then: 'run' | 'deploy') => {
      failedAt.current = 'Save'
      const saved = await updateWorkflow(wf.id, {
        // An empty name would be stored as one; an emptied description is cleared.
        name: name.trim() || undefined,
        description: desc.trim() || null,
        steps: payload,
      })
      failedAt.current = then === 'run' ? 'Run' : 'Deploy'
      if (then === 'run') {
        const r = await runWorkflow(wf.id)
        queryClient.setQueryData(key, saved)
        return r.execution_id
      }
      queryClient.setQueryData(key, await promoteWorkflow(wf.id))
      return null
    },
    onSuccess: (execId) => {
      void queryClient.invalidateQueries({ queryKey: workflowKeys.lists })
      if (execId) onRun(execId)
      else toast.success(copy.deployedToast)
    },
    onError: (err) => {
      // A save that landed before the run or promote failed is still the row now.
      void queryClient.invalidateQueries({ queryKey: key })
      toast.error(copy.stepFailed(failedAt.current, remapStepError(reason(err), index)))
    },
  })
  const save = (then: 'run' | 'deploy') => {
    if (!payload.length) return void toast.error(copy.needsStep)
    act.mutate(then)
  }
  const busy = act.isPending || gen.isPending
  const spin = (then: 'run' | 'deploy') =>
    act.isPending && act.variables === then ? (
      <Loader2 className="animate-spin" aria-hidden />
    ) : null

  return (
    <>
      <h1 tabIndex={-1} className="sr-only">
        {wf.name}
      </h1>
      <div className="flex flex-col gap-2">
        <Label htmlFor="wf-name">{copy.nameLabel}</Label>
        <Input id="wf-name" value={name} onChange={(e) => setName(e.target.value)} />
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
        placeholder={copy.descPlaceholderEdit}
      />
      {gen.isPending ? null : (
        <StepEditor steps={steps} onChange={setSteps} agents={agentOptions(agents.data ?? [])} />
      )}
      {children}
      <div className="flex flex-wrap justify-end gap-2">
        {live ? (
          <>
            <Button variant="ghost" disabled={busy} onClick={onCancel}>
              {copy.cancel}
            </Button>
            <Button disabled={busy} onClick={() => save('run')}>
              {spin('run') ?? <Play aria-hidden />} {copy.saveAndRun}
            </Button>
          </>
        ) : (
          <>
            <Button variant="outline" disabled={busy} onClick={() => save('run')}>
              {spin('run') ?? <Play aria-hidden />} {copy.saveAndRun}
            </Button>
            <Button disabled={busy} onClick={() => save('deploy')}>
              {spin('deploy')} {copy.deploy}
            </Button>
          </>
        )}
      </div>
    </>
  )
}
