/**
 * A collapsible section (WAI-ARIA disclosure) on shadcn `Collapsible`. Content mounts only when open,
 * so charts never measure a hidden 0×0 box. The height animation is tw-animate-css's
 * `collapsible-down/up` (200 ms ease-out, DESIGN.md `disclosure`), off under reduced motion.
 */
import { ChevronRight } from 'lucide-react'
import type { ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { cn } from '@/lib/utils'

export function Disclosure({
  id,
  title,
  hint,
  open,
  onToggle,
  children,
}: {
  id: string
  title: string
  /** One short line shown next to the title while collapsed. */
  hint?: ReactNode
  open: boolean
  onToggle: () => void
  children: ReactNode
}) {
  const titleId = `disclosure-${id}-title`
  return (
    <Collapsible
      open={open}
      onOpenChange={onToggle}
      className="border-t border-border first:border-t-0"
    >
      {/* A plain disclosure button: the panel inside keeps its own heading (no duplicate names). */}
      <CollapsibleTrigger asChild>
        <Button
          id={titleId}
          variant="ghost"
          className="min-h-11 w-full justify-start gap-2 rounded-none px-0 py-2 text-left text-base hover:bg-transparent"
        >
          <ChevronRight
            className={cn(
              'size-4 shrink-0 text-muted-foreground transition-transform',
              open && 'rotate-90',
            )}
            aria-hidden
          />
          <span>{title}</span>
          {!open && hint ? (
            <span className="truncate text-sm font-normal text-muted-foreground">{hint}</span>
          ) : null}
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent
        role="region"
        aria-labelledby={titleId}
        // The negative margin widens the clip box so a field's focus ring (2 px + 2 px offset) isn't cut at the sides.
        className="-mx-1.5 overflow-hidden px-1.5 data-[state=closed]:animate-collapsible-up data-[state=open]:animate-collapsible-down motion-reduce:animate-none"
      >
        <div className="flex flex-col gap-4 pb-4">{children}</div>
      </CollapsibleContent>
    </Collapsible>
  )
}
