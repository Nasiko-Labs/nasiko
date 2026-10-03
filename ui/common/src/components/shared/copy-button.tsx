/** Copy controls on shadcn `Button` and `DropdownMenuItem`, over `useCopy` (plan §6.6 of feat-agents). */
import { Check, Copy } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { DropdownMenuItem } from '@/components/ui/dropdown-menu'
import { useCopy } from '@/lib/useCopy'
import { cn } from '@/lib/utils'

const COPIED = 'Copied'
const COPY_FAILED = 'Copy failed — select and copy manually.'

/** Copies `text`; on failure the text stays visible and selectable with a note. */
export function CopyButton({
  text,
  label,
  showText = false,
  className,
}: {
  text: string
  label: string
  showText?: boolean
  className?: string
}) {
  const [state, onCopy] = useCopy(text)
  return (
    <span className={cn('inline-flex flex-wrap items-center gap-1.5', className)}>
      {showText || state === 'failed' ? (
        <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs select-all">{text}</code>
      ) : null}
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        className="pointer-coarse:size-11"
        onClick={() => void onCopy()}
        aria-label={label}
      >
        {state === 'copied' ? (
          <Check className="size-3.5" aria-hidden />
        ) : (
          <Copy className="size-3.5" aria-hidden />
        )}
        <span className="sr-only">{label}</span>
      </Button>
      {state === 'copied' ? (
        <span className="text-xs text-muted-foreground" role="status">
          {COPIED}
        </span>
      ) : null}
      {state === 'failed' ? (
        <span className="text-xs text-warning" role="alert">
          {COPY_FAILED}
        </span>
      ) : null}
    </span>
  )
}

/**
 * A copy action inside a DropdownMenu: a real menu item, so arrow keys reach it. The menu
 * stays open to show the result; on failure the text is shown selectable.
 */
export function CopyMenuItem({ text, label }: { text: string; label?: string }) {
  const [state, run] = useCopy(text)
  return (
    <DropdownMenuItem
      className="flex-col items-start gap-1"
      onSelect={(e) => {
        e.preventDefault()
        void run()
      }}
    >
      <span className="flex w-full items-center justify-between gap-2">
        <code className="truncate font-mono text-xs" title={text}>
          {label ?? text}
        </code>
        {state === 'copied' ? (
          <span
            className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground"
            role="status"
          >
            <Check className="size-3.5" aria-hidden />
            {COPIED}
          </span>
        ) : (
          <Copy className="size-3.5 shrink-0" aria-hidden />
        )}
      </span>
      {state === 'failed' ? (
        <span className="text-xs text-warning" role="alert">
          {COPY_FAILED} <code className="font-mono select-all">{text}</code>
        </span>
      ) : null}
    </DropdownMenuItem>
  )
}
