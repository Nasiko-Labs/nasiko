/**
 * Type-to-confirm field for destructive dialogs: "Type <phrase> to confirm" with the phrase as an
 * inline code chip, then an empty input. The sentence names the input through aria-labelledby, so
 * the phrase is read once, as part of it. A <p>, not a <label>: clicking a label focuses its input,
 * which would undo selecting the phrase to copy it.
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
    <div className={cn('grid gap-2', className)}>
      <p id={`${id}-label`} className="text-sm">
        {lead}{' '}
        <code
          className={cn(
            'rounded bg-muted box-decoration-clone px-1.5 py-0.5 font-medium wrap-anywhere select-all',
            mono ? 'font-mono text-[0.85em]' : 'font-sans',
          )}
        >
          {phrase}
        </code>{' '}
        {tail}
      </p>
      <Input
        aria-labelledby={`${id}-label`}
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
