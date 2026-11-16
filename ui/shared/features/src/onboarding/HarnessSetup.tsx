/**
 * The CLI commands that bring a developer's coding harnesses in: install, then connect to this server, and what
 * happens next. One block for the setup guide's last step (HarnessStep.tsx) and Coding harnesses' empty state
 * (harnesses/components/AgentsOnboardingCard.tsx), so the two never drift apart.
 */
import { CopyButton } from '@nasiko/ui/composites/copy-button'
import { connectCommand } from '@nasiko/features/cli/commands'
import { copy as cliCopy } from '@nasiko/features/cli/copy'
import { copy as guideCopy } from './copy'

const copy = guideCopy.harness

/** The block's title and intro as a card heading; `id` names it for the card's `aria-labelledby`. */
export function HarnessSetupHeading({ id }: { id: string }) {
  return (
    <div className="flex flex-col gap-1">
      <h2 id={id} className="text-base font-semibold">
        {copy.title}
      </h2>
      <p className="max-w-[70ch] text-sm text-muted-foreground">{copy.intro}</p>
    </div>
  )
}

export function HarnessSetupSteps() {
  const steps = [
    { label: copy.steps.install, command: cliCopy.install.unixCommand },
    { label: copy.steps.connect, command: connectCommand() },
  ]
  return (
    <>
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
    </>
  )
}
