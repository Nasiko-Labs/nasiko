/**
 * First run (design review 7A): no agents yet, so one full-width card with the Agents first-run commands (checked
 * against recorded CLI help) replaces the data cards, whose queries don't run. Deploy an agent comes first, the CLI steps
 * are the alternative (plans/feat-deploy.md §7). It spans two columns, so Quick actions sits beside it in the 3-column grid.
 */
import { Link } from '@tanstack/react-router'
import { ChevronRight } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { FirstRunSteps } from '@/features/agents/components/bits'
import { firstRunCommands } from '@/features/agents/format'
import { DeployAgentButton } from '@/features/deploy/components/DeployAgentButton'
import { copy as deployCopy } from '@/features/deploy/copy'
import { useGuide } from '@/features/onboarding/api'
import { cn } from '@/lib/utils'
import { GuideSteps } from '@/features/onboarding/GuideCard'
import { copy } from '../copy'
import { Card, TOUCH } from './Card'

/** How many commands the CLI path takes (connect, new, deploy). */
const CLI_STEPS = firstRunCommands('').length

/**
 * The first-run headline and, with the guide's card below, the page's one primary action beside it: Deploy an agent,
 * with the CLI as the alternative. The CLI opens to all its steps (connect, new, deploy): the deploy command alone
 * fails on a machine that hasn't run the first two. On an older server the deploy card is that action, so the lead is
 * text.
 */
export function FirstRunLead() {
  const { absent } = useGuide()
  const [cli, setCli] = useState(false)
  return (
    <div className="flex flex-col items-start gap-3">
      <p className="max-w-3xl text-lg text-pretty" data-testid="overview-headline">
        {copy.firstRun.headline}
      </p>
      {absent ? null : (
        <Collapsible open={cli} onOpenChange={setCli} className="flex flex-col items-start gap-3">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-2">
            <DeployAgentButton className={TOUCH} />
            <CollapsibleTrigger asChild>
              <Button variant="ghost" size="sm" className={`text-muted-foreground ${TOUCH}`}>
                <ChevronRight
                  aria-hidden
                  className={cn(
                    'transition-transform motion-reduce:transition-none',
                    cli && 'rotate-90',
                  )}
                />
                {copy.firstRun.cli(CLI_STEPS)}
              </Button>
            </CollapsibleTrigger>
          </div>
          <CollapsibleContent className="overflow-hidden data-[state=closed]:animate-collapsible-up data-[state=open]:animate-collapsible-down motion-reduce:animate-none">
            <FirstRunSteps />
          </CollapsibleContent>
        </Collapsible>
      )}
    </div>
  )
}

export function FirstRun() {
  // The onboarding guide covers deploying (its Bring an agent step), so the Setup guide card leads instead (spec §4);
  // servers without the endpoint keep the deploy card.
  const { absent } = useGuide()
  if (!absent)
    return (
      <Card
        id="overview-setup-guide"
        title={copy.setup.button}
        className="@[700px]/overview:col-span-2"
      >
        <GuideSteps buttonClassName={TOUCH} />
      </Card>
    )
  return (
    <Card
      id="overview-first-run"
      title={copy.firstRun.title}
      to="/agents"
      linkLabel={copy.firstRun.link}
      className="@[700px]/overview:col-span-2"
    >
      <DeployAgentButton size="default" className={TOUCH} />
      <p className="mt-4 mb-2 text-xs text-muted-foreground">{deployCopy.entry.orCli}</p>
      <FirstRunSteps />
      <p className="mt-3 flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
        {copy.firstRun.after}{' '}
        <Button asChild variant="link" size="sm" className={`h-auto px-0 text-xs ${TOUCH}`}>
          <Link to="/agents">{copy.firstRun.link}</Link>
        </Button>
      </p>
    </Card>
  )
}

const PREVIEW = [
  ['needs', copy.needs.title, copy.preview.needs],
  ['spend', copy.spend.title, copy.preview.spend],
  ['health', copy.health.title, copy.preview.health],
] as const

/**
 * What the Overview's lead cards show once an agent runs, in their page order: a dashed frame (a place, not a card)
 * with each card's name and one sentence. No numbers: the server has none yet (Honest numbers).
 */
export function FirstRunPreview({ className }: { className?: string }) {
  return (
    <section
      aria-labelledby="overview-preview"
      data-testid="overview-preview"
      className={cn('rounded-lg border border-dashed p-4', className)}
    >
      <h2 id="overview-preview" className="text-sm font-semibold">
        {copy.preview.title}
      </h2>
      <ul className="mt-3 grid gap-4 @[700px]/overview:grid-cols-3 @[700px]/overview:gap-6">
        {PREVIEW.map(([id, title, text]) => (
          <li key={id} className="flex flex-col gap-1">
            <h3 className="text-sm font-medium">{title}</h3>
            <p className="text-sm text-pretty text-muted-foreground">{text}</p>
          </li>
        ))}
      </ul>
    </section>
  )
}
