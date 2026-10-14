"use client";

import { useId, useState, type KeyboardEvent } from "react";
import { cn } from "../lib/cn";

/**
 * Heatmap: a bounded grid of cells (ADR-0003 bespoke). Accessibility model:
 * the grid is a `role="grid"` with one roving tab stop; arrow keys move the
 * focused cell and each cell announces "row, column: value". A visually hidden
 * table carries the same data for screen-reader users who prefer tables.
 * Cell count is capped; the caller aggregates above it (no silent truncation).
 */
export interface HeatmapProps {
  readonly rows: readonly string[];
  readonly columns: readonly string[];
  /** values[rowIndex][columnIndex]; `null` = no data (rendered distinct from zero). */
  readonly values: readonly (readonly (number | null)[])[];
  readonly label: string;
  readonly formatValue?: (v: number) => string;
  readonly maxCells?: number;
  readonly className?: string;
  readonly onSelect?: (cell: { row: string; column: string; value: number | null }) => void;
}

export const HEATMAP_MAX_CELLS = 2_000;

/** 5-step sequential scale from the chart tokens; `null` is the empty pattern. */
function tone(v: number | null, max: number): string {
  if (v === null) return "var(--muted)";
  if (max <= 0) return "color-mix(in oklch, var(--chart-1) 15%, var(--card))";
  const f = Math.min(1, Math.max(0, v / max));
  const pct = 15 + Math.round(f * 85);
  return `color-mix(in oklch, var(--chart-1) ${pct}%, var(--card))`;
}

export function Heatmap({
  rows,
  columns,
  values,
  label,
  formatValue = (v) => String(v),
  maxCells = HEATMAP_MAX_CELLS,
  className,
  onSelect,
}: HeatmapProps) {
  const id = useId();
  const [focus, setFocus] = useState<[number, number]>([0, 0]);
  const cellCount = rows.length * columns.length;
  if (cellCount > maxCells) {
    return (
      <p role="status" className="text-sm text-muted-foreground">
        {label}: {cellCount} cells exceed the {maxCells}-cell limit; aggregate before rendering.
      </p>
    );
  }
  const max = values.flat().reduce<number>((m, v) => (v !== null && v > m ? v : m), 0);
  const cell = 18;
  const gap = 2;
  const labelW = 64;
  const w = labelW + columns.length * (cell + gap);
  const h = 20 + rows.length * (cell + gap);

  const onKey = (e: KeyboardEvent<SVGSVGElement>) => {
    const [r, c] = focus;
    const next: Record<string, [number, number]> = {
      ArrowRight: [r, Math.min(columns.length - 1, c + 1)],
      ArrowLeft: [r, Math.max(0, c - 1)],
      ArrowDown: [Math.min(rows.length - 1, r + 1), c],
      ArrowUp: [Math.max(0, r - 1), c],
      Home: [r, 0],
      End: [r, columns.length - 1],
    };
    const n = next[e.key];
    if (n) {
      e.preventDefault();
      setFocus(n);
    } else if ((e.key === "Enter" || e.key === " ") && onSelect) {
      e.preventDefault();
      onSelect({ row: rows[r] ?? "", column: columns[c] ?? "", value: values[r]?.[c] ?? null });
    }
  };

  const [fr, fc] = focus;
  const focusedValue = values[fr]?.[fc] ?? null;
  return (
    <figure className={cn("inline-block max-w-full overflow-x-auto", className)}>
      <svg
        viewBox={`0 0 ${w} ${h}`}
        width={w}
        height={h}
        role="grid"
        aria-labelledby={`${id}-t`}
        aria-rowcount={rows.length}
        aria-colcount={columns.length}
        aria-activedescendant={`${id}-${fr}-${fc}`}
        tabIndex={0}
        onKeyDown={onKey}
        className="outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        <title id={`${id}-t`}>{label}</title>
        {columns.map((col, c) => (
          <text
            key={col}
            x={labelW + c * (cell + gap) + cell / 2}
            y={12}
            textAnchor="middle"
            fontSize={9}
            fill="var(--chart-foreground-muted)"
            aria-hidden="true"
          >
            {col}
          </text>
        ))}
        {rows.map((row, r) => (
          <g key={row} role="row" aria-rowindex={r + 1}>
            <text
              x={labelW - 6}
              y={20 + r * (cell + gap) + cell / 2 + 3}
              textAnchor="end"
              fontSize={9}
              fill="var(--chart-foreground-muted)"
              aria-hidden="true"
            >
              {row}
            </text>
            {columns.map((col, c) => {
              const v = values[r]?.[c] ?? null;
              const focused = r === fr && c === fc;
              return (
                <rect
                  key={col}
                  id={`${id}-${r}-${c}`}
                  role="gridcell"
                  aria-colindex={c + 1}
                  aria-label={`${row}, ${col}: ${v === null ? "no data" : formatValue(v)}`}
                  aria-selected={focused}
                  x={labelW + c * (cell + gap)}
                  y={20 + r * (cell + gap)}
                  width={cell}
                  height={cell}
                  rx={3}
                  fill={tone(v, max)}
                  stroke={focused ? "var(--ring)" : "transparent"}
                  strokeWidth={focused ? 2 : 0}
                  onClick={() => {
                    setFocus([r, c]);
                    onSelect?.({ row, column: col, value: v });
                  }}
                  style={{ transition: "fill var(--motion-feedback) var(--motion-ease-out)" }}
                >
                  {v === null ? (
                    <title>{`${row}, ${col}: no data`}</title>
                  ) : (
                    <title>{`${row}, ${col}: ${formatValue(v)}`}</title>
                  )}
                </rect>
              );
            })}
          </g>
        ))}
      </svg>
      <figcaption className="sr-only" aria-live="polite">
        {rows[fr]}, {columns[fc]}: {focusedValue === null ? "no data" : formatValue(focusedValue)}
      </figcaption>
      <div className="sr-only">
        <table>
          <caption>{label}</caption>
          <thead>
            <tr>
              <th scope="col">Row</th>
              {columns.map((c) => (
                <th key={c} scope="col">
                  {c}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, r) => (
              <tr key={row}>
                <th scope="row">{row}</th>
                {columns.map((col, c) => {
                  const v = values[r]?.[c] ?? null;
                  return <td key={col}>{v === null ? "no data" : formatValue(v)}</td>;
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </figure>
  );
}
