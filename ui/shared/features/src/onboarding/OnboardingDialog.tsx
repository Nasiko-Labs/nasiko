/**
 * The onboarding guide (docs/superpowers/specs/2026-10-01-login-onboarding-design.md §3): the prototype's dialog, a
 * left rail of five steps beside the step itself, with Back / Continue below. Lazy (index.ts).
 */
import { useNavigate } from '@tanstack/react-router'
import {
  ArrowLeft,
  ArrowRight,
  Bot,
  Check,
  Clock,
  DollarSign,
  Gauge,
  ListTree,
  MessageSquare,
  Waypoints,
  X,
  type LucideIcon,
} from 'lucide-react'
import { useRef, useState } from 'react'
import { NAV_ITEMS } from '@nasiko/app-core/shell/nav'
import { DotBackground } from '@nasiko/ui/composites/dot-background'
import { GlowingEffect } from '@nasiko/ui/composites/glowing-effect'
import { NasikoMark } from '@nasiko/ui/composites/NasikoMark'
import { TextGenerateEffect } from '@nasiko/ui/composites/text-generate-effect'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { Button } from '@nasiko/ui/primitives/button'
import { Dialog, DialogContent } from '@nasiko/ui/primitives/dialog'
import { Progress } from '@nasiko/ui/primitives/progress'
import { RadioGroup, RadioGroupItem } from '@nasiko/ui/primitives/radio-group'
import { Spinner } from '@nasiko/ui/primitives/spinner'
import type { DeployStarted } from '@nasiko/features/deploy/UploadTab'
import { useConfigs } from '@nasiko/features/router/api'
import { cn } from '@nasiko/ui/lib/utils'
import { useGuide, useSavePersona } from './api'
import { AgentStep } from './AgentStep'
import { OptimiseStep } from './OptimiseStep'
import { copy } from './copy'
import { minutesLeft, opensFor, STEPS, type PersonaPage, type StepId } from './logic'
import { ModelStep } from './ModelStep'
import { StepHeading } from './parts'
import { PERSONAS, type Persona } from './types'

/** The Welcome rows' icons: the nav's own for Agents, Monitor, LLM router, Optimize, Sessions and Chat. */
const ROW_ICONS = [Bot, DollarSign, Waypoints, Gauge, ListTree, MessageSquare]
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
  const configs = useConfigs()
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
  // Nasiko provides no model credits, so an agent answers nothing until the user has a key of their own.
  const hasKey = !!model || !!configs.data?.length
  // A key from before the guide is the model Ready shows, not a skipped step.
  const current = configs.data?.find((c) => c.is_default) ?? configs.data?.[0]
  /** Out of the guide to a page: closing first, so the guide never stays open over it. */
  const leave = (to: Parameters<typeof navigate>[0]) => {
    onClose()
    void navigate(to)
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        showCloseButton={false}
        overlayClassName="backdrop-blur-sm"
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
                    : id === 'start' && (!!model || !!agent))
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

          <div className="relative min-h-0 flex-1 overflow-y-auto px-6 py-8 md:px-10">
            {step === 'welcome' ? (
              <>
                <DotBackground />
                <Welcome />
              </>
            ) : null}
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
            {step === 'start' ? (
              <div className="flex flex-col gap-10">
                <StepHeading title={copy.start.title} intro={copy.start.intro} />
                <ModelStep connected={model} onConnected={setModel} />
                <AgentStep
                  started={agent}
                  onStarted={setAgent}
                  hasKey={hasKey}
                  onChat={(agent) => {
                    onClose()
                    void navigate({ to: '/chat', search: { agent } })
                  }}
                  onLeave={onClose}
                />
              </div>
            ) : null}
            {step === 'optimize' ? (
              <OptimiseStep
                onOpenAgents={() => leave({ to: '/agents' })}
                onSavings={() => leave({ to: '/optimization' })}
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
                  { step: 'start', label: copy.ready.model, value: model ?? current?.name ?? null },
                  {
                    step: 'start',
                    label: copy.ready.agent,
                    value: agent ? copy.ready.building : null,
                  },
                ]}
                onEdit={(s) => go(STEPS.indexOf(s))}
                opens={opens}
                hasKey={hasKey}
                onGo={(to) => leave({ to })}
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
            {/* Informational, so there is nothing to complete: the opt-out is worded as a
                decision rather than as skipping an unfinished task. */}
            {step === 'optimize' ? (
              <Button type="button" variant="outline" onClick={next}>
                {copy.optimise.skip}
              </Button>
            ) : null}
            <Button
              ref={primary}
              type="button"
              // No skipping the key: nothing past it works without one. Skip guide is the way out.
              disabled={
                (step === 'role' && !persona) || (step === 'start' && !hasKey) || save.isPending
              }
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

/** The Welcome rows' sequence, in seconds: the first row starts as the title settles, then one row every `ROW_STEP`. */
const ROW_START = 0.4
const ROW_STEP = 0.3

/** Where the panel's band sits: its bottom-left, where the last row ends (degrees clockwise from the top). Still: a
 *  band that followed the rows down the edge read as a crawl (user review 2026-11-04). */
const BAND_ANGLE = 248

function Welcome() {
  const rows = copy.welcome.rows
  return (
    <div className="relative flex flex-col gap-8">
      <StepHeading
        title={<TextGenerateEffect words={copy.welcome.title} />}
        intro={copy.welcome.intro}
      />
      {/* Capability beside outcome (user review 2026-11-04): what Nasiko does, and what that gets you. The outcome is
          the point, so it carries the weight. The step's one effect area, all Aceternity (user request 2026-11-04): the
          dot background behind it, the title's words arriving, and the Glowing Effect on the panel's edge, the tour's
          pairing (a soft blurred band under a sharp one). Once, on open, the rows switch on in turn: the icon, the
          capability, the arrow, then the outcome's words. The band stays still at the bottom-left. Then nothing moves;
          under reduced motion it is all there at once. */}
      <div className="relative rounded-xl">
        <GlowingEffect
          angle={BAND_ANGLE}
          spread={30}
          blur={12}
          borderWidth={4}
          className="opacity-60"
        />
        <GlowingEffect glow angle={BAND_ANGLE} spread={30} borderWidth={2} />
        <div className="relative overflow-hidden rounded-xl border bg-card shadow-sm">
          <div className="hidden grid-cols-2 gap-6 border-b bg-muted/40 px-4 py-2 text-xs font-medium text-muted-foreground sm:grid">
            <span>{copy.welcome.does}</span>
            <span className="pl-6">{copy.welcome.get}</span>
          </div>
          <dl className="divide-y">
            {rows.map((r, i) => {
              const Icon = ROW_ICONS[i] ?? Bot
              const on = ROW_START + i * ROW_STEP
              return (
                <div
                  key={r.does}
                  data-row
                  className="grid gap-1.5 px-4 py-3 sm:grid-cols-2 sm:items-center sm:gap-6"
                >
                  <dt
                    className={cn(
                      'flex items-center gap-3 text-sm text-muted-foreground',
                      SWITCH_ON,
                    )}
                    style={{ animationDelay: `${on}s` }}
                  >
                    <span
                      className={cn(
                        'flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-foreground zoom-in-50',
                        SWITCH_ON,
                      )}
                      style={{ animationDelay: `${on}s` }}
                    >
                      <Icon aria-hidden className="size-4" />
                    </span>
                    {r.does}
                  </dt>
                  <dd className="flex items-center gap-2 pl-11 text-sm font-medium text-pretty sm:pl-0">
                    <ArrowRight
                      aria-hidden
                      className={cn(
                        'hidden size-4 shrink-0 text-muted-foreground slide-in-from-left-2 sm:block',
                        SWITCH_ON,
                      )}
                      style={{ animationDelay: `${on + 0.15}s` }}
                    />
                    <span>
                      <TextGenerateEffect words={r.get} stagger={0.04} delay={on + 0.25} />
                    </span>
                  </dd>
                </div>
              )
            })}
          </dl>
        </div>
      </div>
    </div>
  )
}

/** A Welcome row's part arriving on its turn (tw-animate-css, with the turn as an inline `animation-delay`): hidden
 *  until then, still under reduced motion. */
const SWITCH_ON = 'animate-in fade-in fill-mode-both duration-300 motion-reduce:animate-none'

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
            className="flex cursor-pointer items-start gap-3 rounded-xl border bg-card p-4 hover:bg-accent/50 has-[[data-checked]]:border-foreground/50 has-[[data-checked]]:bg-accent"
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
  hasKey,
  onGo,
}: {
  rows: { step: StepId; label: string; value: string | null }[]
  onEdit: (step: StepId) => void
  opens: PersonaPage
  hasKey: boolean
  onGo: (to: PersonaPage) => void
}) {
  return (
    <div className="flex flex-col gap-6">
      <span className="flex size-12 items-center justify-center rounded-full bg-primary text-primary-foreground">
        <Check aria-hidden className="size-6" />
      </span>
      <StepHeading title={copy.ready.title} intro={copy.ready.intro} />
      <ul className="divide-y rounded-xl border bg-card">
        {rows.map((r) => (
          <li key={r.label} className="flex items-center gap-3 px-4 py-3 text-sm">
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
      {/* Chat and Agents lead for everyone (user review 2026-11-04); the role's own page follows when it is another. */}
      <section className="flex flex-col gap-3">
        <h3 className="text-sm font-medium">{copy.ready.next}</h3>
        <div className="grid grid-cols-[repeat(auto-fit,minmax(14rem,1fr))] gap-3">
          {/* No key, no answer: Chat takes the user back to add one rather than to a chat that fails. */}
          <Choice
            icon={MessageSquare}
            title={copy.ready.chat}
            line={hasKey ? copy.ready.chatLine : copy.ready.chatNeedsKey}
            onClick={() => (hasKey ? onGo('/chat') : onEdit('start'))}
          />
          <Choice
            icon={Bot}
            title={copy.ready.agents}
            line={copy.ready.agentsLine}
            onClick={() => onGo('/agents')}
          />
          {opens !== '/' && opens !== '/chat' && opens !== '/agents' ? (
            <Choice
              icon={ArrowRight}
              title={copy.ready.open(pageLabel(opens))}
              line={copy.ready.openLine}
              onClick={() => onGo(opens)}
            />
          ) : null}
        </div>
      </section>
    </div>
  )
}

function Choice({
  icon: Icon,
  title,
  line,
  onClick,
}: {
  icon: LucideIcon
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
