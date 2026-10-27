/**
 * Lab addition (plans/feat-agent-centric.md eng, chart spike): stacked lanes on one shared time axis, the agent hub's
 * spend / requests / p95 / savings. The kit's own charts each own their scale and hover, so four of them can't share a
 * crosshair; this draws every lane against one band layout (`lane-band.ts`) with one hover source.
 *
 * - Each lane's SVG is one viewBox unit wide per bucket and stretched to the column (`preserveAspectRatio="none"`):
 *   the vertical axis stays in pixels, so 2 px bar edges stay 2 px, and lines keep their width with
 *   `vector-effect: non-scaling-stroke`. Nothing needs the measured width, so it renders the same in jsdom.
 * - The marks are presentation (aria-hidden). Keyboard and screen readers use the axis: one button per bucket in a
 *   roving tab stop (Arrow keys, Home, End; Enter or Space picks), each named with every lane's value. The page keeps
 *   a Table view as well.
 * - Bars carry the 2 px top edge (DESIGN.md "Charts"); a bucket the caller marks `hatched` is a diagonal hatch with no
 *   figures; a null value is a gap, never 0. The previous window draws as an outline behind each bar (`ghost`).
 * - Static: nothing animates, so reduced motion has nothing to stop.
 */
import { line as d3Line } from 'd3-shape'
import { useId, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import type { Series } from '@nasiko/ui/lib/chart'
import { cn } from '@nasiko/ui/lib/utils'
import { HatchPattern } from './hatch-pattern'
import { laneBand } from './lane-band'

/** ViewBox units per bucket: only the ratio matters, the SVG is stretched to the column. */
const UNIT = 10
const EDGE = 2
/** Head room over the tallest mark, so a peak's edge never touches the lane above. */
const HEAD = 6

export interface LaneSpec<R> {
  key: string
  kind: 'bar' | 'line'
  series: Series
  /** null is a gap (no observation), never drawn as 0. */
  value: (row: R) => number | null
  /** The previous window's value at the same position, drawn as an outline behind the bar. */
  ghost?: (row: R) => number | null
  /** The strip above the lane: its name, window total and delta (the page's own markup). */
  header: ReactNode
}

export interface LaneChartProps<R> {
  rows: readonly R[]
  /** A stable key per bucket (its start time). */
  rowKey: (row: R) => string
  lanes: readonly LaneSpec<R>[]
  /** Pixel height of each lane's plot. */
  height: number
  /** The bucket is outside the subject's life (e.g. before the agent existed): hatched, no figures. */
  hatched?: (row: R) => boolean
  /** Short axis text for a row; shown for at most `maxLabels` evenly spaced buckets. */
  axisLabel: (row: R, index: number) => string
  /** Each bucket button's accessible name: every lane's value for that bucket. */
  bucketName: (row: R, index: number) => string
  maxLabels?: number
  hoverIndex: number | null
  selectedIndex: number | null
  onHover: (index: number | null) => void
  onPick: (index: number) => void
  /** The synced tooltip's content for the hovered bucket (every lane's value). */
  tooltip?: (row: R, index: number) => ReactNode
  /** Drawn between the headers and the first lane, on the same band layout (the hub's marker rail). */
  rail?: ReactNode
  /** Names the bucket buttons' group, e.g. "Days. Arrow keys move between them; Enter selects one." */
  axisLabelText: string
  className?: string
}

function scaleMax(values: number[]): number {
  const max = Math.max(0, ...values)
  return max > 0 ? max : 1
}

function Lane<R>({
  rows,
  lane,
  height,
  hatched,
  hoverIndex,
  selectedIndex,
}: {
  rows: readonly R[]
  lane: LaneSpec<R>
  height: number
  hatched?: (row: R) => boolean
  hoverIndex: number | null
  selectedIndex: number | null
}) {
  const hatchId = `lane-hatch-${useId().replace(/:/g, '')}`
  const n = rows.length
  const band = laneBand(n, n * UNIT)
  const values = rows.map((r) => (hatched?.(r) ? null : lane.value(r)))
  const ghosts = rows.map((r) => (lane.ghost && !hatched?.(r) ? lane.ghost(r) : null))
  const max = scaleMax([...values, ...ghosts].filter((v): v is number => v !== null))
  const plot = height - HEAD
  const y = (v: number) => HEAD + plot - (v / max) * plot
  // One path per mark kind, not one element per bucket: an 800-bucket custom window stays a handful of nodes (review).
  const rect = (x: number, top: number, w: number, h: number) => `M${x},${top}h${w}v${h}h${-w}Z`
  let hatchPath = ''
  let ghostPath = ''
  let barPath = ''
  let edgePath = ''
  let lonePath = ''
  rows.forEach((r, i) => {
    if (hatched?.(r)) hatchPath += rect(band.slot(i), HEAD, band.step, plot)
    const g = ghosts[i] ?? null
    if (g !== null && g > 0) ghostPath += rect(band.barX(i), y(g), band.bar, height - y(g))
    const v = values[i] ?? null
    if (v === null) return
    if (lane.kind === 'bar' && v > 0) {
      const top = y(v)
      const h = Math.max(0, height - top)
      barPath += rect(band.barX(i), top, band.bar, h)
      edgePath += rect(band.barX(i), top, band.bar, Math.min(EDGE, h))
    }
    if (
      lane.kind === 'line' &&
      (values[i - 1] ?? null) === null &&
      (values[i + 1] ?? null) === null
    )
      lonePath += `M${band.center(i)},${y(v)}h0`
  })
  const path =
    lane.kind === 'line'
      ? d3Line<number | null>()
          .defined((v) => v !== null)
          .x((_, i) => band.center(i))
          .y((v) => y(v ?? 0))(values)
      : null
  return (
    <svg
      aria-hidden
      className="block w-full overflow-visible"
      style={{ height }}
      viewBox={`0 0 ${n * UNIT} ${height}`}
      preserveAspectRatio="none"
    >
      <HatchPattern id={hatchId} />
      <line
        x1={0}
        x2={n * UNIT}
        y1={height - 0.5}
        y2={height - 0.5}
        stroke="var(--border)"
        vectorEffect="non-scaling-stroke"
      />
      {selectedIndex !== null && selectedIndex < n ? (
        <rect
          x={band.slot(selectedIndex)}
          width={band.step}
          y={0}
          height={height}
          fill="var(--accent)"
        />
      ) : null}
      {hatchPath ? <path d={hatchPath} fill={`url(#${hatchId})`} opacity={0.6} /> : null}
      {ghostPath ? (
        <path
          d={ghostPath}
          fill="none"
          stroke="var(--muted-foreground)"
          strokeDasharray="3 2"
          vectorEffect="non-scaling-stroke"
        />
      ) : null}
      {barPath ? <path d={barPath} fill={lane.series.fill} /> : null}
      {edgePath ? <path d={edgePath} fill={lane.series.edge} /> : null}
      {/* A lone observation (null on both sides) is a zero-length subpath, invisible as a line: a round-capped dot. */}
      {lonePath ? (
        <path
          d={lonePath}
          fill="none"
          stroke={lane.series.edge}
          strokeWidth={6}
          strokeLinecap="round"
          vectorEffect="non-scaling-stroke"
        />
      ) : null}
      {path ? (
        <path
          d={path}
          fill="none"
          stroke={lane.series.edge}
          strokeWidth={2}
          vectorEffect="non-scaling-stroke"
        />
      ) : null}
      {hoverIndex !== null && hoverIndex < n ? (
        <line
          x1={band.center(hoverIndex)}
          x2={band.center(hoverIndex)}
          y1={0}
          y2={height}
          stroke="var(--chart-crosshair)"
          vectorEffect="non-scaling-stroke"
        />
      ) : null}
    </svg>
  )
}

export function LaneChart<R>(props: LaneChartProps<R>) {
  const { rows, lanes, hoverIndex, selectedIndex, onHover, onPick } = props
  const n = rows.length
  const keys = useRef<(HTMLButtonElement | null)[]>([])
  const maxLabels = props.maxLabels ?? 6
  const every = Math.max(1, Math.ceil(n / maxLabels))
  // The roving tab stop: the selected bucket, else the hovered one, else the newest.
  // The roving tab stop: the selected bucket, else the last one focused, else the newest; clamped, so a shorter window
  // never leaves every button at tabIndex -1. Hover never moves it (each move would re-render every button).
  const [focused, setFocused] = useState<number | null>(null)
  const stop = Math.min(n - 1, Math.max(0, selectedIndex ?? focused ?? n - 1))
  const indexFromPointer = (e: MouseEvent<HTMLElement>) => {
    const box = e.currentTarget.getBoundingClientRect()
    const i = laneBand(n, box.width).indexAt(e.clientX - box.left)
    return i < 0 ? null : i
  }
  const move = (to: number) => {
    const i = Math.min(n - 1, Math.max(0, to))
    onHover(i)
    keys.current[i]?.focus()
  }
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') move(i + 1)
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') move(i - 1)
    else if (e.key === 'Home') move(0)
    else if (e.key === 'End') move(n - 1)
    else return
    e.preventDefault()
  }
  const hovered = hoverIndex !== null ? rows[hoverIndex] : undefined
  const tipLeft = hoverIndex !== null && n > 0 ? ((hoverIndex + 0.5) / n) * 100 : 0

  return (
    <div className={cn('relative flex flex-col', props.className)}>
      {props.rail}
      {lanes.map((lane) => (
        <div key={lane.key} className="flex flex-col gap-1 pt-2">
          {lane.header}
          {/* Pointer-only (presentation): the axis buttons below reach the same buckets by keyboard. */}
          <div
            role="presentation"
            className="cursor-pointer"
            onPointerMove={(e) => onHover(indexFromPointer(e))}
            onPointerLeave={() => onHover(null)}
            onClick={(e) => {
              const i = indexFromPointer(e)
              if (i !== null) onPick(i)
            }}
          >
            <Lane
              rows={rows}
              lane={lane}
              height={props.height}
              hatched={props.hatched}
              hoverIndex={hoverIndex}
              selectedIndex={selectedIndex}
            />
          </div>
        </div>
      ))}
      <div
        role="group"
        aria-label={props.axisLabelText}
        className="grid pt-1"
        style={{ gridTemplateColumns: `repeat(${Math.max(1, n)}, minmax(0, 1fr))` }}
        onPointerLeave={() => onHover(null)}
      >
        {rows.map((r, i) => (
          <Button
            key={props.rowKey(r)}
            ref={(el) => {
              keys.current[i] = el
            }}
            variant="ghost"
            size="sm"
            tabIndex={i === stop ? 0 : -1}
            aria-label={props.bucketName(r, i)}
            aria-pressed={selectedIndex === i}
            className={cn(
              'h-6 min-w-0 justify-center overflow-visible rounded-sm px-0 text-2xs font-normal whitespace-nowrap text-muted-foreground tabular-nums hover:bg-transparent pointer-coarse:min-h-11',
              selectedIndex === i && 'text-foreground',
            )}
            onFocus={() => {
              setFocused(i)
              onHover(i)
            }}
            onPointerEnter={() => onHover(i)}
            onKeyDown={(e) => onKeyDown(e, i)}
            onClick={() => onPick(i)}
          >
            <span aria-hidden>{i % every === 0 || i === n - 1 ? props.axisLabel(r, i) : ''}</span>
          </Button>
        ))}
      </div>
      {hovered !== undefined && hoverIndex !== null && props.tooltip ? (
        <div
          aria-hidden
          className="pointer-events-none absolute top-0 z-10 min-w-36 -translate-x-1/2 rounded-md border border-border bg-popover px-3 py-2 text-xs text-popover-foreground shadow-md"
          style={{ left: `clamp(4.5rem, ${tipLeft}%, calc(100% - 4.5rem))` }}
        >
          {props.tooltip(hovered, hoverIndex)}
        </div>
      ) : null}
    </div>
  )
}
