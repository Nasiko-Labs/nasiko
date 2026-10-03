/**
 * Page- and section-level states on shadcn `Alert` and `Empty` (plan §8 Phase 1): what happened,
 * then what to do about it. Never a bare "No items found". Feature code maps its errors onto these
 * (observability `ErrorState`, agents `ErrorNote`, TokenOps `ServerDown`); the look lives here once.
 */
import { AlertTriangle, Info, ServerOff, type LucideIcon } from 'lucide-react'
import type { ReactNode } from 'react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from '@/components/ui/empty'
import { cn } from '@/lib/utils'

export type StateTone = 'info' | 'warning' | 'error'

const ICON = { info: Info, warning: AlertTriangle, error: ServerOff } as const
// On the Alert, not the icon: the primitive's `[&>svg]:text-current` outranks a class on the svg itself.
const COLOR = {
  info: '[&>svg]:text-info',
  warning: '[&>svg]:text-warning',
  error: '[&>svg]:text-destructive',
} as const

/** Info is announced politely (`status`); warning and error interrupt (`alert`). */
export function StateCard({
  tone = 'info',
  title,
  children,
  fix,
  action,
  className,
}: {
  tone?: StateTone
  title: ReactNode
  children?: ReactNode
  /** The next step, in plain words or a command. */
  fix?: ReactNode
  action?: ReactNode
  className?: string
}) {
  const Icon = ICON[tone]
  return (
    <Alert
      role={tone === 'info' ? 'status' : 'alert'}
      className={cn('gap-y-2 p-5', COLOR[tone], className)}
    >
      <Icon aria-hidden />
      <AlertTitle className="line-clamp-none">{title}</AlertTitle>
      {children || fix || action ? (
        <AlertDescription className="max-w-prose gap-2">
          {children ? <div>{children}</div> : null}
          {fix ? <div>{fix}</div> : null}
          {action}
        </AlertDescription>
      ) : null}
    </Alert>
  )
}

/**
 * Nothing to show yet: the app's one empty screen, a dashed `Empty` with an optional icon, a title, one line on why
 * or what next, and an optional action. Pages pass `icon` (their nav icon, or what is missing) and a next step; a
 * section inside a panel uses `PanelEmpty` (no icon, compact). Filtered to nothing: say so and offer to clear.
 */
export function EmptyState({
  icon: Icon,
  title,
  children,
  action,
  className,
}: {
  icon?: LucideIcon
  title: ReactNode
  children?: ReactNode
  action?: ReactNode
  className?: string
}) {
  return (
    <Empty
      className={cn(
        'gap-2 border border-border px-4 py-8 md:p-8',
        Icon && 'gap-4 py-12 md:py-14',
        className,
      )}
    >
      <EmptyHeader className="gap-1">
        {Icon ? (
          <EmptyMedia variant="icon" className="mb-2 text-muted-foreground">
            <Icon aria-hidden className="size-5" />
          </EmptyMedia>
        ) : null}
        <EmptyTitle className={Icon ? 'text-base' : 'text-sm'}>{title}</EmptyTitle>
        {children ? (
          <EmptyDescription className={Icon ? 'text-sm' : 'text-xs'}>{children}</EmptyDescription>
        ) : null}
      </EmptyHeader>
      {action ? (
        <EmptyContent className="flex-row flex-wrap justify-center gap-2">{action}</EmptyContent>
      ) : null}
    </Empty>
  )
}
