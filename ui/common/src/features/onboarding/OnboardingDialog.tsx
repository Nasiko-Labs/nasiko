/**
 * The onboarding guide (docs/superpowers/specs/2026-10-01-login-onboarding-design.md §3): the prototype's dialog, a
 * left rail of five steps beside the step itself, with Back / Skip this step / Continue below. Lazy (index.ts).
 */
import { useNavigate } from '@tanstack/react-router'
import {
  ArrowLeft,
  ArrowRight,
  Check,
  Clock,
  Info,
  LayoutDashboard,
  Play,
  Plug,
  Scissors,
  ShieldCheck,
  Sparkles,
  WandSparkles,
  X,
} from 'lucide-react'
import { useRef, useState } from 'react'
import { NAV_ITEMS } from '@/app/shell/nav'
import { NasikoMark } from '@/app/shell/NasikoMark'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent } from '@/components/ui/dialog'
import { Progress } from '@/components/ui/progress'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Spinner } from '@/components/ui/spinner'
import type { DeployStarted } from '@/features/deploy/UploadTab'
import { cn } from '@/lib/utils'
import { useGuide, useSavePersona } from './api'
import { AgentStep } from './AgentStep'
import { OptimiseStep } from './OptimiseStep'
import { copy } from './copy'
import { minutesLeft, opensFor, STEPS, type StepId } from './logic'
import { ModelStep } from './ModelStep'
import { StepHeading } from './parts'
import { PERSONAS, type Persona } from './types'

const STAGE_ICONS = [Plug, WandSparkles, Play, ShieldCheck]
/** When each stage lights in the beam's 4 s pass (index.css `--animate-beam` / `--animate-stage`): negative, so the
 *  loop is already running when the step opens. */
const STAGE_DELAYS = ['-3.8s', '-2.73s', '-1.67s', '-0.6s']
const pageLabel = (to: string) => NAV_ITEMS.find((i) => i.to === to)?.label ?? to

export function OnboardingDialog({
  initialStep,
  onClose,
}: {
  initialStep: StepId
  /** Skip guide, the close button, Escape, finishing and leaving through a link all end here. */
  onClose: () => void
}) {
  const navigate = useNavigate()
  const guide = useGuide()
  const save = useSavePersona()
  const [index, setIndex] = useState(STEPS.indexOf(initialStep))
  const [reached, setReached] = useState(index)
  const [persona, setPersona] = useState<Persona | null>(guide.persona)
  const [model, setModel] = useState<string | null>(null)
  const [agent, setAgent] = useState<DeployStarted | null>(null)
  const step = STEPS[index] ?? 'welcome'
  const primary = useRef<HTMLButtonElement>(null)
  const go = (i: number) => {
    setIndex(i)
    setReached((r) => Math.max(r, i))
  }
  const next = () => go(index + 1)

  const onContinue = () => {
    if (step === 'ready') return onClose()
    if (step !== 'role') return next()
    if (!persona) return
    // Already saved (back on this step): nothing to send.
    if (persona === guide.persona) return next()
    save.mutate(persona, { onSuccess: next })
  }
  const opens = persona ? opensFor(persona) : '/'

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        showCloseButton={false}
        // Start on the step's own action (Get started), not the rail's first button.
        onOpenAutoFocus={(e) => {
          e.preventDefault()
          primary.current?.focus()
        }}
        className="grid h-[min(45rem,calc(100svh-2rem))] grid-rows-1 gap-0 overflow-hidden p-0 sm:max-w-6xl md:grid-cols-[17rem_minmax(0,1fr)]"
      >
        <aside className="hidden min-h-0 flex-col gap-6 border-r bg-muted/60 p-5 md:flex">
          <div className="flex items-center gap-3">
            <NasikoMark className="size-7 shrink-0 text-logo" />
            <div className="min-w-0">
              <p className="text-sm font-semibold">{copy.guide.title}</p>
              <p className="text-xs text-muted-foreground">{copy.guide.meta}</p>
            </div>
          </div>
          <nav aria-label={copy.guide.rail} className="flex min-h-0 flex-1 flex-col gap-1">
            <p className="mb-1 px-2 text-xs font-medium tracking-wide text-muted-foreground uppercase">
              {copy.guide.rail}
            </p>
            {STEPS.map((id, i) => {
              // Ticked only when done, not merely passed: a skipped model or agent stays a number.
              const done =
                i !== index &&
                (id === 'welcome'
                  ? reached > 0
                  : id === 'role'
                    ? !!persona && persona === guide.persona
                    : id === 'model'
                      ? !!model
                      : id === 'agent' && !!agent)
              return (
                <Button
                  key={id}
                  type="button"
                  variant="ghost"
                  disabled={i > reached}
                  aria-current={i === index ? 'step' : undefined}
                  onClick={() => go(i)}
                  className={cn(
                    'h-auto justify-start gap-3 px-2 py-2 text-left whitespace-normal',
                    i === index && 'bg-background shadow-xs hover:bg-background',
                  )}
                >
                  <span
                    className={cn(
                      'flex size-6 shrink-0 items-center justify-center rounded-full border text-xs',
                      i === index && 'border-primary bg-primary text-primary-foreground',
                      done && 'border-transparent bg-foreground/10',
                    )}
                  >
                    {done ? <Check aria-hidden className="size-3.5" /> : i + 1}
                  </span>
                  <span className="min-w-0">
                    <span className="block text-sm font-medium">{copy.steps[id].title}</span>
                    <span className="block text-xs font-normal text-muted-foreground">
                      {copy.steps[id].sub}
                    </span>
                  </span>
                </Button>
              )
            })}
          </nav>
          <div className="flex flex-col gap-2">
            <Progress
              value={((index + 1) / STEPS.length) * 100}
              aria-label={copy.guide.step(index + 1, STEPS.length)}
            />
            <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
              <Clock aria-hidden className="size-3.5" />
              {copy.guide.left(minutesLeft(index + 1))}
            </p>
          </div>
        </aside>

        <div className="flex min-h-0 flex-col">
          <div className="flex items-center gap-2 border-b px-6 py-3">
            <span className="text-sm text-muted-foreground">
              {copy.guide.step(index + 1, STEPS.length)}
            </span>
            <span className="flex-1" />
            {step !== 'ready' ? (
              <Button type="button" variant="ghost" size="sm" onClick={onClose}>
                {copy.guide.skipGuide}
              </Button>
            ) : null}
            <Button
              type="button"
              variant="outline"
              size="icon-sm"
              aria-label={copy.guide.close}
              onClick={onClose}
            >
              <X aria-hidden />
            </Button>
          </div>

          <div className="min-h-0 flex-1 overflow-y-auto px-6 py-8 md:px-10">
            {step === 'welcome' ? <Welcome /> : null}
            {step === 'role' ? (
              <Role
                persona={persona}
                onPick={(p) => {
                  setPersona(p)
                  save.reset()
                }}
                failed={save.isError}
              />
            ) : null}
            {step === 'model' ? <ModelStep connected={model} onConnected={setModel} /> : null}
            {step === 'agent' ? (
              <AgentStep started={agent} onStarted={setAgent} onLeave={onClose} />
            ) : null}
            {step === 'optimise' ? (
              <OptimiseStep
                onOpenAgents={() => {
                  onClose()
                  void navigate({ to: '/agents' })
                }}
              />
            ) : null}
            {step === 'ready' ? (
              <Ready
                rows={[
                  {
                    step: 'role',
                    label: copy.ready.role,
                    value: persona ? copy.role.personas[persona].title : null,
                  },
                  {
                    step: 'model',
                    label: copy.ready.model,
                    value: model,
                  },
                  {
                    step: 'agent',
                    label: copy.ready.agent,
                    value: agent ? copy.ready.building : null,
                  },
                ]}
                onEdit={(s) => go(STEPS.indexOf(s))}
                opens={opens}
                onOverview={onClose}
                onOpen={() => {
                  onClose()
                  void navigate({ to: opens })
                }}
              />
            ) : null}
          </div>

          <div className="flex items-center gap-2 border-t px-6 py-3">
            {index > 0 ? (
              <Button type="button" variant="ghost" onClick={() => go(index - 1)}>
                <ArrowLeft aria-hidden />
                {copy.guide.back}
              </Button>
            ) : null}
            <span className="flex-1" />
            {step === 'model' && !model ? (
              <span className="hidden text-sm text-muted-foreground sm:inline">
                {copy.model.later}
              </span>
            ) : null}
            {(step === 'model' && !model) || (step === 'agent' && !agent) ? (
              <Button type="button" variant="outline" onClick={next}>
                {copy.guide.skipStep}
              </Button>
            ) : null}
            {/* Informational, so there is nothing to complete: the opt-out is worded as a
                decision rather than as skipping an unfinished task. */}
            {step === 'optimise' ? (
              <Button type="button" variant="outline" onClick={next}>
                {copy.optimise.skip}
              </Button>
            ) : null}
            <Button
              ref={primary}
              type="button"
              disabled={(step === 'role' && !persona) || save.isPending}
              onClick={onContinue}
            >
              {save.isPending ? <Spinner aria-hidden /> : null}
              {step === 'welcome'
                ? copy.guide.getStarted
                : step === 'ready'
                  ? copy.guide.finish
                  : copy.guide.continue}
              <ArrowRight aria-hidden />
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}

function Welcome() {
  return (
    <div className="flex flex-col gap-6">
      <Badge variant="outline" className="gap-1.5 rounded-full px-3 py-1 font-normal">
        <Sparkles aria-hidden />
        {copy.welcome.badge}
      </Badge>
      <StepHeading title={copy.welcome.title} intro={copy.welcome.intro} />
      {/* The four stages as one flow (user review 2026-10-01): our take on Aceternity's Tracing Beam. A beam runs along
          the line from Connect to Govern and each stage's icon lights as it passes; under reduced motion Connect stays
          lit, the stage the next steps set up. Decorative: the words carry the meaning. */}
      <div className="relative pt-2">
        <div
          aria-hidden
          className="pointer-events-none absolute top-7 right-[calc(25%-2.375rem)] left-5 hidden h-px overflow-hidden bg-border sm:block"
        >
          <div className="absolute inset-y-0 w-1/4 animate-beam bg-linear-to-r from-transparent via-primary to-transparent motion-reduce:hidden" />
        </div>
        <ol className="grid gap-x-6 gap-y-6 sm:grid-cols-4">
          {copy.welcome.stages.map((s, i) => {
            const Icon = STAGE_ICONS[i] ?? Plug
            return (
              <li key={s.title} className="relative flex flex-col gap-3">
                <span
                  aria-hidden
                  style={{ animationDelay: STAGE_DELAYS[i] }}
                  className={cn(
                    'flex size-10 animate-stage items-center justify-center rounded-xl border bg-muted text-foreground ring-4 ring-background motion-reduce:animate-none',
                    i === 0 &&
                      'motion-reduce:border-primary motion-reduce:bg-primary motion-reduce:text-primary-foreground',
                  )}
                >
                  <Icon className="size-[18px]" />
                </span>
                <div>
                  <p className="flex items-baseline gap-2 font-medium">
                    {s.title}
                    <span className="font-mono text-[11px] font-normal text-muted-foreground">{`0${i + 1}`}</span>
                  </p>
                  <p className="text-sm text-pretty text-muted-foreground">{s.line}</p>
                </div>
              </li>
            )
          })}
        </ol>
      </div>
      <p className="flex items-center gap-2 text-sm text-muted-foreground">
        <Info aria-hidden className="size-4 shrink-0" />
        {copy.welcome.note}
      </p>
    </div>
  )
}

function Role({
  persona,
  onPick,
  failed,
}: {
  persona: Persona | null
  onPick: (p: Persona) => void
  failed: boolean
}) {
  return (
    <div className="flex flex-col gap-6">
      <StepHeading title={copy.role.title} intro={copy.role.intro} />
      {failed ? (
        <Alert variant="destructive">
          <AlertDescription>{copy.role.saveFailed}</AlertDescription>
        </Alert>
      ) : null}
      <RadioGroup
        aria-label={copy.role.label}
        value={persona ?? ''}
        onValueChange={(v) => onPick(v as Persona)}
        className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3"
      >
        {PERSONAS.map((p) => (
          <label
            key={p}
            htmlFor={`persona-${p}`}
            className="flex cursor-pointer items-start gap-3 rounded-xl border bg-card p-4 hover:bg-accent/50 has-[[data-state=checked]]:border-foreground/50 has-[[data-state=checked]]:bg-accent"
          >
            <RadioGroupItem id={`persona-${p}`} value={p} className="mt-0.5" />
            <span className="flex min-w-0 flex-col gap-1">
              <span className="text-sm font-medium">{copy.role.personas[p].title}</span>
              <span className="text-xs text-muted-foreground">{copy.role.personas[p].line}</span>
              <span className="text-xs text-muted-foreground">
                {copy.role.opens(pageLabel(opensFor(p)))}
              </span>
            </span>
          </label>
        ))}
      </RadioGroup>
    </div>
  )
}

function Ready({
  rows,
  onEdit,
  opens,
  onOverview,
  onOpen,
}: {
  rows: { step: StepId; label: string; value: string | null }[]
  onEdit: (step: StepId) => void
  opens: string
  onOverview: () => void
  onOpen: () => void
}) {
  return (
    <div className="flex flex-col gap-6">
      <span className="flex size-12 items-center justify-center rounded-full bg-primary text-primary-foreground">
        <Check aria-hidden className="size-6" />
      </span>
      <StepHeading title={copy.ready.title} intro={copy.ready.intro} />
      <ul className="divide-y rounded-xl border bg-card">
        {rows.map((r) => (
          <li key={r.step} className="flex items-center gap-3 px-4 py-3 text-sm">
            <span
              className={cn(
                'flex size-5 shrink-0 items-center justify-center rounded-full',
                r.value ? 'bg-primary text-primary-foreground' : 'border',
              )}
            >
              {r.value ? <Check aria-hidden className="size-3" /> : null}
            </span>
            <span className="w-20 text-muted-foreground">{r.label}</span>
            <span className="flex-1 font-medium">{r.value ?? copy.ready.skipped}</span>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              aria-label={copy.ready.edit(r.label)}
              onClick={() => onEdit(r.step)}
            >
              {copy.ready.editShort}
            </Button>
          </li>
        ))}
      </ul>
      <div className="rounded-xl border border-dashed bg-muted/40 px-4 py-3 text-sm">
        <p className="flex items-center gap-2 font-medium">
          <Scissors aria-hidden className="size-4 text-muted-foreground" />
          {copy.ready.savings.title}
        </p>
        <p className="mt-1 text-xs text-muted-foreground">{copy.ready.savings.line}</p>
        <p className="mt-1 text-xs text-muted-foreground italic">{copy.ready.savings.beta}</p>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <Choice
          icon={LayoutDashboard}
          title={copy.ready.overview}
          line={copy.ready.overviewLine}
          onClick={onOverview}
        />
        {opens !== '/' ? (
          <Choice
            icon={ArrowRight}
            title={copy.ready.open(pageLabel(opens))}
            line={copy.ready.openLine}
            onClick={onOpen}
          />
        ) : null}
      </div>
    </div>
  )
}

function Choice({
  icon: Icon,
  title,
  line,
  onClick,
}: {
  icon: typeof LayoutDashboard
  title: string
  line: string
  onClick: () => void
}) {
  return (
    <Button
      type="button"
      variant="outline"
      onClick={onClick}
      className="h-auto justify-start gap-3 rounded-xl p-4 text-left whitespace-normal"
    >
      <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
        <Icon aria-hidden className="size-4" />
      </span>
      <span className="min-w-0">
        <span className="block font-medium">{title}</span>
        <span className="block text-xs font-normal text-muted-foreground">{line}</span>
      </span>
    </Button>
  )
}
