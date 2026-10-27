/** Small shared pieces for the Harnesses page: harness label, cost figure, tooltip term, the import's state icon. */
import { CircleCheck, CircleDashed, CircleDot, CircleX, Loader2, Minus } from 'lucide-react'
import type { ReactNode } from 'react'
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { fmtMoney } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import { harnessStyle, type CostView } from '../rollup'

/** Colour chip + text label: colour is never the only cue (G7). */
export function HarnessLabel({ id, className }: { id: string; className?: string }) {
  const s = harnessStyle(id)
  return (
    <span className={`inline-flex items-center gap-1.5 ${className ?? ''}`}>
      <span
        aria-hidden
        className="inline-block size-2.5 shrink-0 rounded-xs"
        style={{ background: s.color, borderTop: `2px solid ${s.edge}` }}
      />
      <span>
        {s.name}
        {!s.known ? <span className="text-muted-foreground"> ({id})</span> : null}
      </span>
    </span>
  )
}

/** A term with an explanatory tooltip (keyboard-focusable). */
export function Term({ children, tip }: { children: ReactNode; tip: string }) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span
            // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focusable only so keyboard users can open the term's tooltip; it has no action, so no button role
            tabIndex={0}
            className="cursor-help underline decoration-dotted underline-offset-2"
          />
        }
      >
        {children}
      </TooltipTrigger>
      <TooltipContent className="max-w-xs">{tip}</TooltipContent>
    </Tooltip>
  )
}

/** Est. cost, or the word "unpriced" (never "—", which means undefined; G13). */
export function CostFigure({ view }: { view: CostView }) {
  if (view.kind === 'unpriced') return <Term tip={copy.unpricedTip}>{copy.unpriced}</Term>
  return <span className="tabular-nums">{fmtMoney(view.value)}</span>
}

/** A money ratio, or "—" when it is undefined (zero denominator, unpriced, or no source). */
export function Ratio({ value }: { value: number | null }) {
  if (value === null) return <span className="text-muted-foreground">—</span>
  return <span className="tabular-nums">{fmtMoney(value)}</span>
}

export type State = 'done' | 'current' | 'pending' | 'failed'

/**
 * The stepper's icon per state (deploy/components/StageList.tsx), plus `active` (a still mark for what the one spinning
 * step is working on, so a card never spins in three places) and a dash for an agent that isn't taking part.
 */
export function StateIcon({ state }: { state: State | 'active' | 'none' }) {
  const cls = 'size-4 shrink-0'
  switch (state) {
    case 'done':
      return <CircleCheck aria-hidden className={cn(cls, 'text-success')} />
    case 'current':
      return <Loader2 aria-hidden className={cn(cls, 'text-info motion-safe:animate-spin')} />
    case 'failed':
      return <CircleX aria-hidden className={cn(cls, 'text-destructive')} />
    case 'active':
      return <CircleDot aria-hidden className={cn(cls, 'text-info')} />
    case 'none':
      return <Minus aria-hidden className={cn(cls, 'text-muted-foreground')} />
    default:
      return <CircleDashed aria-hidden className={cn(cls, 'text-muted-foreground')} />
  }
}
