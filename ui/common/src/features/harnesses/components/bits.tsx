/** Small shared pieces for the Harnesses page: harness label, cost figure, tooltip term. */
import type { ReactNode } from 'react'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { fmtMoney } from '@/lib/format'
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
      <TooltipTrigger asChild>
        <span
          // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focusable only so keyboard users can open the term's tooltip; it has no action, so no button role
          tabIndex={0}
          className="cursor-help underline decoration-dotted underline-offset-2"
        >
          {children}
        </span>
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
