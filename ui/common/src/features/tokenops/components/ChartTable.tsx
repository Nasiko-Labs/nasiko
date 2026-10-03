/**
 * The chart panels' Table views (the keyboard and screen-reader path) on shadcn `Table`: compact,
 * sticky head, a row header per row. Not `DataTable`: that has no row-header cells and sticks its
 * head only when virtualised.
 */
import type { ReactNode } from 'react'
import { Table } from '@/components/ui/table'
import { cn } from '@/lib/utils'

/** `TableHeader` */
export const STICKY_HEAD = 'sticky top-0 z-10 bg-card'
/** `TableHead scope="col"` */
export const HEAD = 'h-8 font-medium text-muted-foreground'
/** `TableHead scope="row"` */
export const ROW_HEAD = 'h-auto py-1 font-normal'
/** `TableCell` */
export const CELL = 'py-1'
/** Right-aligned figures (`TableCell` or `TableHead`). */
export const NUM = 'text-right tabular-nums'

export function ChartTable({ className, children }: { className?: string; children: ReactNode }) {
  // shadcn's table container scrolls on its own and would become the sticky head's scroller, so
  // it stays visible and this box scrolls.
  return (
    <div
      className={cn(
        'max-h-72 overflow-auto [&>[data-slot=table-container]]:overflow-visible',
        className,
      )}
    >
      <Table className="text-xs">{children}</Table>
    </div>
  )
}
