/**
 * First run (design review 7A; redesign 2026-10-21): no agents yet, so the data cards, whose queries don't run, give way
 * to one hero card (the page's one effect area, as the summary's lead card is once agents run). Its left half is the
 * page's one primary action, Deploy an agent, with the CLI as the alternative (plans/feat-deploy.md §7); its right half
 * is the Setup guide where the server has it, or the CLI steps where it doesn't. Under it, a preview of the cards a
 * deploy brings, drawn without numbers.
 */
import { ChevronRight } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { SpotlightCard } from '@nasiko/ui/composites/spotlight-card'
import { Button } from '@nasiko/ui/primitives/button'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@nasiko/ui/primitives/collapsible'
import { FirstRunSteps } from '@nasiko/features/agents/components/bits'
import { firstRunCommands } from '@nasiko/features/agents/format'
import { DeployAgentButton } from '@nasiko/features/deploy/components/DeployAgentButton'
import { useGuide } from '@nasiko/features/onboarding/api'
import { GuideSteps } from '@nasiko/features/onboarding/GuideCard'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import { TOUCH } from './Card'

/** How many commands the CLI path takes (connect, new, deploy). */
const CLI_STEPS = firstRunCommands('').length

/** The hero's right half: a hairline apart, never a card inside the card. */
function Side({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <section
      aria-labelledby={`${id}-title`}
      data-testid={id}
      data-tour={id}
      className="flex flex-col gap-3 border-t bg-card/80 p-5 @[900px]/overview:border-t-0 @[900px]/overview:border-l @[900px]/overview:p-7"
    >
      <h3 id={`${id}-title`} className="text-sm font-semibold">
        {title}
      </h3>
      {children}
    </section>
  )
}

/**
 * The hero. The CLI opens to all its steps (connect, new, deploy): the deploy command alone fails on a machine that
 * hasn't run the first two. On an older server (no guide) those steps fill the right half, so there is no disclosure.
 */
export function FirstRunHero() {
  const { absent } = useGuide()
  const [cli, setCli] = useState(false)
  return (
    <SpotlightCard>
      <section
        aria-labelledby="overview-first-run-title"
        data-testid="overview-first-run"
        className="relative grid @[900px]/overview:grid-cols-[minmax(0,7fr)_minmax(0,5fr)]"
      >
        <div className="flex flex-col items-start justify-center gap-5 p-5 @[900px]/overview:p-8">
          <div className="flex max-w-xl flex-col gap-2">
            <h2
              id="overview-first-run-title"
              className="text-2xl font-semibold tracking-tight text-balance @[900px]/overview:text-3xl"
            >
              {copy.firstRun.title}
            </h2>
            <p
              className="text-base text-pretty text-muted-foreground"
              data-testid="overview-headline"
            >
              {copy.firstRun.headline}
            </p>
          </div>
          {absent ? (
            <DeployAgentButton size="lg" className={TOUCH} />
          ) : (
            <Collapsible
              open={cli}
              onOpenChange={setCli}
              className="flex flex-col items-start gap-3"
            >
              <div className="flex flex-wrap items-center gap-2">
                <DeployAgentButton size="lg" className={TOUCH} />
                <CollapsibleTrigger
                  render={<Button variant="ghost" className={`text-muted-foreground ${TOUCH}`} />}
                >
                  <ChevronRight
                    aria-hidden
                    className={cn(
                      'transition-transform motion-reduce:transition-none',
                      cli && 'rotate-90',
                    )}
                  />
                  {copy.firstRun.cli(CLI_STEPS)}
                </CollapsibleTrigger>
              </div>
              <CollapsibleContent className="overflow-hidden [--radix-collapsible-content-height:var(--collapsible-panel-height)] motion-reduce:animate-none data-open:animate-collapsible-down data-closed:animate-collapsible-up">
                <FirstRunSteps />
              </CollapsibleContent>
            </Collapsible>
          )}
        </div>
        {/* The onboarding guide covers deploying (its Deploy an agent step), so it fills the right half (spec §4). */}
        {absent ? (
          <Side id="overview-cli" title={copy.firstRun.cliTitle}>
            <FirstRunSteps />
          </Side>
        ) : (
          <Side id="overview-setup-guide" title={copy.setup.button}>
            <GuideSteps buttonClassName={TOUCH} />
          </Side>
        )}
      </section>
    </SpotlightCard>
  )
}

/** A short placeholder bar: the sketches' one mark. */
const bar = 'h-2 rounded-full bg-muted-foreground/20'

/** Fixed heights, a shape and not data (Honest numbers): the Spend sketch draws no value. */
const SPEND_SHAPE = [35, 50, 42, 60, 48, 70, 55, 66, 80, 62, 74, 90, 68, 84].map((h, day) => ({
  day,
  h,
}))

/** Line drawings of each card, decorative and number-free, in the card's own colours at low strength. */
const SKETCH: Record<'needs' | 'spend' | 'health', ReactNode> = {
  needs: (
    <div className="flex h-full flex-col justify-center gap-3">
      {(
        [
          ['bg-destructive/60', 'w-3/5'],
          ['bg-warning/60', 'w-2/5'],
          ['bg-muted-foreground/30', 'w-1/2'],
        ] as const
      ).map(([dot, width]) => (
        <div key={width} className="flex items-center gap-2.5">
          <span className={cn('size-2 shrink-0 rounded-full', dot)} />
          <span className={cn(bar, width)} />
          <span className={cn(bar, 'ml-auto w-8')} />
        </div>
      ))}
    </div>
  ),
  spend: (
    <div className="flex h-full items-end gap-1">
      {SPEND_SHAPE.map(({ day, h }) => (
        <span key={day} className="flex-1 rounded-t-sm bg-chart-1/30" style={{ height: `${h}%` }} />
      ))}
    </div>
  ),
  health: (
    <div className="flex h-full flex-col justify-center gap-3">
      <div className="flex h-2 gap-0.5 overflow-hidden rounded-full">
        <span className="w-3/5 bg-success/45" />
        <span className="w-1/4 bg-warning/45" />
        <span className="flex-1 bg-destructive/45" />
      </div>
      {(['bg-success/20', 'bg-warning/20'] as const).map((pill, i) => (
        <div key={pill} className="flex items-center gap-2.5">
          <span className={cn('h-4 w-12 shrink-0 rounded-full', pill)} />
          <span className={cn(bar, i ? 'w-1/3' : 'w-1/2')} />
        </div>
      ))}
    </div>
  ),
}

const PREVIEW = [
  ['needs', copy.needs.title, copy.preview.needs],
  ['spend', copy.spend.title, copy.preview.spend],
  ['health', copy.health.title, copy.preview.health],
] as const

/**
 * What the Overview's lead cards show once an agent runs, in their page order: dashed frames (places, not cards), each
 * with a drawing of the card, its name and one sentence. No numbers: the server has none yet (Honest numbers).
 */
export function FirstRunPreview({ className }: { className?: string }) {
  return (
    <section
      aria-labelledby="overview-preview"
      data-testid="overview-preview"
      className={cn('flex flex-col gap-3', className)}
    >
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h2 id="overview-preview" className="text-sm font-semibold">
          {copy.preview.title}
        </h2>
        <p className="text-xs text-muted-foreground">{copy.preview.note}</p>
      </div>
      <ul className="grid gap-3 @[700px]/overview:grid-cols-3">
        {PREVIEW.map(([id, title, text]) => (
          <li key={id} className="flex flex-col overflow-hidden rounded-lg border border-dashed">
            <div aria-hidden className="h-24 border-b border-dashed bg-muted/40 px-5 py-4">
              {SKETCH[id]}
            </div>
            <div className="flex flex-col gap-1 p-4">
              <h3 className="text-sm font-medium">{title}</h3>
              <p className="text-sm text-pretty text-muted-foreground">{text}</p>
            </div>
          </li>
        ))}
      </ul>
    </section>
  )
}
