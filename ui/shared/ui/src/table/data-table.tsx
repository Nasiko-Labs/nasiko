"use client";

import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
  type SortingState,
} from "@tanstack/react-table";
import { ArrowDownIcon, ArrowUpIcon, ArrowUpDownIcon } from "lucide-react";
import { useId, type ReactNode } from "react";
import { cn } from "../lib/cn";
import { describeTotal, type Page } from "@nasiko/data-access";
import { Button } from "../primitives/button";
import { Skeleton } from "../primitives/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../primitives/table";

/**
 * DataTable: TanStack Table 8 (headless) + shadcn table primitives, with the
 * data-access `Page<T>` as its input. Pagination and sorting are *server*
 * operations: the table reports the user's intent (`onPageChange`,
 * `onSortChange`) and renders whatever page the caller fetched. It never sorts
 * or slices rows itself (contract §9: server-backed pagination for large data).
 *
 * Motion (§8.3 tables): no row-by-row entrance, no animated resort. Loading
 * keeps the header and shows stable skeleton rows of the same height.
 * Selection and focus are the caller's state and survive page updates.
 */
export interface DataTableProps<T> {
  readonly columns: readonly ColumnDef<T>[];
  /** The current page, or `null` while the first page loads. */
  readonly page: Page<T> | null;
  readonly loading?: boolean;
  /** Error already classified by the caller; rendered in place of rows. */
  readonly error?: ReactNode;
  readonly emptyMessage?: string;
  readonly sorting?: SortingState;
  readonly onSortChange?: (s: SortingState) => void;
  readonly onPageChange?: (offset: number) => void;
  readonly getRowId?: (row: T, index: number) => string;
  readonly caption: string;
  readonly className?: string;
  /** Rows the caller treats as stale (a refetch is in flight after a filter change). */
  readonly stale?: boolean;
}

export function DataTable<T>({
  columns,
  page,
  loading = false,
  error,
  emptyMessage = "No rows",
  sorting = [],
  onSortChange,
  onPageChange,
  getRowId,
  caption,
  className,
  stale = false,
}: DataTableProps<T>) {
  const id = useId();
  const rows = page?.rows ?? [];
  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Table's hook returns functions the React Compiler can't memoise; the compiler is not enabled here (an experiment, ADR note), so this only says it would skip this component
  const table = useReactTable<T>({
    data: rows as T[],
    columns: columns as ColumnDef<T>[],
    state: { sorting },
    manualSorting: true,
    manualPagination: true,
    enableSortingRemoval: true,
    onSortingChange: (u) => {
      const next = typeof u === "function" ? u(sorting) : u;
      onSortChange?.(next);
    },
    getCoreRowModel: getCoreRowModel(),
    ...(getRowId ? { getRowId } : {}),
  });
  const skeletonRows = page ? Math.min(page.limit, 10) : 5;
  const canPrev = !!page && page.offset > 0;
  const canNext =
    !!page &&
    (page.total === "unknown"
      ? page.rows.length === page.limit
      : page.offset + page.limit < page.total);

  return (
    <div
      className={cn("flex flex-col gap-2", className)}
      aria-busy={loading}
      data-stale={stale ? "true" : "false"}
    >
      <Table scrollLabel={caption}>
        <caption className="sr-only">{caption}</caption>
        <TableHeader>
          {table.getHeaderGroups().map((hg) => (
            <TableRow key={hg.id}>
              {hg.headers.map((h) => {
                const sortable = h.column.getCanSort() && !!onSortChange;
                const dir = h.column.getIsSorted();
                return (
                  <TableHead
                    key={h.id}
                    aria-sort={
                      dir === "asc"
                        ? "ascending"
                        : dir === "desc"
                          ? "descending"
                          : sortable
                            ? "none"
                            : undefined
                    }
                  >
                    {h.isPlaceholder ? null : sortable ? (
                      <Button
                        variant="ghost"
                        size="sm"
                        className="-ml-2 h-7 gap-1"
                        onClick={h.column.getToggleSortingHandler()}
                      >
                        {flexRender(h.column.columnDef.header, h.getContext())}
                        {dir === "asc" ? (
                          <ArrowUpIcon aria-hidden />
                        ) : dir === "desc" ? (
                          <ArrowDownIcon aria-hidden />
                        ) : (
                          <ArrowUpDownIcon aria-hidden className="opacity-50" />
                        )}
                      </Button>
                    ) : (
                      flexRender(h.column.columnDef.header, h.getContext())
                    )}
                  </TableHead>
                );
              })}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody className={cn(stale && "opacity-70 transition-opacity duration-feedback")}>
          {error ? (
            <TableRow>
              <TableCell
                colSpan={columns.length}
                className="py-6 text-center text-sm text-destructive"
              >
                {error}
              </TableCell>
            </TableRow>
          ) : loading && rows.length === 0 ? (
            Array.from({ length: skeletonRows }, (_, i) => (
              <TableRow key={`${id}-s-${i}`} aria-hidden="true">
                {columns.map((_, c) => (
                  <TableCell key={c}>
                    <Skeleton className="h-4 w-[60%]" />
                  </TableCell>
                ))}
              </TableRow>
            ))
          ) : rows.length === 0 ? (
            <TableRow>
              <TableCell
                colSpan={columns.length}
                className="py-6 text-center text-sm text-muted-foreground"
              >
                {emptyMessage}
              </TableCell>
            </TableRow>
          ) : (
            table.getRowModel().rows.map((r) => (
              <TableRow key={r.id} data-state={r.getIsSelected() ? "selected" : undefined}>
                {r.getVisibleCells().map((cell) => (
                  <TableCell key={cell.id}>
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </TableCell>
                ))}
              </TableRow>
            ))
          )}
        </TableBody>
      </Table>
      {page ? (
        <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
          <span aria-live="polite">
            {describeTotal(page.total, page.offset + page.rows.length)}
          </span>
          {onPageChange ? (
            <div className="flex gap-1">
              <Button
                variant="outline"
                size="xs"
                disabled={!canPrev || loading}
                onClick={() => {
                  onPageChange(Math.max(0, page.offset - page.limit));
                }}
              >
                Previous
              </Button>
              <Button
                variant="outline"
                size="xs"
                disabled={!canNext || loading}
                onClick={() => {
                  onPageChange(page.offset + page.limit);
                }}
              >
                Next
              </Button>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
