/**
 * Settings rows: one hairline card per group of settings, a row per setting with its label and hint on the left and
 * the control on the right (stacked in a narrow card, or when the control needs the full width), rows divided by
 * hairlines. A form's Save sits in the card's `footer`. The EE sections use them too, so every Settings page reads
 * the same.
 */
import type { ReactNode } from 'react'
import { FieldError, FieldLabel } from '@/components/ui/field'
import { cn } from '@/lib/utils'

/** The card: the query target for its rows' two-column switch. */
export function SettingRows({
  children,
  footer,
  className,
}: {
  children: ReactNode
  /** The card's action bar (a form's Save), on a muted strip under the rows. */
  footer?: ReactNode
  className?: string
}) {
  return (
    <div
      className={cn(
        '@container overflow-hidden rounded-lg border border-border bg-card text-card-foreground',
        className,
      )}
    >
      {children}
      {footer ? (
        <div className="flex flex-wrap items-center justify-end gap-3 border-t border-border bg-muted/40 px-5 py-3">
          {footer}
        </div>
      ) : null}
    </div>
  )
}

export function SettingRow({
  htmlFor,
  label,
  hint,
  hintId,
  error,
  stacked = false,
  children,
}: {
  /** The control's id; omit for a row with no control (a note). */
  htmlFor?: string
  label: string
  hint?: ReactNode
  /** Lets the control point `aria-describedby` at the hint. */
  hintId?: string
  error?: string
  /** The control goes under the label at full width (choice grids). */
  stacked?: boolean
  children?: ReactNode
}) {
  return (
    <div
      className={cn(
        // Hairlines between rows; a row last in its parent (the card, or a section wrapper) has none.
        'grid gap-3 border-b border-border px-5 py-5 last:border-b-0',
        !stacked && '@[640px]:grid-cols-[minmax(0,1fr)_minmax(0,20rem)] @[640px]:gap-10',
        stacked && 'gap-4',
      )}
    >
      <div className="min-w-0">
        {htmlFor ? (
          <FieldLabel htmlFor={htmlFor} className="text-sm font-medium">
            {label}
          </FieldLabel>
        ) : (
          <p className="text-sm font-medium">{label}</p>
        )}
        {hint ? (
          <div id={hintId} className="mt-1 max-w-prose text-sm text-muted-foreground">
            {hint}
          </div>
        ) : null}
      </div>
      {children ? (
        <div className={cn('min-w-0', !stacked && 'self-center')}>
          {children}
          {error ? <FieldError className="mt-1.5">{error}</FieldError> : null}
        </div>
      ) : null}
    </div>
  )
}

/** A sub-heading between cards in a section (SCIM provisioning, Organization rules). */
export function SettingHeading({ children }: { children: ReactNode }) {
  return <h2 className="pt-6 pb-3 text-base font-semibold">{children}</h2>
}
