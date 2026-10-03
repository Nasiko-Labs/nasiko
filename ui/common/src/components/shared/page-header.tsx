/** The one page heading (plan §8 Phase 1): optional breadcrumb, `h1`, one line of description, actions. */
import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'

export function PageHeader({
  title,
  description,
  actions,
  breadcrumb,
  className,
}: {
  title: ReactNode
  description?: ReactNode
  actions?: ReactNode
  /** A shadcn `Breadcrumb`, shown above the title. */
  breadcrumb?: ReactNode
  className?: string
}) {
  return (
    <header className={cn('flex flex-col gap-2', className)}>
      {breadcrumb}
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h1 className="text-xl font-semibold">{title}</h1>
          {description ? <p className="text-sm text-muted-foreground">{description}</p> : null}
        </div>
        {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
      </div>
    </header>
  )
}
