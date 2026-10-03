/**
 * The one data table (plan §2.2): shadcn `Table` + TanStack Table 9, virtualised by TanStack Virtual
 * past `VIRTUAL_AFTER` rows so pages never decide it. Adapted from the React migration's
 * `components/data-table/data-table.tsx`; states (loading, error, empty) stay with the caller's
 * `Panel`, and CSV stays with the page (TokenOps' `csv.ts` is formula-safe and tested).
 *
 * Sorting: pass `sorting` to mark the sorted column (`aria-sort`). Add `onSortingChange` to make
 * sortable headers buttons. Rows are sorted here only with `clientSort`; otherwise `data` arrives in
 * order (URL-driven sorts keep their tested pure sort functions).
 *
 * `header` and `cell` functions are called, never mounted: TanStack's FlexRender mounts a function as a
 * component, so columns built during render remounted every cell and lost focus. The flip side is that
 * a cell function can't call hooks; render a component from it instead.
 */
import {
  createSortedRowModel,
  rowSortingFeature,
  sortFn_alphanumeric,
  sortFn_basic,
  sortFn_datetime,
  tableFeatures,
  useTable,
  type ColumnDef,
  type RowData,
  type SortingState,
} from '@tanstack/react-table'
import { useVirtualizer } from '@tanstack/react-virtual'
import { ArrowDown, ArrowUp, ArrowUpDown } from 'lucide-react'
import { useRef, type ComponentProps, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { cn } from '@/lib/utils'

const features = tableFeatures({
  rowSortingFeature,
  sortedRowModel: createSortedRowModel(),
  sortFns: { alphanumeric: sortFn_alphanumeric, datetime: sortFn_datetime, basic: sortFn_basic },
})
type Features = typeof features

declare module '@tanstack/react-table' {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars -- a merged declaration must repeat TanStack's type parameter names
  interface ColumnMeta<TFeatures, TData, TValue> {
    /** Right-aligned, tabular figures. */
    numeric?: boolean
    /** This column's cells are row headers (`th scope="row"`), which name the row for screen readers. */
    rowHeader?: boolean
    headerClassName?: string
    cellClassName?: string
  }
}

// Columns of mixed value types in one array (TanStack's own pattern for helper.columns).
export type DataTableColumn<T extends RowData> = ColumnDef<Features, T, unknown>

/** Rows past this render through the virtualizer. */
const VIRTUAL_AFTER = 100
const ROW_HEIGHT = 37

export interface DataTableProps<T extends RowData> {
  columns: DataTableColumn<T>[]
  data: T[]
  getRowId: (row: T) => string
  /** Accessible name for the table. */
  label: string
  /** A visible caption instead of (or as well as) the label. */
  caption?: ReactNode
  sorting?: SortingState
  onSortingChange?: (next: SortingState) => void
  /** Sort rows here with the column's sortFn; off by default (the page sorts). */
  clientSort?: boolean
  /** Per-row props: a ref, a highlight class, data attributes. */
  rowProps?: (row: T) => ComponentProps<'tr'>
  className?: string
}

/** A column's `header` or `cell`: a function is called with its context; anything else renders as is. */
function renderSlot<C>(slot: unknown, ctx: C): ReactNode {
  return typeof slot === 'function' ? (slot as (c: C) => ReactNode)(ctx) : (slot as ReactNode)
}

export function DataTable<T extends RowData>({
  columns,
  data,
  getRowId,
  label,
  caption,
  sorting,
  onSortingChange,
  clientSort,
  rowProps,
  className,
}: DataTableProps<T>) {
  'use no memo'
  const table = useTable({
    features,
    columns,
    data,
    getRowId: (row: T) => getRowId(row),
    state: sorting ? { sorting } : undefined,
    onSortingChange: onSortingChange
      ? (u) => onSortingChange(typeof u === 'function' ? u(sorting ?? []) : u)
      : undefined,
    manualSorting: !clientSort,
    enableSorting: !!onSortingChange,
  })
  const rows = table.getRowModel().rows
  const scrollRef = useRef<HTMLDivElement>(null)
  const virtual = rows.length > VIRTUAL_AFTER
  // eslint-disable-next-line react-hooks/incompatible-library -- useVirtualizer returns mutable instances the Compiler can't memoise; DataTable is 'use no memo'
  const virtualizer = useVirtualizer({
    count: virtual ? rows.length : 0,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (i) => rows[i]?.id ?? i,
    overscan: 10,
  })
  const items = virtualizer.getVirtualItems()
  const padTop = virtual ? (items[0]?.start ?? 0) : 0
  const padBottom = virtual ? virtualizer.getTotalSize() - (items.at(-1)?.end ?? 0) : 0
  // [row, position] pairs: positions feed aria-rowindex, which only a virtualised table needs.
  const visible = virtual
    ? items.flatMap((i) => {
        const row = rows[i.index]
        return row ? [[row, i.index] as const] : []
      })
    : rows.map((r, i) => [r, i] as const)
  const leaves = table.getAllLeafColumns()
  const sortedBy = new Map(
    (sorting ?? []).map((s) => [s.id, s.desc ? 'descending' : 'ascending'] as const),
  )

  return (
    // Virtualised, this div is the scroll box, so the Table's own overflow container is switched off:
    // otherwise it becomes the sticky header's scroll parent and the header never sticks.
    <div
      ref={scrollRef}
      className={cn(
        'min-w-0',
        virtual && 'max-h-[70vh] overflow-auto [&>[data-slot=table-container]]:overflow-visible',
        className,
      )}
    >
      <Table
        aria-label={caption ? undefined : label}
        aria-rowcount={virtual ? rows.length + 1 : undefined}
      >
        {caption ? <TableCaption>{caption}</TableCaption> : null}
        <TableHeader className={cn(virtual && 'sticky top-0 z-10 bg-card')}>
          {table.getHeaderGroups().map((group) => (
            <TableRow key={group.id} className="hover:bg-transparent">
              {group.headers.map((header) => {
                const col = header.column
                const meta = col.columnDef.meta
                const dir = sortedBy.get(col.id)
                const sortable = col.getCanSort()
                return (
                  <TableHead
                    key={header.id}
                    scope="col"
                    aria-sort={dir ?? (sortable ? 'none' : undefined)}
                    className={cn(
                      'text-xs font-medium text-muted-foreground',
                      meta?.numeric && 'text-right',
                      meta?.headerClassName,
                    )}
                  >
                    {header.isPlaceholder ? null : sortable ? (
                      <Button
                        variant="ghost"
                        size="sm"
                        className={cn('-mx-2 h-7 text-xs', meta?.numeric && 'ml-auto flex')}
                        onClick={col.getToggleSortingHandler()}
                      >
                        {renderSlot(col.columnDef.header, header.getContext())}
                        {dir === 'ascending' ? (
                          <ArrowUp aria-hidden />
                        ) : dir === 'descending' ? (
                          <ArrowDown aria-hidden />
                        ) : (
                          <ArrowUpDown className="opacity-40" aria-hidden />
                        )}
                      </Button>
                    ) : (
                      renderSlot(col.columnDef.header, header.getContext())
                    )}
                  </TableHead>
                )
              })}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody>
          {padTop > 0 ? (
            <tr aria-hidden>
              <td style={{ height: padTop }} colSpan={leaves.length} />
            </tr>
          ) : null}
          {visible.map(([row, pos]) => {
            const extra = rowProps?.(row.original)
            return (
              <TableRow key={row.id} aria-rowindex={virtual ? pos + 2 : undefined} {...extra}>
                {row.getAllCells().map((cell) => {
                  const meta = cell.column.columnDef.meta
                  const className = cn(
                    meta?.numeric && 'text-right tabular-nums',
                    meta?.cellClassName,
                  )
                  const content = renderSlot(cell.column.columnDef.cell, cell.getContext())
                  return meta?.rowHeader ? (
                    <TableHead
                      key={cell.id}
                      scope="row"
                      className={cn('font-medium text-foreground', className)}
                    >
                      {content}
                    </TableHead>
                  ) : (
                    <TableCell key={cell.id} className={className}>
                      {content}
                    </TableCell>
                  )
                })}
              </TableRow>
            )
          })}
          {padBottom > 0 ? (
            <tr aria-hidden>
              <td style={{ height: padBottom }} colSpan={leaves.length} />
            </tr>
          ) : null}
        </TableBody>
      </Table>
    </div>
  )
}
