/**
 * Type-to-confirm field for destructive dialogs: the phrase to type sits on its own line as a
 * selectable chip, then an empty input below it. The phrase is part of the label, so screen
 * readers announce "Type … to confirm" and tests can find the input by label.
 *
 * No placeholder on purpose: a grey copy of the phrase inside the input reads as already filled in.
 */
import { useId } from 'react'
import { Input } from '../primitives/input'
import { cn } from '../lib/utils'

export function TypeToConfirm({
  phrase,
  value,
  onChange,
  lead = 'Type',
  tail = 'to confirm',
  mono = true,
  className,
}: {
  /** The exact text the user must type. */
  phrase: string
  value: string
  onChange: (value: string) => void
  lead?: string
  tail?: string
  /** Show the phrase and the input in the monospace face (identifiers); off for plain words. */
  mono?: boolean
  className?: string
}) {
  const id = useId()
  return (
    <div className={cn('flex flex-col gap-2', className)}>
      <label htmlFor={id} className="flex flex-col gap-1.5 text-sm text-muted-foreground">
        <span>
          {lead} <span className="sr-only">{phrase}</span> {tail}:
        </span>
        <code
          aria-hidden
          className={cn(
            'block w-fit max-w-full rounded-md border bg-muted px-2 py-1 text-sm break-all text-foreground select-all',
            mono ? 'font-mono' : 'font-sans font-medium',
          )}
        >
          {phrase}
        </code>
      </label>
      <Input
        id={id}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        autoComplete="off"
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        className={cn(mono && 'font-mono')}
      />
    </div>
  )
}
