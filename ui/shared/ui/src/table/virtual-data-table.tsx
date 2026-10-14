// Ported from nasiko-ui-lab@1b99bdda common/src/components/shared/data-table.tsx (as `VirtualDataTable`, beside the
// existing server-paginated `DataTable`). Local edits — TanStack Table 9 -> 8.21.3:
// - `tableFeatures({ rowSortingFeature, sortedRowModel, sortFns })` + `useTable({ features, … })` ->
//   `useReactTable({ getCoreRowModel: getCoreRowModel(), getSortedRowModel: getSortedRowModel(), … })` (v8 ships
//   alphanumeric/datetime/basic sorting built in; the sorted model is only consulted when `clientSort` is on, since
//   `manualSorting` is `!clientSort` as in the lab);
// - `ColumnDef<Features, T, unknown>` -> `ColumnDef<T>` (`VirtualDataTableColumn<T>`; v8's TValue defaults to unknown),
//   `RowData` kept; `renderSlot` loses its single-use type parameter (lint);
// - the `ColumnMeta` augmentation takes v8's two type parameters (`TData extends RowData, TValue`);
// - `state: sorting ? { sorting } : undefined` -> `state: { sorting: sorting ?? [] }`: v8's `state` field is not
//   optional-undefined under exactOptionalPropertyTypes;
// - `onSortingChange` is only passed when given (v8 types it `OnChangeFn | undefined`, same semantics);
// - the lab's eslint-disable moves from `useVirtualizer` to `useReactTable` (what this repo's react-hooks plugin flags);
// - imports mapped to ../primitives/*, ../lib/cn; lucide icons as in the lab.
// Rendering (sticky header, padding rows, aria-rowindex/rowcount, sort buttons, row headers) is the lab's.
// - the virtualised scroll box is a focusable `role="region"` named by the caption (or label), with the lab's focus
//   ring, as the Table primitive's own overflow container is (axe `scrollable-region-focusable`).
"use client";

/**
 * The one data table (plan §2.2): shadcn `Table` + TanStack Table, virtualised by TanStack Virtual
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
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type RowData,
  type SortingState,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, ArrowUpDown } from "lucide-react";
import { useRef, type ComponentProps, type ReactNode } from "react";
import { cn } from "../lib/cn";
import { Button } from "../primitives/button";
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../primitives/table";

declare module "@tanstack/react-table" {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars -- a merged declaration must repeat TanStack's type parameter names
  interface ColumnMeta<TData extends RowData, TValue> {
    /** Right-aligned, tabular figures. */
    numeric?: boolean;
    /** This column's cells are row headers (`th scope="row"`), which name the row for screen readers. */
    rowHeader?: boolean;
    headerClassName?: string;
    cellClassName?: string;
  }
}

// Columns of mixed value types in one array (TanStack's own pattern for helper.columns).
export type VirtualDataTableColumn<T extends RowData> = ColumnDef<T>;

/** Rows past this render through the virtualizer. */
const VIRTUAL_AFTER = 100;
const ROW_HEIGHT = 37;

export interface VirtualDataTableProps<T extends RowData> {
  columns: VirtualDataTableColumn<T>[];
  data: T[];
  getRowId: (row: T) => string;
  /** Accessible name for the table. */
  label: string;
  /** A visible caption instead of (or as well as) the label. */
  caption?: ReactNode;
  sorting?: SortingState | undefined;
  onSortingChange?: ((next: SortingState) => void) | undefined;
  /** Sort rows here with the column's sortFn; off by default (the page sorts). */
  clientSort?: boolean | undefined;
  /** Per-row props: a ref, a highlight class, data attributes. */
  rowProps?: ((row: T) => ComponentProps<"tr">) | undefined;
  className?: string | undefined;
}

/** A column's `header` or `cell`: a function is called with its context; anything else renders as is. */
function renderSlot(slot: unknown, ctx: unknown): ReactNode {
  return typeof slot === "function"
    ? (slot as (c: unknown) => ReactNode)(ctx)
    : (slot as ReactNode);
}

export function VirtualDataTable<T extends RowData>({
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
}: VirtualDataTableProps<T>) {
  "use no memo";
  // eslint-disable-next-line react-hooks/incompatible-library -- useReactTable returns mutable instances the Compiler can't memoise; the lab's table is 'use no memo' for the same reason
  const table = useReactTable<T>({
    columns,
    data,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getRowId: (row: T) => getRowId(row),
    state: { sorting: sorting ?? [] },
    ...(onSortingChange
      ? {
          onSortingChange: (u) => {
            onSortingChange(typeof u === "function" ? u(sorting ?? []) : u);
          },
        }
      : {}),
    manualSorting: !clientSort,
    enableSorting: !!onSortingChange,
  });
  const rows = table.getRowModel().rows;
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtual = rows.length > VIRTUAL_AFTER;
  const virtualizer = useVirtualizer({
    count: virtual ? rows.length : 0,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (i) => rows[i]?.id ?? i,
    overscan: 10,
  });
  const items = virtualizer.getVirtualItems();
  const padTop = virtual ? (items[0]?.start ?? 0) : 0;
  const padBottom = virtual ? virtualizer.getTotalSize() - (items.at(-1)?.end ?? 0) : 0;
  // [row, position] pairs: positions feed aria-rowindex, which only a virtualised table needs.
  const visible = virtual
    ? items.flatMap((i) => {
        const row = rows[i.index];
        return row ? [[row, i.index] as const] : [];
      })
    : rows.map((r, i) => [r, i] as const);
  const leaves = table.getAllLeafColumns();
  const scrollLabel = typeof caption === "string" ? caption : label;
  const sortedBy = new Map(
    (sorting ?? []).map((s) => [s.id, s.desc ? "descending" : "ascending"] as const),
  );

  return (
    // Virtualised, this div is the scroll box, so the Table's own overflow container is switched off:
    // otherwise it becomes the sticky header's scroll parent and the header never sticks.
    <div
      ref={scrollRef}
      className={cn(
        "min-w-0",
        virtual &&
          "max-h-[70vh] overflow-auto focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background focus-visible:outline-none [&>[data-slot=table-container]]:overflow-visible",
        className,
      )}
      // The scroll box is a named, keyboard-reachable region (axe `scrollable-region-focusable`); unvirtualised,
      // the Table's own container does the same when it overflows.
      {...(virtual ? { tabIndex: 0, role: "region", "aria-label": scrollLabel } : {})}
    >
      <Table
        aria-label={caption ? undefined : label}
        scrollLabel={scrollLabel}
        aria-rowcount={virtual ? rows.length + 1 : undefined}
      >
        {caption ? <TableCaption>{caption}</TableCaption> : null}
        <TableHeader className={cn(virtual && "sticky top-0 z-10 bg-card")}>
          {table.getHeaderGroups().map((group) => (
            <TableRow key={group.id} className="hover:bg-transparent">
              {group.headers.map((header) => {
                const col = header.column;
                const meta = col.columnDef.meta;
                const dir = sortedBy.get(col.id);
                const sortable = col.getCanSort();
                return (
                  <TableHead
                    key={header.id}
                    scope="col"
                    aria-sort={dir ?? (sortable ? "none" : undefined)}
                    className={cn(
                      "text-xs font-medium text-muted-foreground",
                      meta?.numeric && "text-right",
                      meta?.headerClassName,
                    )}
                  >
                    {header.isPlaceholder ? null : sortable ? (
                      <Button
                        variant="ghost"
                        size="sm"
                        className={cn("-mx-2 h-7 text-xs", meta?.numeric && "ml-auto flex")}
                        onClick={col.getToggleSortingHandler()}
                      >
                        {renderSlot(col.columnDef.header, header.getContext())}
                        {dir === "ascending" ? (
                          <ArrowUp aria-hidden />
                        ) : dir === "descending" ? (
                          <ArrowDown aria-hidden />
                        ) : (
                          <ArrowUpDown className="opacity-40" aria-hidden />
                        )}
                      </Button>
                    ) : (
                      renderSlot(col.columnDef.header, header.getContext())
                    )}
                  </TableHead>
                );
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
            const extra = rowProps?.(row.original);
            return (
              <TableRow key={row.id} aria-rowindex={virtual ? pos + 2 : undefined} {...extra}>
                {row.getAllCells().map((cell) => {
                  const meta = cell.column.columnDef.meta;
                  const className = cn(
                    meta?.numeric && "text-right tabular-nums",
                    meta?.cellClassName,
                  );
                  const content = renderSlot(cell.column.columnDef.cell, cell.getContext());
                  return meta?.rowHeader ? (
                    <TableHead
                      key={cell.id}
                      scope="row"
                      className={cn("font-medium text-foreground", className)}
                    >
                      {content}
                    </TableHead>
                  ) : (
                    <TableCell key={cell.id} className={className}>
                      {content}
                    </TableCell>
                  );
                })}
              </TableRow>
            );
          })}
          {padBottom > 0 ? (
            <tr aria-hidden>
              <td style={{ height: padBottom }} colSpan={leaves.length} />
            </tr>
          ) : null}
        </TableBody>
      </Table>
    </div>
  );
}
