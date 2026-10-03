/**
 * The steps of a workflow: text, agent, order. Reorder by dragging the grip (only the grip, so text stays selectable;
 * Motion `Reorder` instead of the React flow's dnd-kit, which the lab doesn't ship) or ArrowUp/ArrowDown on it; each
 * move is announced in the editor's own polite region. Controlled: every edit calls `onChange`.
 */
import { GripVertical, Plus, X } from 'lucide-react'
import { Reorder, useDragControls } from 'motion/react'
import { useEffect, useId, useRef, useState } from 'react'
import { EmptyState } from '@/components/shared/state-card'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'
import { copy } from '../copy'
import { blankStep, moveStep, type AgentOption, type EditorStep } from '../logic'

interface CardProps {
  step: EditorStep
  index: number
  count: number
  agents: AgentOption[]
  listId: string
  onChange: (patch: Partial<EditorStep>) => void
  onRemove: () => void
  onMove: (to: number) => void
  onInsert: () => void
  onDragEnd: () => void
}

function StepItem({
  step,
  index,
  count,
  agents,
  listId,
  onChange,
  onRemove,
  onMove,
  onInsert,
  onDragEnd,
}: CardProps) {
  const n = index + 1
  const movable = count > 1
  const controls = useDragControls()
  const [dragging, setDragging] = useState(false)
  return (
    <Reorder.Item
      value={step.uid}
      dragListener={false}
      dragControls={controls}
      onDragStart={() => setDragging(true)}
      onDragEnd={() => {
        setDragging(false)
        onDragEnd()
      }}
      className="relative flex flex-col"
    >
      <InsertPoint after={index} onInsert={onInsert} />
      <div
        role="group"
        aria-label={copy.step(n)}
        className={cn(
          'relative flex flex-col gap-3 rounded-xl border bg-card p-4',
          dragging && 'z-10 shadow-lg ring-2 ring-ring/40',
        )}
      >
        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="icon-sm"
            data-grip={step.uid}
            disabled={!movable}
            title={movable ? copy.dragHint : undefined}
            aria-label={movable ? copy.reorder(n) : copy.step(n)}
            onPointerDown={(e) => movable && controls.start(e)}
            onKeyDown={(e) => {
              if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return
              e.preventDefault()
              onMove(e.key === 'ArrowUp' ? index - 1 : index + 1)
            }}
            className="-ml-1 cursor-grab touch-none text-muted-foreground active:cursor-grabbing disabled:cursor-default"
          >
            <GripVertical aria-hidden />
          </Button>
          <span className="text-sm font-medium">{copy.step(n)}</span>
          <Button
            variant="ghost"
            size="icon-sm"
            className="ml-auto"
            title={copy.removeStep(n)}
            aria-label={copy.removeStep(n)}
            disabled={count < 2}
            onClick={onRemove}
          >
            <X aria-hidden />
          </Button>
        </div>
        <Textarea
          rows={2}
          data-text={step.uid}
          aria-label={copy.instructions(n)}
          placeholder={copy.instructionsPlaceholder}
          value={step.taskDescription}
          onChange={(e) => onChange({ taskDescription: e.target.value })}
        />
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            {copy.chooseAgent}
            {step.suggested && step.agentId ? (
              <Badge variant="secondary">{copy.suggested}</Badge>
            ) : null}
          </div>
          <Input
            list={listId}
            aria-label={copy.agentFor(n)}
            placeholder={copy.agentPlaceholder}
            value={step.agentName}
            onChange={(e) => {
              const typed = e.target.value
              // An exact name picks that agent; anything else is auto-assign.
              const hit = agents.find((a) => a.name === typed.trim())
              onChange({
                agentId: hit?.id ?? '',
                agentName: hit ? hit.name : typed,
                suggested: false,
              })
            }}
          />
        </div>
      </div>
    </Reorder.Item>
  )
}

function InsertPoint({ after, onInsert }: { after: number; onInsert: () => void }) {
  return (
    <Button
      variant="ghost"
      aria-label={after === 0 ? copy.insertFirst : copy.insertAfter(after)}
      onClick={onInsert}
      className="group relative h-4 w-full rounded-none p-0 hover:bg-transparent focus-visible:ring-0 focus-visible:ring-offset-0"
    >
      <span className="h-px w-full bg-transparent group-hover:bg-border group-focus-visible:bg-ring" />
      <span className="absolute grid size-5 place-content-center rounded-full border bg-background opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100">
        <Plus className="size-3" aria-hidden />
      </span>
    </Button>
  )
}

export function StepEditor({
  steps,
  onChange,
  agents,
}: {
  steps: EditorStep[]
  onChange: (steps: EditorStep[]) => void
  agents: AgentOption[]
}) {
  const listId = useId()
  const [said, setSaid] = useState('')
  const root = useRef<HTMLDivElement>(null)
  // What to focus once the next render lands: a moved grip, a new textarea.
  const focus = useRef<{ uid: string; on: 'grip' | 'text' } | null>(null)

  useEffect(() => {
    const f = focus.current
    if (!f) return
    focus.current = null
    root.current?.querySelector<HTMLElement>(`[data-${f.on}="${f.uid}"]`)?.focus()
  }, [steps])

  const announce = (uid: string, list: EditorStep[]) =>
    setSaid(copy.moved(list.findIndex((s) => s.uid === uid) + 1, list.length))
  const move = (from: number, to: number) => {
    const next = moveStep(steps, from, to)
    const moved = steps[from]
    if (next === steps || !moved) return
    focus.current = { uid: moved.uid, on: 'grip' }
    onChange(next)
    announce(moved.uid, next)
  }
  const insert = (at: number) => {
    const step = blankStep()
    focus.current = { uid: step.uid, on: 'text' }
    onChange([...steps.slice(0, at), step, ...steps.slice(at)])
  }
  // Motion reports the order as the drag passes each card; the move is announced once, where it lands.
  const reorder = (uids: string[]) =>
    onChange(uids.map((u) => steps.find((s) => s.uid === u)).filter((s) => !!s))

  return (
    <div ref={root} id="editor" className="flex flex-col">
      {steps.length === 0 ? (
        <EmptyState title={copy.noSteps}>{copy.noStepsText}</EmptyState>
      ) : (
        <Reorder.Group
          as="ol"
          axis="y"
          values={steps.map((s) => s.uid)}
          onReorder={reorder}
          className="flex flex-col"
        >
          {steps.map((step, i) => (
            <StepItem
              key={step.uid}
              step={step}
              index={i}
              count={steps.length}
              agents={agents}
              listId={listId}
              onChange={(patch) =>
                onChange(steps.map((s) => (s.uid === step.uid ? { ...s, ...patch } : s)))
              }
              onRemove={() => onChange(steps.filter((s) => s.uid !== step.uid))}
              onMove={(to) => move(i, to)}
              onInsert={() => insert(i)}
              onDragEnd={() => announce(step.uid, steps)}
            />
          ))}
        </Reorder.Group>
      )}
      <div className="mt-2">
        <Button
          variant="ghost"
          size="icon-sm"
          title={copy.addStep}
          aria-label={copy.addStep}
          onClick={() => insert(steps.length)}
        >
          <Plus aria-hidden />
        </Button>
      </div>
      <datalist id={listId}>
        {agents.map((a) => (
          <option key={a.id} value={a.name} label={a.deployed ? undefined : copy.notDeployed} />
        ))}
      </datalist>
      <p role="status" aria-live="polite" className="sr-only">
        {said}
      </p>
    </div>
  )
}
