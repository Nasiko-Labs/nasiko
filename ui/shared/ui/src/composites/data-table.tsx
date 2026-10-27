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
 * Groups: pass `groups` to put a full-width header row before each run of rows sharing a key (the caller orders the
 * rows); its button collapses the run. Header rows count as rows for virtualisation and `aria-rowcount`.
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
  type Row,
  type RowData,
  type SortingState,
} from '@tanstack/react-table'
import { useVirtualizer } from '@tanstack/react-virtual'
import { ArrowDown, ArrowUp, ArrowUpDown, ChevronDown, ChevronRight } from 'lucide-react'
import { useRef, type ComponentProps, type ReactNode } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { cn } from '@nasiko/ui/lib/utils'

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
/** The first estimate per row; each rendered row is then measured (`measureElement`). */
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
  groups?: DataTableGroups<T>
  className?: string
}

export interface DataTableGroups<T> {
  /** The group a row belongs to; consecutive rows with the same key form one group. */
  of: (row: T) => string
  /** The header row's content (a name, a count, a subtotal): it is also the collapse button's name. */
  header: (key: string) => ReactNode
  collapsed: ReadonlySet<string>
  onToggle: (key: string) => void
}

type Item<T extends RowData> =
  { kind: 'group'; key: string } | { kind: 'row'; row: Row<Features, T> }

function itemsOf<T extends RowData>(
  rows: Row<Features, T>[],
  groups: DataTableGroups<T> | undefined,
): Item<T>[] {
  if (!groups) return rows.map((row) => ({ kind: 'row', row }))
  const out: Item<T>[] = []
  let last: string | undefined
  for (const row of rows) {
    const key = groups.of(row.original)
    if (key !== last) out.push({ kind: 'group', key })
    last = key
    if (!groups.collapsed.has(key)) out.push({ kind: 'row', row })
  }
  return out
}

const itemKey = <T extends RowData>(item: Item<T> | undefined) =>
  item ? (item.kind === 'group' ? `group:${item.key}` : item.row.id) : undefined

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
  groups,
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
  const items = itemsOf(table.getRowModel().rows, groups)
  const scrollRef = useRef<HTMLDivElement>(null)
  const virtual = items.length > VIRTUAL_AFTER
  // eslint-disable-next-line react-hooks/incompatible-library -- useVirtualizer returns mutable instances the Compiler can't memoise; DataTable is 'use no memo'
  const virtualizer = useVirtualizer({
    count: virtual ? items.length : 0,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (i) => itemKey(items[i]) ?? i,
    overscan: 10,
  })
  const windowed = virtualizer.getVirtualItems()
  const padTop = virtual ? (windowed[0]?.start ?? 0) : 0
  const padBottom = virtual ? virtualizer.getTotalSize() - (windowed.at(-1)?.end ?? 0) : 0
  // [item, position] pairs: positions feed aria-rowindex, which only a virtualised table needs.
  const visible = virtual
    ? windowed.flatMap((i) => {
        const item = items[i.index]
        return item ? [[item, i.index] as const] : []
      })
    : items.map((r, i) => [r, i] as const)
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
        aria-rowcount={virtual ? items.length + 1 : undefined}
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
          {visible.map(([item, pos]) => {
            if (item.kind === 'group')
              return groups ? (
                <TableRow
                  key={`group:${item.key}`}
                  aria-rowindex={virtual ? pos + 2 : undefined}
                  data-index={virtual ? pos : undefined}
                  ref={virtual ? virtualizer.measureElement : undefined}
                  data-group={item.key}
                  className="bg-muted/40 hover:bg-muted/40"
                >
                  <TableCell colSpan={leaves.length} className="py-1">
                    <Button
                      variant="ghost"
                      size="sm"
                      className="-mx-2 h-auto min-h-7 gap-2 text-xs font-medium pointer-coarse:min-h-11"
                      aria-expanded={!groups.collapsed.has(item.key)}
                      onClick={() => groups.onToggle(item.key)}
                    >
                      {groups.collapsed.has(item.key) ? (
                        <ChevronRight aria-hidden />
                      ) : (
                        <ChevronDown aria-hidden />
                      )}
                      {groups.header(item.key)}
                    </Button>
                  </TableCell>
                </TableRow>
              ) : null
            const row = item.row
            const extra = rowProps?.(row.original)
            return (
              <TableRow
                key={row.id}
                aria-rowindex={virtual ? pos + 2 : undefined}
                // Measured, not assumed: rows taller than ROW_HEIGHT (two-line cells) would drift the window. A caller's
                // own row ref keeps the estimate.
                data-index={virtual ? pos : undefined}
                ref={virtual && !extra?.ref ? virtualizer.measureElement : undefined}
                {...extra}
              >
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
