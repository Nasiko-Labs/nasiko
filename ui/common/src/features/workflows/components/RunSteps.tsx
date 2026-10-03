/**
 * A run's timeline: one row per step result, then MAF's own planning and synthesis tokens, so the header total
 * reconciles with the step chips. `labels` titles steps by their task (step_id → text); a step whose id no longer
 * matches (every save regenerates ids) keeps its agent as the title. A step's requests use chat's `RequestCard`:
 * decided ones render as receipts, the waiting one is answered in place.
 */
import { Circle, CircleCheck, CircleX, WandSparkles } from 'lucide-react'
import { useState } from 'react'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Markdown } from '@/features/chat/components/Markdown'
import { RequestCard, type RequestActions } from '@/features/chat/components/RequestCard'
import type { HitlDto } from '@/features/chat/types'
import { fmtDuration, fmtTokens } from '@/lib/format'
import { cn } from '@/lib/utils'
import { cancelHitl, resolveHitl } from '../api'
import { copy } from '../copy'
import { hitlByStep, planningTokens, stepStatus } from '../logic'
import type { StepResult } from '../types'
import { Chip, ToneBadge } from './bits'

function Glyph({ status, n }: { status: string; n: number }) {
  if (status === 'success') return <CircleCheck aria-hidden className="size-4 text-success" />
  if (status === 'failed') return <CircleX aria-hidden className="size-4 text-destructive" />
  if (status === 'stopped') return <Circle aria-hidden className="size-4 text-muted-foreground" />
  return (
    <span
      aria-hidden
      className={cn(
        'grid size-5 place-content-center rounded-full border text-[11px] tabular-nums',
        status === 'running' && 'border-primary text-primary-text',
        status === 'awaiting_human' && 'border-warning text-warning',
      )}
    >
      {n}
    </span>
  )
}

const ERROR_BOX =
  'rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm break-words whitespace-pre-wrap text-destructive'

/** Output, prompt or error: the error wins outright (no tabs that do nothing). */
function Panes({ step }: { step: StepResult }) {
  const [tab, setTab] = useState(step.extracted_info ? 'output' : 'prompt')
  if (step.error) return <p className={ERROR_BOX}>{step.error}</p>
  const output = step.extracted_info ? <Markdown text={step.extracted_info} /> : null
  const prompt = step.prompt ? (
    <pre
      // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- a scrollable region must be keyboard-reachable (axe scrollable-region-focusable)
      tabIndex={0}
      role="region"
      aria-label={copy.promptFor(step.step_index + 1)}
      className="max-h-80 overflow-auto rounded-md bg-muted/50 p-3 font-mono text-xs whitespace-pre-wrap"
    >
      {step.prompt}
    </pre>
  ) : null
  if (!output || !prompt) return output ?? prompt
  return (
    <Tabs value={tab} onValueChange={setTab} className="gap-2">
      <TabsList>
        <TabsTrigger value="output">{copy.output}</TabsTrigger>
        <TabsTrigger value="prompt">{copy.prompt}</TabsTrigger>
      </TabsList>
      <TabsContent value="output">{output}</TabsContent>
      <TabsContent value="prompt">{prompt}</TabsContent>
    </Tabs>
  )
}

function Detail({ step, well }: { step: StepResult; well: React.ReactNode }) {
  if (step.status === 'awaiting_human')
    return (
      <>
        {step.prompt ? (
          <p className="text-sm break-words text-muted-foreground">{step.prompt}</p>
        ) : null}
        {well}
      </>
    )
  // A stopped step shows what it got to, like a finished one.
  if (step.status === 'success' || step.status === 'failed' || step.status === 'stopped')
    return (
      <>
        {well}
        <Panes step={step} />
      </>
    )
  return well
}

export function RunSteps({
  steps,
  labels,
  totalTokens,
  hitl,
  onHitlChange,
}: {
  steps: StepResult[]
  labels?: Record<string, string>
  totalTokens: number
  hitl?: HitlDto[]
  /** A request was answered or withdrawn: refetch the run now. */
  onHitlChange: () => void
}) {
  if (!steps.length) return null
  const byStep = hitlByStep(steps, hitl)
  const remainder = planningTokens(totalTokens, steps)
  const actions: RequestActions = {
    resolve: resolveHitl,
    cancel: cancelHitl,
    onDone: onHitlChange,
  }
  return (
    <ol aria-label={copy.stepsLabel} className="flex flex-col">
      {steps.map((s, i) => {
        const label = labels?.[s.step_id]
        const rows = byStep.get(s.step_index) ?? []
        const last = i === steps.length - 1 && remainder <= 0
        const status = stepStatus(s.status)
        const well = rows.length ? (
          <div data-hitl-step={s.step_index} className="flex flex-col gap-2">
            {rows.map((r) => (
              // Keyed by status too: a decided row becomes a receipt; a waiting one is never remounted by a poll.
              <RequestCard
                key={`${r.id}:${r.status}`}
                id={`request-${r.id}`}
                request={r}
                agentName={s.agent_name || copy.thisAgent}
                actions={actions}
              />
            ))}
          </div>
        ) : null
        return (
          <li key={s.step_id || s.step_index} className="flex gap-3">
            <div className="flex w-5 shrink-0 flex-col items-center pt-0.5">
              <Glyph status={s.status} n={i + 1} />
              {!last ? <span aria-hidden className="mt-1 w-px flex-1 bg-border" /> : null}
            </div>
            <div className="flex min-w-0 flex-1 flex-col gap-2 pb-5">
              <div className="flex flex-wrap items-center gap-1.5">
                <h3 className="mr-1 text-sm font-medium break-words">
                  {copy.stepTitle(i + 1, label || s.agent_name || copy.unassigned)}
                </h3>
                {label && s.agent_name ? <Chip>{s.agent_name}</Chip> : null}
                <ToneBadge tone={status.tone}>{status.label}</ToneBadge>
                {s.latency_ms ? <Chip>{fmtDuration(s.latency_ms)}</Chip> : null}
                {s.tokens_used ? <Chip>{copy.tokens(fmtTokens(s.tokens_used))}</Chip> : null}
              </div>
              <Detail step={s} well={well} />
            </div>
          </li>
        )
      })}
      {remainder > 0 ? (
        <li className="flex gap-3">
          <div className="flex w-5 shrink-0 justify-center pt-0.5">
            <WandSparkles aria-hidden className="size-4 text-muted-foreground" />
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <h3 className="mr-1 text-sm font-medium">{copy.planning}</h3>
            <Chip>{copy.tokens(fmtTokens(remainder))}</Chip>
          </div>
        </li>
      ) : null}
    </ol>
  )
}
