/**
 * The guide's last step (user request 2026-11-05, in place of Spend less and Ready): the two CLI commands that bring a
 * developer's coding harnesses into Coding harnesses, install and sign in (user review 2026-11-05: the rest dropped). The installer is the CLI guide's own;
 * the sign-in asks for SSO only where this edition signs in through another method (the `loginMethods` slot).
 */
import { Bot, MessageSquare, SquareTerminal } from 'lucide-react'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { CopyButton } from '@nasiko/ui/composites/copy-button'
import { Button } from '@nasiko/ui/primitives/button'
import { copy as cliCopy } from '@nasiko/features/cli/copy'
import { copy as guideCopy } from './copy'
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
  const sso = !!useSlots().loginMethods
  const steps = [
    { label: copy.steps.install, command: cliCopy.install.unixCommand },
    sso
      ? { label: copy.steps.signInSso, command: 'nasiko auth login --sso' }
      : { label: copy.steps.signIn, command: 'nasiko auth login' },
  ]
  return (
    <div className="flex flex-col gap-6">
      <StepHeading title={copy.title} intro={copy.intro} />
      <ol className="flex flex-col gap-4">
        {steps.map((s, i) => (
          <li key={s.label} className="flex gap-3">
            <span
              aria-hidden
              className="flex size-6 shrink-0 items-center justify-center rounded-md border font-mono text-xs"
            >
              {i + 1}
            </span>
            <div className="flex min-w-0 flex-1 flex-col gap-1.5">
              <span className="text-sm">{s.label}</span>
              <div className="flex items-center gap-2 rounded-md border bg-muted/50 py-1 pr-1 pl-3">
                <span aria-hidden className="font-mono text-xs text-muted-foreground select-none">
                  $
                </span>
                <code className="min-w-0 flex-1 font-mono text-xs break-all whitespace-pre-wrap">
                  {s.command}
                </code>
                <CopyButton text={s.command} label={copy.copy(s.label)} />
              </div>
            </div>
          </li>
        ))}
      </ol>
      <p className="text-sm text-muted-foreground">{copy.after}</p>
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
