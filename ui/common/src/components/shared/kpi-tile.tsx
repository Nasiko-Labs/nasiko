/** One figure on a shadcn `Card`: label (plus an optional aside such as a `Delta`), then the value. */
import type { ComponentProps, ReactNode } from 'react'
import { Card } from '@/components/ui/card'
import { cn } from '@/lib/utils'

export function KpiTile({
  label,
  aside,
  value,
  compact,
  className,
  children,
  ...rest
}: Omit<ComponentProps<typeof Card>, 'children'> & {
  label: ReactNode
  aside?: ReactNode
  value: ReactNode
  compact?: boolean
  className?: string
  /** Anything under the value (a meter, a footnote). */
  children?: ReactNode
}) {
  return (
    <Card className={cn('gap-1 p-3', className)} {...rest}>
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs text-muted-foreground">{label}</span>
        {aside}
      </div>
      <div className={cn('font-semibold tabular-nums', compact ? 'text-lg' : 'text-2xl')}>
        {value}
      </div>
      {children}
    </Card>
  )
}
