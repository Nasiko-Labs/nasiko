/**
 * The guide's last step (user request 2026-11-05, in place of Spend less and Ready): the CLI commands that bring a
 * developer's coding harnesses into Coding harnesses: install, then connect to this server (user review 2026-11-05: the
 * rest dropped). The installer is the CLI guide's own. The connect names this page's own origin, since a fresh CLI
 * otherwise signs in to the control plane built into its release (NAS-1464). It is the last command (user request
 * 2026-11-15): `nasiko connect` signs in itself, through SSO where the server has it, and then asks which coding agents
 * to connect, offering to move any connected to another server.
 */
import { Bot, MessageSquare, SquareTerminal } from 'lucide-react'
import { Button } from '@nasiko/ui/primitives/button'
import { copy as guideCopy } from './copy'
import { HarnessSetupSteps } from './HarnessSetup'
import { StepHeading } from './parts'

const copy = guideCopy.harness

/** The nav's own icons for each page. */
const CHOICES = [
  { to: '/harnesses', icon: SquareTerminal, key: 'harnesses' },
  { to: '/chat', icon: MessageSquare, key: 'chat' },
  { to: '/agents', icon: Bot, key: 'agents' },
] as const

export function HarnessStep({
  onOpen,
}: {
  /** Out of the guide to one of these pages. */
  onOpen: (to: '/harnesses' | '/agents' | '/chat') => void
}) {
  return (
    <div className="flex flex-col gap-6">
      <StepHeading title={copy.title} intro={copy.intro} />
      <HarnessSetupSteps />
      <section className="flex flex-col gap-3">
        <h3 className="text-sm font-medium">{copy.next}</h3>
        <div className="grid grid-cols-[repeat(auto-fit,minmax(13rem,1fr))] gap-3">
          {CHOICES.map(({ to, icon: Icon, key }) => (
            <Button
              key={to}
              type="button"
              variant="outline"
              onClick={() => onOpen(to)}
              className="h-auto justify-start gap-3 rounded-xl p-4 text-left whitespace-normal"
            >
              <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
                <Icon aria-hidden className="size-4" />
              </span>
              <span className="min-w-0">
                <span className="block font-medium">{copy.choices[key].title}</span>
                <span className="block text-xs font-normal text-muted-foreground">
                  {copy.choices[key].line}
                </span>
              </span>
            </Button>
          ))}
        </div>
      </section>
    </div>
  )
}
